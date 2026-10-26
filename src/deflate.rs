// Copyright (c) 2026 okooo5km(十里). All rights reserved.
// Licensed under the MIT License.

//! zlib stream compression for PNG IDAT data, built on libdeflate.
//!
//! libdeflate's strongest levels (11/12, near-optimal parsing) compress palette
//! indices ~6–7% smaller than zlib level 9, but are an order of magnitude slower and
//! single-threaded. Since the encode stage runs after quantization has released the
//! thread pool, the input is split into independent row-aligned chunks that are
//! compressed in parallel and stitched into one zlib stream (the pigz/gzjoin
//! technique): every chunk but the last has the BFINAL bit of its final block cleared
//! and is padded to a byte boundary with an empty stored block, so the concatenation
//! is a single valid deflate stream. Each chunk starts with an empty window, which
//! costs only a few bytes per boundary at the chunk sizes used here.
//!
//! The chunk layout depends only on the input size — never on the thread count — so
//! output bytes are identical on every machine.

use rayon::prelude::*;

/// Chunks smaller than this lose too much to the cold window at each boundary.
const MIN_CHUNK_BYTES: usize = 64 * 1024;
/// Inputs below `SMALL_IDAT_BYTES` use this chunk size instead: they would otherwise
/// be one serial chunk, and the cold windows cost ~2% there, less than the level
/// choice saves in time.
const SMALL_MIN_CHUNK_BYTES: usize = 16 * 1024;
const SMALL_IDAT_BYTES: usize = 128 * 1024;
/// Inputs from this size up get libdeflate level 12; smaller ones level 10. Measured
/// on 80 quantized outputs (filtered palette indices): level 10 is 0.6-1.3% larger
/// than 12 at a quarter to a third of the time, and still 3-5% smaller and faster
/// than zlib level 9. Small and medium images are where encode time dominates the
/// total, large ones already run well ahead of pngquant.
const LEVEL12_MIN_BYTES: usize = 2 * 1024 * 1024;

/// Compress filtered IDAT scanlines, picking the method by input size and
/// redundancy (see `zlib_compress_idat_probed`).
pub(crate) fn zlib_compress_idat(data: &[u8], row_len: usize) -> Vec<u8> {
    if data.len() < SMALL_IDAT_BYTES {
        // Small inputs are rarely redundant, so the level 10 pass runs alongside
        // the probe instead of after it; same output, minus the serial probe.
        let (probe, level10) = rayon::join(
            || probe_size(data),
            || compress_rows(data, row_len, 10, SMALL_MIN_CHUNK_BYTES),
        );
        if let Some(zlib) = redundant_zlib(data, probe) {
            return zlib;
        }
        return level10;
    }
    let probe = if data.len() >= LEVEL12_MIN_BYTES {
        0
    } else {
        probe_size(data)
    };
    zlib_compress_idat_probed(data, row_len, probe)
}

/// Rough compressed size of `data` from a fast libdeflate level 6 pass (parallel,
/// a few percent of a level 10 pass). Used to choose between alternative encodings
/// of the same image and to detect highly redundant data.
pub(crate) fn probe_size(data: &[u8]) -> usize {
    let n = chunk_count(data.len(), MIN_CHUNK_BYTES);
    let parts: Vec<&[u8]> = data.chunks(data.len().div_ceil(n).max(1)).collect();
    parts
        .par_iter()
        .map(|part| {
            let mut c = compressor(6);
            let mut raw = vec![0u8; c.deflate_compress_bound(part.len())];
            c.deflate_compress(part, &mut raw).unwrap_or(part.len())
        })
        .sum()
}

/// `zlib_compress_idat` with a `probe_size` result already at hand:
/// - from `LEVEL12_MIN_BYTES` up, libdeflate level 12;
/// - highly redundant data (probe ratio of at least `REDUNDANT_RATIO`: flat icons,
///   line art), zlib-rs level 9 with mem_level 5 and 8: lazy matching does as well
///   as libdeflate 11 there, beats level 10 by up to 10%, and is fast on such input;
/// - everything else, libdeflate level 10, in smaller chunks below `SMALL_IDAT_BYTES`.
pub(crate) fn zlib_compress_idat_probed(data: &[u8], row_len: usize, probe: usize) -> Vec<u8> {
    if data.len() >= LEVEL12_MIN_BYTES {
        return compress_rows(data, row_len, 12, MIN_CHUNK_BYTES);
    }
    if let Some(zlib) = redundant_zlib(data, probe) {
        return zlib;
    }
    let min_chunk = if data.len() >= SMALL_IDAT_BYTES {
        MIN_CHUNK_BYTES
    } else {
        SMALL_MIN_CHUNK_BYTES
    };
    compress_rows(data, row_len, 10, min_chunk)
}

/// Probe compression ratio from which data counts as highly redundant.
const REDUNDANT_RATIO: usize = 20;

/// For highly redundant data (see `zlib_compress_idat_probed`), the smaller of
/// zlib-rs level 9 with mem_level 5 and 8.
fn redundant_zlib(data: &[u8], probe: usize) -> Option<Vec<u8>> {
    if probe.saturating_mul(REDUNDANT_RATIO) > data.len() {
        return None;
    }
    let (ml5, ml8) = rayon::join(|| zlib_rs_level9(data, 5), || zlib_rs_level9(data, 8));
    [ml5, ml8].into_iter().flatten().min_by_key(Vec::len)
}
/// Upper bound on chunk count: enough to fill a typical 8-core machine; every extra
/// boundary costs a cold window and a new Huffman header.
const MAX_CHUNKS: usize = 8;

/// Compress `data` (a sequence of `row_len`-byte filtered scanlines) into a zlib
/// stream with libdeflate at `level`, splitting at row boundaries for parallelism.
#[cfg(test)]
pub(crate) fn zlib_compress_rows(data: &[u8], row_len: usize, level: i32) -> Vec<u8> {
    compress_rows(data, row_len, level, MIN_CHUNK_BYTES)
}

fn compress_rows(data: &[u8], row_len: usize, level: i32, min_chunk: usize) -> Vec<u8> {
    let chunk_count = chunk_count(data.len(), min_chunk);
    if chunk_count > 1 && row_len > 0 {
        let rows = data.len() / row_len;
        let rows_per_chunk = rows.div_ceil(chunk_count);
        let parts: Vec<&[u8]> = data.chunks(rows_per_chunk * row_len).collect();
        if let Some(stream) = compress_and_stitch(data, &parts, level) {
            return stream;
        }
    }
    let stream = zlib_compress_single(data, level);
    if data.len() < SMALL_INPUT_BYTES {
        // libdeflate falls back to stored blocks on tiny inputs where zlib's fixed
        // Huffman block is a few bytes smaller; both are negligible in cost here.
        if let Some(zlib) = zlib_rs_level9(data, 8)
            && zlib.len() < stream.len()
        {
            return zlib;
        }
    }
    stream
}

/// Inputs below this size also try zlib-rs level 9 (see `zlib_compress_rows`).
const SMALL_INPUT_BYTES: usize = 16 * 1024;

fn zlib_rs_level9(data: &[u8], mem_level: i32) -> Option<Vec<u8>> {
    let config = zlib_rs::DeflateConfig {
        level: 9,
        mem_level,
        ..zlib_rs::DeflateConfig::default()
    };
    let mut out = vec![0u8; zlib_rs::compress_bound(data.len())];
    let (compressed, rc) = zlib_rs::compress_slice(&mut out, data, config);
    if rc != zlib_rs::ReturnCode::Ok {
        return None;
    }
    let len = compressed.len();
    out.truncate(len);
    Some(out)
}

fn chunk_count(len: usize, min_chunk: usize) -> usize {
    (len / min_chunk).clamp(1, MAX_CHUNKS)
}

fn compressor(level: i32) -> libdeflater::Compressor {
    let level = libdeflater::CompressionLvl::new(level).unwrap_or_default();
    libdeflater::Compressor::new(level)
}

fn zlib_compress_single(data: &[u8], level: i32) -> Vec<u8> {
    let mut c = compressor(level);
    let mut out = vec![0u8; c.zlib_compress_bound(data.len())];
    let len = c
        .zlib_compress(data, &mut out)
        .expect("output buffer sized by zlib_compress_bound");
    out.truncate(len);
    out
}

fn compress_and_stitch(data: &[u8], parts: &[&[u8]], level: i32) -> Option<Vec<u8>> {
    let last = parts.len() - 1;
    let pieces: Vec<Option<Vec<u8>>> = parts
        .par_iter()
        .enumerate()
        .map(|(i, part)| {
            let mut c = compressor(level);
            let mut raw = vec![0u8; c.deflate_compress_bound(part.len())];
            let len = c.deflate_compress(part, &mut raw).ok()?;
            raw.truncate(len);
            if i != last {
                make_non_final(&mut raw)?;
            }
            Some(raw)
        })
        .collect();

    let total: usize = pieces.iter().map(|p| p.as_ref().map_or(0, Vec::len)).sum();
    let mut out = Vec::with_capacity(total + 6);
    // CMF=0x78 (deflate, 32K window), FLG=0xDA (max compression, FCHECK valid).
    out.extend_from_slice(&[0x78, 0xDA]);
    for piece in pieces {
        out.extend_from_slice(&piece?);
    }
    out.extend_from_slice(&libdeflater::adler32(data).to_be_bytes());
    Some(out)
}

/// Turn a complete raw deflate stream into a non-final prefix that ends on a byte
/// boundary: clear BFINAL on its last block, then append an empty stored block
/// (the same byte-aligning marker zlib emits for `Z_SYNC_FLUSH`).
fn make_non_final(raw: &mut Vec<u8>) -> Option<()> {
    let (final_start, end) = find_final_block(raw)?;
    raw[final_start / 8] &= !(1 << (final_start % 8));
    raw.truncate(end.div_ceil(8));
    let used = end % 8;
    if used != 0 {
        // Zero the padding bits after the end-of-block code.
        let last = raw.len() - 1;
        raw[last] &= (1u8 << used) - 1;
    }
    // A stored-block header is 3 zero bits (BFINAL=0, BTYPE=00) followed by padding to
    // the byte boundary. It fits in the current byte's free bits only if there are >= 3.
    if used == 0 || used > 5 {
        raw.push(0);
    }
    raw.extend_from_slice(&[0x00, 0x00, 0xFF, 0xFF]);
    Some(())
}

// ── Minimal deflate block walker ──
//
// Decodes block headers and Huffman symbols (without producing output) to locate the
// final block's header bit and the bit just past its end-of-block code.

struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl BitReader<'_> {
    fn peek(&self, n: u32) -> u32 {
        let byte = self.pos / 8;
        let mut buf = [0u8; 8];
        let avail = self.data.len().saturating_sub(byte).min(8);
        buf[..avail].copy_from_slice(&self.data[byte..byte + avail]);
        let word = u64::from_le_bytes(buf) >> (self.pos % 8);
        (word & ((1u64 << n) - 1)) as u32
    }

    fn bits(&mut self, n: u32) -> Option<u32> {
        if self.pos + n as usize > self.data.len() * 8 {
            return None;
        }
        let v = self.peek(n);
        self.pos += n as usize;
        Some(v)
    }
}

/// Canonical Huffman decode table indexed by the next `max_len` stream bits.
struct Huffman {
    /// (symbol, code length); length 0 marks an unused (invalid) code.
    table: Vec<(u16, u8)>,
    max_len: u32,
}

impl Huffman {
    fn new(lengths: &[u8]) -> Option<Self> {
        let max_len = u32::from(*lengths.iter().max()?);
        if max_len == 0 || max_len > 15 {
            return None;
        }
        let mut count = [0u32; 16];
        for &l in lengths {
            count[l as usize] += 1;
        }
        count[0] = 0;
        let mut next = [0u32; 16];
        let mut code = 0u32;
        for len in 1..16 {
            code = (code + count[len - 1]) << 1;
            next[len] = code;
        }
        let size = 1usize << max_len;
        let mut table = vec![(0u16, 0u8); size];
        for (sym, &len) in lengths.iter().enumerate() {
            if len == 0 {
                continue;
            }
            let len = u32::from(len);
            let c = next[len as usize];
            next[len as usize] += 1;
            if c >= 1 << len {
                return None; // over-subscribed
            }
            let rev = (c.reverse_bits() >> (32 - len)) as usize;
            let mut idx = rev;
            while idx < size {
                table[idx] = (sym as u16, len as u8);
                idx += 1 << len;
            }
        }
        Some(Self { table, max_len })
    }

    fn decode(&self, r: &mut BitReader) -> Option<u16> {
        let (sym, len) = self.table[r.peek(self.max_len) as usize];
        if len == 0 {
            return None;
        }
        r.bits(u32::from(len))?;
        Some(sym)
    }
}

const LENGTH_EXTRA: [u8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
const DIST_EXTRA: [u8; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13,
];
const CLEN_ORDER: [usize; 19] = [
    16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
];

fn fixed_tables() -> Option<(Huffman, Huffman)> {
    let mut lit = [0u8; 288];
    lit[..144].fill(8);
    lit[144..256].fill(9);
    lit[256..280].fill(7);
    lit[280..].fill(8);
    Some((Huffman::new(&lit)?, Huffman::new(&[5u8; 30])?))
}

fn dynamic_tables(r: &mut BitReader) -> Option<(Huffman, Huffman)> {
    let hlit = r.bits(5)? as usize + 257;
    let hdist = r.bits(5)? as usize + 1;
    let hclen = r.bits(4)? as usize + 4;
    let mut clen = [0u8; 19];
    for &slot in &CLEN_ORDER[..hclen] {
        clen[slot] = r.bits(3)? as u8;
    }
    let clen_table = Huffman::new(&clen)?;
    let mut lengths = vec![0u8; hlit + hdist];
    let mut i = 0;
    while i < lengths.len() {
        let sym = clen_table.decode(r)?;
        let (value, repeat) = match sym {
            0..=15 => (sym as u8, 1),
            16 => (*lengths.get(i.checked_sub(1)?)?, 3 + r.bits(2)? as usize),
            17 => (0, 3 + r.bits(3)? as usize),
            18 => (0, 11 + r.bits(7)? as usize),
            _ => return None,
        };
        if i + repeat > lengths.len() {
            return None;
        }
        lengths[i..i + repeat].fill(value);
        i += repeat;
    }
    let (lit, dist) = lengths.split_at(hlit);
    Some((Huffman::new(lit)?, Huffman::new(dist)?))
}

fn skip_compressed_block(r: &mut BitReader, lit: &Huffman, dist: &Huffman) -> Option<()> {
    loop {
        let sym = lit.decode(r)?;
        match sym {
            0..=255 => {}
            256 => return Some(()),
            257..=285 => {
                r.bits(u32::from(LENGTH_EXTRA[(sym - 257) as usize]))?;
                let d = dist.decode(r)? as usize;
                r.bits(u32::from(*DIST_EXTRA.get(d)?))?;
            }
            _ => return None,
        }
    }
}

/// Returns `(final_block_header_bit, end_bit)` for a complete raw deflate stream.
fn find_final_block(data: &[u8]) -> Option<(usize, usize)> {
    let mut r = BitReader { data, pos: 0 };
    let mut fixed = None;
    loop {
        let start = r.pos;
        let bfinal = r.bits(1)? == 1;
        match r.bits(2)? {
            0 => {
                r.pos = r.pos.div_ceil(8) * 8;
                let len = r.bits(16)? as usize;
                let nlen = r.bits(16)? as usize;
                if len != !nlen & 0xFFFF {
                    return None;
                }
                r.pos += len * 8;
                if r.pos > data.len() * 8 {
                    return None;
                }
            }
            1 => {
                if fixed.is_none() {
                    fixed = Some(fixed_tables()?);
                }
                let (lit, dist) = fixed.as_ref()?;
                skip_compressed_block(&mut r, lit, dist)?;
            }
            2 => {
                let (lit, dist) = dynamic_tables(&mut r)?;
                skip_compressed_block(&mut r, &lit, &dist)?;
            }
            _ => return None,
        }
        if bfinal {
            return Some((start, r.pos));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    fn inflate(stream: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        flate2::read::ZlibDecoder::new(stream)
            .read_to_end(&mut out)
            .expect("valid zlib stream");
        out
    }

    /// Deterministic pseudo-random palette-index-like rows with runs and noise.
    fn sample_rows(rows: usize, row_len: usize, seed: u32) -> Vec<u8> {
        let mut s = seed;
        let mut out = Vec::with_capacity(rows * row_len);
        for y in 0..rows {
            out.push(0);
            for x in 0..row_len - 1 {
                s = s.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                let noise = (s >> 16) as u8;
                let v = if noise < 200 {
                    ((x / 7 + y / 5) % 23) as u8
                } else {
                    noise
                };
                out.push(v);
            }
        }
        out
    }

    #[test]
    fn stitched_stream_round_trips() {
        for (rows, row_len) in [(1200, 1001), (3000, 257), (64, 40_000), (10, 7)] {
            let data = sample_rows(rows, row_len, rows as u32);
            for level in [1, 6, 12] {
                let stream = zlib_compress_rows(&data, row_len, level);
                assert_eq!(
                    inflate(&stream),
                    data,
                    "rows={rows} len={row_len} lvl={level}"
                );
            }
        }
    }

    #[test]
    fn idat_paths_round_trip() {
        // Small (16 KiB chunks), medium (64 KiB chunks), large (level 12) and
        // highly redundant (zlib-rs) inputs.
        let flat = vec![0u8; 300 * 1001];
        let cases = [
            sample_rows(100, 1001, 1),
            sample_rows(400, 1001, 2),
            sample_rows(2200, 1001, 3),
            flat,
        ];
        for data in cases {
            let stream = zlib_compress_idat(&data, 1001);
            assert_eq!(inflate(&stream), data, "len={}", data.len());
        }
    }

    #[test]
    fn make_non_final_handles_all_block_types() {
        // Stored (level 0), fixed-Huffman-friendly tiny input, and dynamic blocks,
        // across varied lengths so the end-of-block lands at every bit offset.
        for level in [0, 1, 12] {
            for n in 1..48usize {
                let first: Vec<u8> = (0..n * 37).map(|i| (i * 7 % 13) as u8).collect();
                let second: Vec<u8> = (0..n * 11).map(|i| (i % 5) as u8).collect();
                let mut c = compressor(level);
                let mut a = vec![0u8; c.deflate_compress_bound(first.len())];
                let la = c.deflate_compress(&first, &mut a).unwrap();
                a.truncate(la);
                make_non_final(&mut a).expect("walkable stream");
                let mut b = vec![0u8; c.deflate_compress_bound(second.len())];
                let lb = c.deflate_compress(&second, &mut b).unwrap();
                b.truncate(lb);

                let mut all = first.clone();
                all.extend_from_slice(&second);
                let mut stream = vec![0x78, 0xDA];
                stream.extend_from_slice(&a);
                stream.extend_from_slice(&b);
                stream.extend_from_slice(&libdeflater::adler32(&all).to_be_bytes());
                assert_eq!(inflate(&stream), all, "level={level} n={n}");
            }
        }
    }

    #[test]
    fn chunk_layout_ignores_thread_count() {
        assert_eq!(chunk_count(0, MIN_CHUNK_BYTES), 1);
        assert_eq!(chunk_count(MIN_CHUNK_BYTES * 2 - 1, MIN_CHUNK_BYTES), 1);
        assert_eq!(chunk_count(MIN_CHUNK_BYTES * 3, MIN_CHUNK_BYTES), 3);
        assert_eq!(chunk_count(usize::MAX / 2, MIN_CHUNK_BYTES), MAX_CHUNKS);
    }
}
