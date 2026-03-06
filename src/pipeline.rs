use image::ImageFormat;
use std::borrow::Cow;
use std::fs;
use std::io::BufWriter;
use std::io::Cursor;
use std::path::Path;
use std::time::Instant;

use crate::cli::QualityRange;
use crate::error::AppError;
use crate::palette_quant::{
    IndexedImage, max_colors_from_quality_speed, quantize_indexed, quantizer_settings,
};
use crate::quality::{
    QualityMetrics, SpeedSettings, evaluate_quality_against_rgba, quality_to_mse,
};

#[derive(Debug, Clone)]
pub struct PipelineOptions {
    pub quality: Option<QualityRange>,
    pub speed: u8,
    pub dither: bool,
    pub posterize: Option<u8>,
    pub strip: bool,
    pub skip_if_larger: bool,
}

#[derive(Debug, Clone)]
pub struct PipelineResult {
    pub width: u32,
    pub height: u32,
    pub input_bytes: u64,
    pub output_bytes: u64,
    pub quality_score: u8,
    pub quality_mse: f64,
    pub png_data: Vec<u8>,
    pub metrics: PipelineMetrics,
}

#[derive(Debug, Clone, Copy)]
pub struct PipelineMetrics {
    pub decode_ms: f64,
    pub quantize_ms: f64,
    pub encode_ms: f64,
    pub total_ms: f64,
}

#[derive(Clone, Debug, Default)]
struct PreservedMetadata {
    source_gamma: Option<png::ScaledFloat>,
    source_chromaticities: Option<png::SourceChromaticities>,
    srgb: Option<png::SrgbRenderingIntent>,
    pixel_dims: Option<png::PixelDimensions>,
    icc_profile: Option<Vec<u8>>,
    exif_metadata: Option<Vec<u8>>,
    uncompressed_latin1_text: Vec<png::text_metadata::TEXtChunk>,
    compressed_latin1_text: Vec<png::text_metadata::ZTXtChunk>,
    utf8_text: Vec<png::text_metadata::ITXtChunk>,
}

pub fn process_png_file(
    input: &Path,
    options: PipelineOptions,
) -> Result<PipelineResult, AppError> {
    let input_bytes = fs::read(input).map_err(|e| AppError::io_with_path(input, e))?;
    process_png_bytes(&input_bytes, options)
}

pub fn process_png_bytes(
    input_bytes: &[u8],
    options: PipelineOptions,
) -> Result<PipelineResult, AppError> {
    let t_total = Instant::now();
    let metadata = if options.strip {
        None
    } else {
        extract_metadata(input_bytes)
    };

    let t_decode = Instant::now();
    let rgba = image::load_from_memory_with_format(input_bytes, ImageFormat::Png)
        .map_err(|e| AppError::Decode(format!("failed to decode PNG: {e}")))?
        .to_rgba8();
    let (width, height) = rgba.dimensions();
    let decode_ms = t_decode.elapsed().as_secs_f64() * 1000.0;

    let t_quantize = Instant::now();
    let speed_settings = SpeedSettings::from_speed(options.speed);
    let candidate = select_palette_candidate(
        rgba.as_raw(),
        width as usize,
        height as usize,
        options.quality.as_ref(),
        options.posterize.unwrap_or(0),
        speed_settings,
        options.dither,
    );
    let quantize_ms = t_quantize.elapsed().as_secs_f64() * 1000.0;

    if let Some(range) = options.quality.as_ref()
        && candidate.quality.quality_score < range.min
    {
        return Err(AppError::QualityTooLow {
            minimum: range.min,
            actual: candidate.quality.quality_score,
        });
    }

    let t_encode = Instant::now();
    let png_data = encode_indexed_png_to_vec(
        width,
        height,
        &candidate.indexed.indices,
        &candidate.indexed.palette,
        metadata.as_ref(),
        options.strip,
        options.speed,
    )?;
    let encode_ms = t_encode.elapsed().as_secs_f64() * 1000.0;
    if options.skip_if_larger && (png_data.len() as u64) > (input_bytes.len() as u64) {
        return Err(AppError::OutputLarger {
            input_bytes: input_bytes.len() as u64,
            output_bytes: png_data.len() as u64,
        });
    }
    let total_ms = t_total.elapsed().as_secs_f64() * 1000.0;

    Ok(PipelineResult {
        width,
        height,
        input_bytes: input_bytes.len() as u64,
        output_bytes: png_data.len() as u64,
        quality_score: candidate.quality.quality_score,
        quality_mse: candidate.quality.standard_mse,
        png_data,
        metrics: PipelineMetrics {
            decode_ms,
            quantize_ms,
            encode_ms,
            total_ms,
        },
    })
}

#[derive(Debug, Clone)]
struct QuantizeCandidate {
    indexed: IndexedImage,
    quality: QualityMetrics,
}

fn select_palette_candidate(
    rgba: &[u8],
    width: usize,
    height: usize,
    quality: Option<&QualityRange>,
    output_posterize_bits: u8,
    speed_settings: SpeedSettings,
    dither: bool,
) -> QuantizeCandidate {
    let target_mse = quality.map(|range| quality_to_mse(range.max));
    let evaluate = |max_colors: usize| {
        evaluate_candidate(
            rgba,
            width,
            height,
            max_colors,
            output_posterize_bits,
            speed_settings,
            target_mse,
            dither,
        )
    };

    let default_colors = quality
        .map(|range| max_colors_from_quality_speed(range.max, speed_settings.effective_speed))
        .unwrap_or(256);

    let mut high_quality = evaluate(default_colors);
    if high_quality.quality.quality_score < quality.map_or(0, |range| range.max)
        && default_colors < 256
    {
        high_quality = evaluate(256);
    }
    let Some(range) = quality else {
        return high_quality;
    };

    if high_quality.quality.quality_score < range.max {
        return high_quality;
    }

    let mut low = 2usize;
    let mut high = default_colors;
    let mut best_colors = default_colors;
    let mut best = high_quality;
    let mut budget = speed_settings.search_budget();

    while low <= high && budget > 0 {
        let mid = low + (high - low) / 2;
        let candidate = evaluate(mid);
        if candidate.quality.quality_score >= range.max {
            best_colors = mid;
            best = candidate;
            high = mid.saturating_sub(1);
        } else {
            low = mid.saturating_add(1);
        }
        budget -= 1;
    }

    for colors in best_colors.saturating_sub(4).max(2)..best_colors {
        let candidate = evaluate(colors);
        if candidate.quality.quality_score >= range.max {
            best = candidate;
        }
    }

    best
}

fn evaluate_candidate(
    rgba: &[u8],
    width: usize,
    height: usize,
    max_colors: usize,
    output_posterize_bits: u8,
    speed_settings: SpeedSettings,
    target_mse: Option<f64>,
    dither: bool,
) -> QuantizeCandidate {
    let mut best = evaluate_candidate_once(
        rgba,
        width,
        height,
        max_colors,
        output_posterize_bits,
        speed_settings,
        target_mse,
        false,
    );

    if dither {
        let dithered = evaluate_candidate_once(
            rgba,
            width,
            height,
            max_colors,
            output_posterize_bits,
            speed_settings,
            target_mse,
            true,
        );
        if dithered.quality.quality_score > best.quality.quality_score
            || (dithered.quality.quality_score == best.quality.quality_score
                && dithered.quality.standard_mse < best.quality.standard_mse)
        {
            best = dithered;
        }
    }

    best
}

fn evaluate_candidate_once(
    rgba: &[u8],
    width: usize,
    height: usize,
    max_colors: usize,
    output_posterize_bits: u8,
    speed_settings: SpeedSettings,
    target_mse: Option<f64>,
    dither: bool,
) -> QuantizeCandidate {
    let quantizer = quantizer_settings(max_colors, speed_settings, target_mse, dither);
    let mut indexed = quantize_indexed(rgba, width, height, quantizer);
    if output_posterize_bits > 0 {
        apply_posterize_palette(&mut indexed.palette, output_posterize_bits);
    }
    let remapped_rgba = remapped_rgba_from_indices(&indexed.indices, &indexed.palette);
    let quality = evaluate_quality_against_rgba(rgba, &remapped_rgba);
    QuantizeCandidate { indexed, quality }
}

fn remapped_rgba_from_indices(indices: &[u8], palette: &[[u8; 4]]) -> Vec<u8> {
    let mut out = Vec::with_capacity(indices.len() * 4);
    for &idx in indices {
        let px = palette[idx as usize];
        out.extend_from_slice(&px);
    }
    out
}

pub fn write_output_file(path: &Path, png_data: &[u8], force: bool) -> Result<(), AppError> {
    if path.exists() && !force {
        return Err(AppError::Arg(format!(
            "output already exists: {} (pass --force to overwrite)",
            path.display()
        )));
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| AppError::io_with_path(parent, e))?;
    }

    let file = fs::File::create(path).map_err(|e| AppError::io_with_path(path, e))?;
    let mut writer = BufWriter::new(file);
    std::io::Write::write_all(&mut writer, png_data)
        .map_err(|e| AppError::io_with_path(path, e))?;
    Ok(())
}

fn encode_indexed_png_to_vec(
    width: u32,
    height: u32,
    indices: &[u8],
    palette_rgba: &[[u8; 4]],
    metadata: Option<&PreservedMetadata>,
    strip: bool,
    speed: u8,
) -> Result<Vec<u8>, AppError> {
    let bit_depth = indexed_bit_depth(palette_rgba.len());
    let packed_indices = pack_indices_by_bit_depth(indices, width, height, bit_depth)?;
    let (compression, filters): (png::DeflateCompression, &[png::Filter]) = match speed {
        1..=2 => (
            png::DeflateCompression::Level(9),
            &[
                png::Filter::MinEntropy,
                png::Filter::Adaptive,
                png::Filter::Up,
                png::Filter::Sub,
                png::Filter::NoFilter,
            ],
        ),
        3..=4 => (
            png::DeflateCompression::Level(9),
            &[png::Filter::MinEntropy, png::Filter::Adaptive],
        ),
        5..=6 => (
            png::DeflateCompression::Level(7),
            &[png::Filter::Adaptive, png::Filter::Up],
        ),
        7..=8 => (png::DeflateCompression::Level(5), &[png::Filter::Adaptive]),
        9..=10 => (
            png::DeflateCompression::Level(3),
            &[png::Filter::Up, png::Filter::NoFilter],
        ),
        _ => (
            png::DeflateCompression::FdeflateUltraFast,
            &[png::Filter::Up],
        ),
    };

    let mut best: Option<Vec<u8>> = None;
    for filter in filters {
        let encoded = encode_indexed_png_with_filter(
            width,
            height,
            bit_depth,
            &packed_indices,
            palette_rgba,
            metadata,
            strip,
            *filter,
            compression,
        )?;
        if best
            .as_ref()
            .is_none_or(|existing| encoded.len() < existing.len())
        {
            best = Some(encoded);
        }
    }

    Ok(best.unwrap_or_default())
}

fn encode_indexed_png_with_filter(
    width: u32,
    height: u32,
    bit_depth: png::BitDepth,
    packed_indices: &[u8],
    palette_rgba: &[[u8; 4]],
    metadata: Option<&PreservedMetadata>,
    strip: bool,
    filter: png::Filter,
    compression: png::DeflateCompression,
) -> Result<Vec<u8>, AppError> {
    let mut info = png::Info::with_size(width, height);
    info.color_type = png::ColorType::Indexed;
    info.bit_depth = bit_depth;
    info.palette = Some(Cow::Owned(
        palette_rgba
            .iter()
            .flat_map(|v| [v[0], v[1], v[2]])
            .collect::<Vec<u8>>(),
    ));
    if let Some(last_non_opaque) = palette_rgba.iter().rposition(|v| v[3] < 255) {
        info.trns = Some(Cow::Owned(
            palette_rgba
                .iter()
                .take(last_non_opaque + 1)
                .map(|v| v[3])
                .collect::<Vec<u8>>(),
        ));
    }

    if !strip {
        if let Some(meta) = metadata {
            info.source_gamma = meta.source_gamma;
            info.source_chromaticities = meta.source_chromaticities;
            info.srgb = meta.srgb;
            info.pixel_dims = meta.pixel_dims;
            info.icc_profile = meta.icc_profile.as_ref().map(|v| Cow::Owned(v.clone()));
            info.exif_metadata = meta.exif_metadata.as_ref().map(|v| Cow::Owned(v.clone()));
            info.uncompressed_latin1_text = meta.uncompressed_latin1_text.clone();
            info.compressed_latin1_text = meta.compressed_latin1_text.clone();
            info.utf8_text = meta.utf8_text.clone();
        }
    }

    let mut out = Vec::new();
    let mut encoder = png::Encoder::with_info(&mut out, info)
        .map_err(|e| AppError::Encode(format!("failed to initialize PNG encoder: {e}")))?;
    encoder.set_deflate_compression(compression);
    encoder.set_filter(filter);
    let mut writer = encoder
        .write_header()
        .map_err(|e| AppError::Encode(format!("failed to write PNG header: {e}")))?;
    writer
        .write_image_data(packed_indices)
        .map_err(|e| AppError::Encode(format!("failed to write PNG image data: {e}")))?;
    drop(writer);
    Ok(out)
}

fn indexed_bit_depth(palette_len: usize) -> png::BitDepth {
    match palette_len {
        0..=2 => png::BitDepth::One,
        3..=4 => png::BitDepth::Two,
        5..=16 => png::BitDepth::Four,
        _ => png::BitDepth::Eight,
    }
}

fn pack_indices_by_bit_depth(
    indices: &[u8],
    width: u32,
    height: u32,
    bit_depth: png::BitDepth,
) -> Result<Vec<u8>, AppError> {
    let pixel_count = (width as usize).saturating_mul(height as usize);
    if indices.len() != pixel_count {
        return Err(AppError::Encode(format!(
            "indexed data length mismatch: expected={pixel_count}, actual={}",
            indices.len()
        )));
    }

    let bits_per_index = match bit_depth {
        png::BitDepth::One => 1usize,
        png::BitDepth::Two => 2usize,
        png::BitDepth::Four => 4usize,
        png::BitDepth::Eight => 8usize,
        png::BitDepth::Sixteen => {
            return Err(AppError::Encode(
                "indexed PNG does not support 16-bit palette indices".to_string(),
            ));
        }
    };

    if bits_per_index == 8 {
        return Ok(indices.to_vec());
    }

    let width_usize = width as usize;
    let max_index = ((1u16 << bits_per_index) - 1) as u8;
    let mut out = Vec::with_capacity((pixel_count * bits_per_index).div_ceil(8));

    for row in 0..height as usize {
        let row_start = row * width_usize;
        let row_end = row_start + width_usize;
        let row_pixels = &indices[row_start..row_end];
        let mut acc = 0u8;
        let mut used_bits = 0usize;

        for &idx in row_pixels {
            if idx > max_index {
                return Err(AppError::Encode(format!(
                    "palette index out of range for {bits_per_index}-bit mode: {idx}"
                )));
            }
            let shift = 8usize - used_bits - bits_per_index;
            acc |= idx << shift;
            used_bits += bits_per_index;
            if used_bits == 8 {
                out.push(acc);
                acc = 0;
                used_bits = 0;
            }
        }

        if used_bits > 0 {
            out.push(acc);
        }
    }

    Ok(out)
}

fn extract_metadata(input_bytes: &[u8]) -> Option<PreservedMetadata> {
    let decoder = png::Decoder::new(Cursor::new(input_bytes));
    let mut reader = decoder.read_info().ok()?;
    let out_size = reader.output_buffer_size()?;
    let mut buf = vec![0; out_size];
    let _ = reader.next_frame(&mut buf).ok()?;
    let info = reader.info();

    Some(PreservedMetadata {
        source_gamma: info.source_gamma,
        source_chromaticities: info.source_chromaticities,
        srgb: info.srgb,
        pixel_dims: info.pixel_dims,
        icc_profile: info.icc_profile.as_ref().map(|v| v.as_ref().to_vec()),
        exif_metadata: info.exif_metadata.as_ref().map(|v| v.as_ref().to_vec()),
        uncompressed_latin1_text: info.uncompressed_latin1_text.clone(),
        compressed_latin1_text: info.compressed_latin1_text.clone(),
        utf8_text: info.utf8_text.clone(),
    })
}

fn apply_posterize_palette(palette: &mut [[u8; 4]], bits: u8) {
    if bits == 0 {
        return;
    }
    if bits >= 8 {
        for px in palette {
            px[0] = 0;
            px[1] = 0;
            px[2] = 0;
            px[3] = 0;
        }
        return;
    }
    let shift = bits;
    for px in palette {
        px[0] = (px[0] >> shift) << shift;
        px[1] = (px[1] >> shift) << shift;
        px[2] = (px[2] >> shift) << shift;
        px[3] = (px[3] >> shift) << shift;
    }
}

#[cfg(test)]
mod tests {
    use super::{
        apply_posterize_palette, indexed_bit_depth, pack_indices_by_bit_depth,
        remapped_rgba_from_indices,
    };

    #[test]
    fn posterize_reduces_bits() {
        let mut palette = vec![[255u8, 127, 63, 31]];
        apply_posterize_palette(&mut palette, 2);
        assert_eq!(palette[0], [252, 124, 60, 28]);
    }

    #[test]
    fn bit_depth_selection_matches_palette_size() {
        assert_eq!(indexed_bit_depth(2), png::BitDepth::One);
        assert_eq!(indexed_bit_depth(4), png::BitDepth::Two);
        assert_eq!(indexed_bit_depth(16), png::BitDepth::Four);
        assert_eq!(indexed_bit_depth(17), png::BitDepth::Eight);
    }

    #[test]
    fn remapped_rgba_is_reconstructed_from_palette_indices() {
        let palette = vec![[1u8, 2, 3, 4], [5u8, 6, 7, 8]];
        let rgba = remapped_rgba_from_indices(&[1, 0], &palette);
        assert_eq!(rgba, vec![5, 6, 7, 8, 1, 2, 3, 4]);
    }

    #[test]
    fn pack_indices_2bit_row_aligned() {
        let indices = vec![0u8, 1, 2, 3, 3, 2, 1, 0];
        let packed = pack_indices_by_bit_depth(&indices, 4, 2, png::BitDepth::Two)
            .expect("pack 2-bit indices");
        assert_eq!(packed, vec![0b0001_1011, 0b1110_0100]);
    }

    #[test]
    fn pack_indices_1bit_with_row_padding() {
        let indices = vec![0u8, 1, 1, 0, 1, 0];
        let packed = pack_indices_by_bit_depth(&indices, 3, 2, png::BitDepth::One)
            .expect("pack 1-bit indices");
        assert_eq!(packed, vec![0b0110_0000, 0b0100_0000]);
    }
}
