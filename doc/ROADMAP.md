# PNGOptim Optimization Roadmap

> Author: okooo5km(十里)
> Created: 2026-03-07

This roadmap is based on detailed investigation of the codebase, reference implementation (libimagequant), and benchmarking against pngquant. It covers remaining algorithm gaps, encoding improvements, decoding optimizations, and parallelization opportunities.

---

## 1. Algorithm Gaps (Quality-Affecting)

### 1.1 Replace Unused Colors After Remap K-Means Finalize

**Priority**: Medium
**Impact**: Marginal quality improvement for images with many similar colors
**File**: `src/palette_quant.rs` (~line 1576)

After `apply_remap_feedback()` in the remap path, the reference implementation calls `replace_unused_colors()` via VP-tree to fill unused palette slots with distant colors. Our implementation skips this step.

**Reference**: `/Users/5km/Dev/C/libimagequant/src/remap.rs` — `remap_to_palette()` calls `kmeans_finalize()` which internally replaces unused colors.

**Action**: After `apply_remap_feedback()`, scan the palette for colors with zero usage count and replace them using `find_nearest_other_color()` from the VP-tree, selecting the color most distant from its nearest neighbor.

---

## 2. Encoding Optimizations

### 2.1 Switch to libdeflater for Deflate Compression

**Priority**: High
**Impact**: 2-3x faster encoding with equivalent compression ratio
**Effort**: Medium

Currently using `flate2` with `zlib-ng` backend. `libdeflater` (Rust binding: `libdeflater` crate) is significantly faster for single-buffer compression, which is our use case (we compress the entire filtered image data at once).

**Approach**:
1. Add `libdeflater` crate dependency
2. Replace `flate2::write::ZlibEncoder` with `libdeflater::Compressor` in the PNG encoding path
3. Need to handle IDAT chunk splitting manually (libdeflater produces a single buffer)
4. Benchmark across compression levels 1-12

**Risk**: libdeflater is a C dependency (like zlib-ng). Pure-Rust fallback would need `flate2` retained.

### 2.2 Hand-Written PNG Encoder

**Priority**: Medium
**Impact**: Eliminates `png` crate encoder overhead, enables direct IDAT control
**Effort**: Medium (~200 lines)

The `png` crate encoder adds overhead for features we don't use (interlacing, 16-bit, etc.). A minimal encoder for indexed-color PNG would:
1. Write PNG signature (8 bytes)
2. Write IHDR chunk (13 bytes payload)
3. Write PLTE chunk (3 * num_colors bytes)
4. Write tRNS chunk (if alpha present)
5. Apply PNG row filters (None, Sub, Up, Average, Paeth — try all, pick smallest)
6. Compress filtered data with libdeflater
7. Write IDAT chunk(s)
8. Optionally write ancillary chunks (gAMA, sRGB, etc.)
9. Write IEND chunk

**Benefit**: Full control over filter selection strategy, compression parameters, and chunk layout. Enables multi-level parallel compression (see 4.3).

### 2.3 Zopfli/Zenzop for `--best` Mode

**Priority**: Low
**Impact**: 3-8% smaller files, 10-50x slower
**Effort**: Low

Add an optional `--best` flag that uses `zopfli` (or `zenzop`, the Rust port) for maximum compression. This is a simple addition once we have a hand-written encoder.

**Crates**: `zopfli` or `zenzop`

---

## 3. Decoding Optimizations

### 3.1 Remove `image` Crate Dependency

**Priority**: Low
**Impact**: ~1-2% decode speedup, reduced binary size
**Effort**: Low

Currently using the `image` crate which wraps the `png` crate. Direct use of `png` crate eliminates the `image` crate's generic abstraction layer.

The `png` crate 0.18 is already the fastest Rust PNG decoder. Other alternatives (`zune-png`, `lodepng`) are 60-90% slower. No benefit to switching decoders.

**Action**: Replace `image::open()` / `image::load_from_memory()` with direct `png::Decoder` usage. We already use `png::Decoder` in `extract_metadata()` and `decode_apng()`, so the pattern is established.

---

## 4. Parallelization Opportunities

### 4.1 Already Parallelized (Done)

| Component | Method | Speedup |
|-----------|--------|---------|
| RGBA→InternalPixel conversion | `par_chunks_exact` | ~2x |
| `compute_contrast_maps` | Per-row parallel | ~2x (47ms → 25ms) |
| Histogram building (`op3`) | Per-row parallel | ~2x |
| `build_histogram_map` | Per-thread HashMaps + merge | ~3x (14ms → 4ms) |
| K-Means iteration | `par_chunks` | ~2x |
| Floyd dithering (chunked) | `par_chunks_mut` | ~1.5x |

### 4.2 Plain Remap Parallelization

**Priority**: High
**Impact**: Significant for non-dithered path
**File**: `src/palette_quant.rs` — `remap_image_plain_pass()`

Currently single-threaded because k-means accumulators are shared mutable state. Solution: per-thread accumulators merged after parallel pass.

**Approach**:
```
1. Split pixel rows into chunks
2. Each thread maintains local KMeansAccumulator
3. Each thread finds nearest palette color via VP-tree (read-only)
4. Merge all thread-local accumulators after parallel pass
5. Row hints still work within each chunk
```

### 4.3 Multi-Level Parallel Compression

**Priority**: Medium
**Impact**: Near-zero cost to try multiple compression levels
**Effort**: Low (with hand-written encoder)

Try compression levels 6, 9, and 12 in parallel, pick the smallest result. With `libdeflater`, each level takes ~5-15ms for a typical quantized image, so running 3 in parallel costs the same wall time as running 1.

### 4.4 `transposing_1d_blur` Parallelization

**Priority**: Low
**Impact**: Minor (~5ms savings on large images)
**File**: `src/palette_quant.rs`

The vertical blur pass in `compute_contrast_maps` processes columns sequentially. Can be parallelized with column-chunked processing, but the benefit is small since the horizontal pass is already parallel.

---

## 5. Implementation Priority Order

| # | Item | Priority | Impact | Effort |
|---|------|----------|--------|--------|
| 1 | Plain remap parallelization (4.2) | High | Perf | Medium |
| 2 | libdeflater integration (2.1) | High | Perf | Medium |
| 3 | Replace unused colors (1.1) | Medium | Quality | Low |
| 4 | Hand-written PNG encoder (2.2) | Medium | Perf | Medium |
| 5 | Multi-level parallel compression (4.3) | Medium | Perf/Size | Low |
| 6 | Remove `image` crate (3.1) | Low | Perf/Size | Low |
| 7 | Zopfli `--best` mode (2.3) | Low | Size | Low |
| 8 | `transposing_1d_blur` parallel (4.4) | Low | Perf | Low |

---

## 6. Phase H: APNG Support

APNG pipeline integration is complete (H1). Remaining work:

### H2: Lossless Optimizations (Done)
- Duplicate frame folding
- Frame rectangle minimization

### H3: Lossy APNG Quantization (Future)
- Per-frame quantization with shared palette
- Background-aware Floyd-Steinberg dithering
- Inter-frame delta optimization

---

## 7. Benchmarking Plan

For comprehensive comparison with pngquant:

### Metrics
- File size (bytes) at quality levels 50, 65-75, 80, 90
- Processing time (wall clock, single file)
- SSIM / DSSIM visual quality score
- Color count in output palette
- Peak memory usage

### Dataset
- 10 diverse sample images (user-provided)
- Categories: photos, illustrations, gradients, transparency, text, icons
- Sizes: small (<100KB), medium (100KB-1MB), large (>1MB)

### Tools
- `pngoptim` vs `pngquant` side-by-side
- Visual comparison via browser or image viewer
- DSSIM tool for objective quality measurement
