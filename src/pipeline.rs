use image::ImageFormat;
use std::borrow::Cow;
use std::fs;
use std::io::BufWriter;
use std::io::Cursor;
use std::path::Path;
use std::time::Instant;

use crate::cli::QualityRange;
use crate::error::AppError;
use crate::palette_quant::{IndexedImage, quantize_indexed, quantizer_settings};
use crate::quality::{
    QualityMetrics, SpeedSettings, evaluate_quality_against_rgba, quality_to_mse,
};

const DEFAULT_MAX_COLORS: usize = 256;

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
    if options.skip_if_larger {
        let max_file_size =
            skip_if_larger_max_file_size(input_bytes.len() as u64, candidate.quality.quality_score);
        if (png_data.len() as u64) > max_file_size {
            return Err(AppError::SkipIfLargerRejected {
                input_bytes: input_bytes.len() as u64,
                output_bytes: png_data.len() as u64,
                maximum_file_size: max_file_size,
                quality_score: candidate.quality.quality_score,
            });
        }
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
    estimated_output_bytes: Option<usize>,
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
    evaluate_candidate(
        rgba,
        width,
        height,
        DEFAULT_MAX_COLORS,
        output_posterize_bits,
        speed_settings,
        target_mse,
        dither,
    )
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
        let mut dithered = evaluate_candidate_once(
            rgba,
            width,
            height,
            max_colors,
            output_posterize_bits,
            speed_settings,
            target_mse,
            true,
        );
        if should_prefer_candidate(
            &mut best,
            &mut dithered,
            width as u32,
            height as u32,
            speed_settings.raw_speed,
        ) {
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
    QuantizeCandidate {
        indexed,
        quality,
        estimated_output_bytes: None,
    }
}

fn should_prefer_candidate(
    best: &mut QuantizeCandidate,
    challenger: &mut QuantizeCandidate,
    width: u32,
    height: u32,
    speed: u8,
) -> bool {
    if challenger.quality.quality_score != best.quality.quality_score {
        return challenger.quality.quality_score > best.quality.quality_score;
    }

    let best_mse = best.quality.standard_mse;
    let challenger_mse = challenger.quality.standard_mse;
    let mse_tolerance = equal_score_mse_tolerance(best_mse, challenger_mse);

    if challenger_mse + mse_tolerance < best_mse {
        return true;
    }

    if (challenger_mse - best_mse).abs() <= mse_tolerance {
        let best_size = estimate_output_bytes(best, width, height, speed);
        let challenger_size = estimate_output_bytes(challenger, width, height, speed);
        if challenger_size != best_size {
            return challenger_size < best_size;
        }
    }

    challenger_mse < best_mse
}

fn equal_score_mse_tolerance(left: f64, right: f64) -> f64 {
    left.max(right) * 0.02 + 0.05
}

fn estimate_output_bytes(
    candidate: &mut QuantizeCandidate,
    width: u32,
    height: u32,
    speed: u8,
) -> usize {
    if let Some(bytes) = candidate.estimated_output_bytes {
        return bytes;
    }

    let bytes = encode_indexed_png_to_vec(
        width,
        height,
        &candidate.indexed.indices,
        &candidate.indexed.palette,
        None,
        true,
        speed,
    )
    .map(|png| png.len())
    .unwrap_or(usize::MAX);
    candidate.estimated_output_bytes = Some(bytes);
    bytes
}

fn remapped_rgba_from_indices(indices: &[u8], palette: &[[u8; 4]]) -> Vec<u8> {
    let mut out = Vec::with_capacity(indices.len() * 4);
    for &idx in indices {
        let px = palette[idx as usize];
        out.extend_from_slice(&px);
    }
    out
}

fn skip_if_larger_max_file_size(input_bytes: u64, quality_score: u8) -> u64 {
    if input_bytes == 0 {
        return 0;
    }

    let quality = f64::from(quality_score) / 100.0;
    let expected_reduced_size = quality.powf(1.5).max(0.5);
    ((input_bytes.saturating_sub(1)) as f64 * expected_reduced_size).floor() as u64
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
        QuantizeCandidate, apply_posterize_palette, equal_score_mse_tolerance, indexed_bit_depth,
        pack_indices_by_bit_depth, remapped_rgba_from_indices, should_prefer_candidate,
        skip_if_larger_max_file_size,
    };
    use crate::palette_quant::IndexedImage;
    use crate::quality::QualityMetrics;

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

    #[test]
    fn equal_score_prefers_smaller_candidate_when_mse_is_close() {
        let mut best = QuantizeCandidate {
            indexed: IndexedImage {
                palette: vec![[0, 0, 0, 255]],
                indices: vec![0],
            },
            quality: QualityMetrics {
                internal_mse: 0.0,
                standard_mse: 9.20,
                quality_score: 70,
            },
            estimated_output_bytes: Some(328_647),
        };
        let mut challenger = QuantizeCandidate {
            indexed: IndexedImage {
                palette: vec![[0, 0, 0, 255]],
                indices: vec![0],
            },
            quality: QualityMetrics {
                internal_mse: 0.0,
                standard_mse: 9.22,
                quality_score: 70,
            },
            estimated_output_bytes: Some(328_049),
        };

        assert!(should_prefer_candidate(&mut best, &mut challenger, 1, 1, 4));
    }

    #[test]
    fn equal_score_prefers_lower_mse_when_gap_is_large() {
        let mut best = QuantizeCandidate {
            indexed: IndexedImage {
                palette: vec![[0, 0, 0, 255]],
                indices: vec![0],
            },
            quality: QualityMetrics {
                internal_mse: 0.0,
                standard_mse: 5.0,
                quality_score: 80,
            },
            estimated_output_bytes: Some(100),
        };
        let mut challenger = QuantizeCandidate {
            indexed: IndexedImage {
                palette: vec![[0, 0, 0, 255]],
                indices: vec![0],
            },
            quality: QualityMetrics {
                internal_mse: 0.0,
                standard_mse: 5.5,
                quality_score: 80,
            },
            estimated_output_bytes: Some(80),
        };

        assert!(!should_prefer_candidate(
            &mut best,
            &mut challenger,
            1,
            1,
            4
        ));
        assert!(equal_score_mse_tolerance(5.0, 5.5) < 0.6);
    }

    #[test]
    fn skip_if_larger_requires_at_least_one_byte_of_savings_at_high_quality() {
        assert_eq!(skip_if_larger_max_file_size(1_000, 100), 999);
    }

    #[test]
    fn skip_if_larger_demands_stronger_savings_at_low_quality() {
        assert_eq!(skip_if_larger_max_file_size(1_000, 10), 499);
        assert_eq!(skip_if_larger_max_file_size(1_000, 75), 648);
    }
}
