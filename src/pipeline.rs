use image::ImageFormat;
use lcms2::{CIExyY, CIExyYTRIPLE, Intent, PixelFormat, Profile, ToneCurve, Transform};
use std::fs;
use std::io::BufWriter;
use std::io::Cursor;
use std::path::Path;
use std::time::Instant;

use crate::apng::{
    IndexedApngFrame, IndexedApngImage, cautious_frame_trim, decode_apng,
    detect_input_characteristics, encode_apng, encode_indexed_apng, fold_duplicate_frames,
    minimize_frame_rects_checked,
};
use crate::cli::{ApngMode, QualityRange};
use crate::error::AppError;
use crate::palette_quant::{
    IndexedImage, build_histogram_map, find_best_palette, finalize_histogram,
    merge_histogram_maps, quantize_indexed, quantizer_settings, remap_image,
    reposterize_histogram_map, sort_palette_entries,
};
use crate::quality::{
    InternalPixel, QualityMetrics, SRGB_OUTPUT_GAMMA, SpeedSettings, evaluate_quality_against_rgba,
    gamma_lut, quality_to_mse,
};

const DEFAULT_MAX_COLORS: usize = 256;

#[derive(Debug, Clone)]
pub struct PipelineOptions {
    pub quality: Option<QualityRange>,
    pub speed: u8,
    pub dither_level: f32,
    pub posterize: Option<u8>,
    pub strip: bool,
    pub skip_if_larger: bool,
    pub no_icc: bool,
    pub apng_mode: ApngMode,
}

impl Default for PipelineOptions {
    fn default() -> Self {
        Self {
            quality: None,
            speed: 4,
            dither_level: 1.0,
            posterize: None,
            strip: false,
            skip_if_larger: false,
            no_icc: false,
            apng_mode: ApngMode::Safe,
        }
    }
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
    // Try APNG detection first — route animated PNGs to the APNG pipeline
    match decode_apng(input_bytes) {
        Ok(Some(apng)) => return process_apng(input_bytes, apng, &options),
        Ok(None) => {} // static PNG, continue normal flow
        Err(_) => {}   // decode issue, fall through to static path
    }

    let t_total = Instant::now();
    let input_metadata = extract_metadata(input_bytes);
    let mut metadata = if options.strip {
        None
    } else {
        input_metadata.clone()
    };

    let t_decode = Instant::now();
    let mut rgba = image::load_from_memory_with_format(input_bytes, ImageFormat::Png)
        .map_err(|e| AppError::Decode(format!("failed to decode PNG: {e}")))?
        .to_rgba8();
    if !options.no_icc {
        normalize_rgba_to_srgb_if_needed(
            rgba.as_mut(),
            input_metadata.as_ref(),
            metadata.as_mut(),
        )?;
    }
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
        options.dither_level,
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

fn process_apng(
    input_bytes: &[u8],
    mut apng: crate::apng::ApngImage,
    options: &PipelineOptions,
) -> Result<PipelineResult, AppError> {
    let t_total = Instant::now();
    let width = apng.width;
    let height = apng.height;

    let t_decode = Instant::now();
    // Decode is already done (apng is passed in)
    let decode_ms = t_decode.elapsed().as_secs_f64() * 1000.0;

    let t_quantize = Instant::now();
    // Detect input characteristics to skip unnecessary re-encoding
    let input_info = detect_input_characteristics(input_bytes);

    if input_info.is_indexed && input_info.has_subrect_frames {
        // Already optimized indexed APNG with sub-rect frames — skip all optimizations,
        // only apply skip-if-larger as safety net
        let quantize_ms = t_quantize.elapsed().as_secs_f64() * 1000.0;
        let t_encode = Instant::now();
        let png_data = encode_apng(&apng)?;
        let encode_ms = t_encode.elapsed().as_secs_f64() * 1000.0;

        if options.skip_if_larger {
            let max_file_size = skip_if_larger_max_file_size(input_bytes.len() as u64, 100);
            if (png_data.len() as u64) > max_file_size {
                return Err(AppError::SkipIfLargerRejected {
                    input_bytes: input_bytes.len() as u64,
                    output_bytes: png_data.len() as u64,
                    maximum_file_size: max_file_size,
                    quality_score: 100,
                });
            }
        }

        let total_ms = t_total.elapsed().as_secs_f64() * 1000.0;
        return Ok(PipelineResult {
            width,
            height,
            input_bytes: input_bytes.len() as u64,
            output_bytes: png_data.len() as u64,
            quality_score: 100,
            quality_mse: 0.0,
            png_data,
            metrics: PipelineMetrics {
                decode_ms,
                quantize_ms,
                encode_ms,
                total_ms,
            },
        });
    }

    // H2 lossless optimizations
    fold_duplicate_frames(&mut apng);

    if options.apng_mode == ApngMode::Aggressive && !input_info.is_indexed {
        minimize_frame_rects_checked(&mut apng);
    } else if !input_info.is_indexed {
        // Safe mode: conservative trim only
        cautious_frame_trim(&mut apng);
    }

    // H3: lossy quantization with global shared palette
    let (indexed_apng, quality) = quantize_apng_frames(&apng, options)?;
    let quantize_ms = t_quantize.elapsed().as_secs_f64() * 1000.0;

    // Quality gating
    if let Some(range) = options.quality.as_ref()
        && quality.quality_score < range.min
    {
        return Err(AppError::QualityTooLow {
            minimum: range.min,
            actual: quality.quality_score,
        });
    }

    let t_encode = Instant::now();
    let png_data = encode_indexed_apng(&indexed_apng)?;
    let encode_ms = t_encode.elapsed().as_secs_f64() * 1000.0;

    // skip-if-larger: compare against original input
    if options.skip_if_larger {
        let max_file_size =
            skip_if_larger_max_file_size(input_bytes.len() as u64, quality.quality_score);
        if (png_data.len() as u64) > max_file_size {
            return Err(AppError::SkipIfLargerRejected {
                input_bytes: input_bytes.len() as u64,
                output_bytes: png_data.len() as u64,
                maximum_file_size: max_file_size,
                quality_score: quality.quality_score,
            });
        }
    }

    let total_ms = t_total.elapsed().as_secs_f64() * 1000.0;

    Ok(PipelineResult {
        width,
        height,
        input_bytes: input_bytes.len() as u64,
        output_bytes: png_data.len() as u64,
        quality_score: quality.quality_score,
        quality_mse: quality.standard_mse,
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

#[derive(Debug, Clone, Copy)]
struct QualityTargets {
    target_mse: f64,
    max_mse: Option<f64>,
    target_mse_is_zero: bool,
}

fn select_palette_candidate(
    rgba: &[u8],
    width: usize,
    height: usize,
    quality: Option<&QualityRange>,
    output_posterize_bits: u8,
    speed_settings: SpeedSettings,
    dither_level: f32,
) -> QuantizeCandidate {
    let targets = quality_targets(quality, output_posterize_bits);
    evaluate_candidate(
        rgba,
        width,
        height,
        DEFAULT_MAX_COLORS,
        output_posterize_bits,
        speed_settings,
        targets,
        dither_level,
    )
}

#[allow(clippy::too_many_arguments)]
fn evaluate_candidate(
    rgba: &[u8],
    width: usize,
    height: usize,
    max_colors: usize,
    output_posterize_bits: u8,
    speed_settings: SpeedSettings,
    quality_targets: QualityTargets,
    dither_level: f32,
) -> QuantizeCandidate {
    evaluate_candidate_once(
        rgba,
        width,
        height,
        max_colors,
        output_posterize_bits,
        speed_settings,
        quality_targets,
        dither_level,
    )
}

#[allow(clippy::too_many_arguments)]
fn evaluate_candidate_once(
    rgba: &[u8],
    width: usize,
    height: usize,
    max_colors: usize,
    output_posterize_bits: u8,
    speed_settings: SpeedSettings,
    quality_targets: QualityTargets,
    dither_level: f32,
) -> QuantizeCandidate {
    let quantizer = quantizer_settings(
        max_colors,
        speed_settings,
        quality_targets.target_mse,
        quality_targets.max_mse,
        quality_targets.target_mse_is_zero,
        output_posterize_bits,
        dither_level,
    );
    let indexed = quantize_indexed(rgba, width, height, quantizer);
    let remapped_rgba = remapped_rgba_from_indices(&indexed.indices, &indexed.palette);
    let quality = evaluate_quality_against_rgba(rgba, &remapped_rgba);
    QuantizeCandidate { indexed, quality }
}

fn quantize_apng_frames(
    apng: &crate::apng::ApngImage,
    options: &PipelineOptions,
) -> Result<(IndexedApngImage, QualityMetrics), AppError> {
    let speed_settings = SpeedSettings::from_speed(options.speed);
    let output_posterize_bits = options.posterize.unwrap_or(0);
    let targets = quality_targets(options.quality.as_ref(), output_posterize_bits);
    let quantizer = quantizer_settings(
        DEFAULT_MAX_COLORS,
        speed_settings,
        targets.target_mse,
        targets.max_mse,
        targets.target_mse_is_zero,
        output_posterize_bits,
        options.dither_level,
    );
    let gamma = gamma_lut(SRGB_OUTPUT_GAMMA);

    // Step 1: Build per-frame histograms and merge into a global one
    let mut global_map = build_histogram_map(&apng.frames[0].rgba, None);
    for frame in &apng.frames[1..] {
        let frame_map = build_histogram_map(&frame.rgba, None);
        merge_histogram_maps(&mut global_map, frame_map);
    }
    if let Some(default_image) = &apng.default_image {
        let default_map = build_histogram_map(&default_image.rgba, None);
        merge_histogram_maps(&mut global_map, default_map);
    }

    // Step 2: Reposterize if needed and finalize histogram
    let requested_bits = speed_settings.input_posterize_bits.min(3);
    if requested_bits > 0 {
        reposterize_histogram_map(&mut global_map, requested_bits);
    }
    if global_map.len() > speed_settings.max_histogram_entries as usize {
        let bits = requested_bits + 1;
        if bits <= 3 {
            reposterize_histogram_map(&mut global_map, bits);
        }
    }
    let histogram = finalize_histogram(global_map, &gamma);

    // Step 3: Find best palette and sort
    let (mut palette, palette_error) = find_best_palette(&histogram, quantizer);
    if palette.is_empty() {
        palette = vec![crate::palette_quant::PaletteEntry {
            color: InternalPixel::default(),
            popularity: 0.0,
        }];
    }
    sort_palette_entries(&mut palette);

    let global_palette: Vec<(InternalPixel, [u8; 4])> = palette
        .iter()
        .map(|entry| (entry.color, entry.color.to_rgba(SRGB_OUTPUT_GAMMA)))
        .collect();
    let global_rgba_palette: Vec<[u8; 4]> = global_palette.iter().map(|e| e.1).collect();

    // Step 4: Remap each frame independently using the global palette.
    // remap_image() may reorder the palette per-frame (by usage count),
    // so we must map per-frame indices back to the global palette order.
    let mut indexed_frames = Vec::with_capacity(apng.frames.len());
    let mut worst_quality = QualityMetrics {
        internal_mse: 0.0,
        standard_mse: 0.0,
        quality_score: 100,
    };

    for frame in &apng.frames {
        let fw = frame.width as usize;
        let fh = frame.height as usize;

        let indexed = remap_image(
            &frame.rgba,
            fw,
            fh,
            &global_palette,
            palette_error,
            quantizer,
            None,
            None,
            None,
        );

        // Map per-frame palette indices back to global palette indices.
        // remap_image returns a reordered palette subset; build a mapping
        // from per-frame index → global index by matching RGBA colors.
        let global_indices =
            remap_indices_to_global(&indexed.indices, &indexed.palette, &global_rgba_palette);

        // Quality evaluation
        let remapped_rgba = remapped_rgba_from_indices(&indexed.indices, &indexed.palette);
        let frame_quality = if fw > 0 && fh > 0 {
            evaluate_quality_against_rgba(&frame.rgba, &remapped_rgba)
        } else {
            QualityMetrics {
                internal_mse: 0.0,
                standard_mse: 0.0,
                quality_score: 100,
            }
        };
        if frame_quality.quality_score < worst_quality.quality_score {
            worst_quality = frame_quality;
        }

        indexed_frames.push(IndexedApngFrame {
            width: frame.width,
            height: frame.height,
            x_offset: frame.x_offset,
            y_offset: frame.y_offset,
            delay_num: frame.delay_num,
            delay_den: frame.delay_den,
            dispose_op: frame.dispose_op,
            blend_op: frame.blend_op,
            indices: global_indices,
        });
    }

    // Step 5: Remap default image if present
    let default_image_indices = if let Some(default_image) = &apng.default_image {
        let dw = apng.width as usize;
        let dh = apng.height as usize;
        let indexed = remap_image(
            &default_image.rgba,
            dw,
            dh,
            &global_palette,
            palette_error,
            quantizer,
            None,
            None,
            None,
        );
        Some(remap_indices_to_global(
            &indexed.indices,
            &indexed.palette,
            &global_rgba_palette,
        ))
    } else {
        None
    };

    let indexed_apng = IndexedApngImage {
        width: apng.width,
        height: apng.height,
        num_plays: apng.num_plays,
        palette: global_rgba_palette,
        default_image_indices,
        frames: indexed_frames,
    };

    Ok((indexed_apng, worst_quality))
}

/// Map per-frame palette indices back to global palette indices.
/// remap_image() reorders the palette per-frame by usage count, so each frame's
/// indices reference a different ordering. This function builds a mapping table
/// and translates all indices to reference the global palette.
fn remap_indices_to_global(
    indices: &[u8],
    frame_palette: &[[u8; 4]],
    global_palette: &[[u8; 4]],
) -> Vec<u8> {
    // Build mapping: frame palette index → global palette index
    let mapping: Vec<u8> = frame_palette
        .iter()
        .map(|frame_color| {
            // Find exact match first, fall back to nearest
            global_palette
                .iter()
                .position(|g| g == frame_color)
                .unwrap_or_else(|| {
                    // Nearest color (k-means may have slightly adjusted colors)
                    global_palette
                        .iter()
                        .enumerate()
                        .min_by_key(|(_, g)| {
                            let dr = i32::from(frame_color[0]) - i32::from(g[0]);
                            let dg = i32::from(frame_color[1]) - i32::from(g[1]);
                            let db = i32::from(frame_color[2]) - i32::from(g[2]);
                            let da = i32::from(frame_color[3]) - i32::from(g[3]);
                            dr * dr + dg * dg + db * db + da * da
                        })
                        .map(|(i, _)| i)
                        .unwrap_or(0)
                }) as u8
        })
        .collect();

    indices.iter().map(|&idx| mapping[idx as usize]).collect()
}

fn quality_targets(quality: Option<&QualityRange>, output_posterize_bits: u8) -> QualityTargets {
    let _ = output_posterize_bits;
    let max_mse = quality.map(|range| quality_to_mse(range.min));
    let target_mse_is_zero = quality.is_none();
    let target_mse = quality.map_or(0.0, |range| quality_to_mse(range.max));

    QualityTargets {
        target_mse,
        max_mse,
        target_mse_is_zero,
    }
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
    let compression_level = if speed >= 10 { 1 } else { 9 };

    if speed >= 10 {
        // Fast mode: single attempt with default mem_level
        return encode_indexed_png_raw(
            width,
            height,
            bit_depth,
            &packed_indices,
            palette_rgba,
            metadata,
            strip,
            compression_level,
            8,
        );
    }

    // Try both mem_level=5 and mem_level=8 in parallel, pick smaller output.
    // mem_level=5 often wins for small palettes / repetitive data,
    // mem_level=8 wins for larger images with more varied index patterns.
    let (out_ml5, out_ml8) = rayon::join(
        || {
            encode_indexed_png_raw(
                width,
                height,
                bit_depth,
                &packed_indices,
                palette_rgba,
                metadata,
                strip,
                compression_level,
                5,
            )
        },
        || {
            encode_indexed_png_raw(
                width,
                height,
                bit_depth,
                &packed_indices,
                palette_rgba,
                metadata,
                strip,
                compression_level,
                8,
            )
        },
    );
    let out_ml5 = out_ml5?;
    let out_ml8 = out_ml8?;

    Ok(if out_ml5.len() <= out_ml8.len() {
        out_ml5
    } else {
        out_ml8
    })
}

// ── Hand-written PNG encoder with zlib-rs (mem_level=5) ──

const PNG_SIGNATURE: [u8; 8] = [137, 80, 78, 71, 13, 10, 26, 10];

fn write_png_chunk(out: &mut Vec<u8>, chunk_type: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(chunk_type);
    out.extend_from_slice(data);
    let mut crc = crc32fast::Hasher::new();
    crc.update(chunk_type);
    crc.update(data);
    out.extend_from_slice(&crc.finalize().to_be_bytes());
}

fn encode_indexed_png_raw(
    width: u32,
    height: u32,
    bit_depth: png::BitDepth,
    packed_indices: &[u8],
    palette_rgba: &[[u8; 4]],
    metadata: Option<&PreservedMetadata>,
    strip: bool,
    compression_level: i32,
    mem_level: i32,
) -> Result<Vec<u8>, AppError> {
    #![allow(clippy::too_many_arguments)]
    let row_bytes = row_byte_count(width, bit_depth);
    if packed_indices.len() != row_bytes * height as usize {
        return Err(AppError::Encode(format!(
            "packed data length mismatch: expected={}, actual={}",
            row_bytes * height as usize,
            packed_indices.len()
        )));
    }

    // Build filtered row data (NoFilter: prepend 0x00 to each row)
    let filtered_len = (row_bytes + 1) * height as usize;
    let mut filtered = Vec::with_capacity(filtered_len);
    for row in packed_indices.chunks(row_bytes) {
        filtered.push(0u8); // NoFilter
        filtered.extend_from_slice(row);
    }

    let config = zlib_rs::DeflateConfig {
        level: compression_level,
        mem_level,
        ..zlib_rs::DeflateConfig::default()
    };
    let bound = zlib_rs::compress_bound(filtered.len());
    let mut compressed = vec![0u8; bound];
    let (compressed_data, rc) = zlib_rs::compress_slice(&mut compressed, &filtered, config);
    if rc != zlib_rs::ReturnCode::Ok {
        return Err(AppError::Encode(format!("zlib compression failed: {rc:?}")));
    }
    let compressed_len = compressed_data.len();

    // Estimate output size and allocate
    let est_size = 8 + 25 + 12 + palette_rgba.len() * 3 + 12 + compressed_len + 12 + 256;
    let mut out = Vec::with_capacity(est_size);

    // PNG signature
    out.extend_from_slice(&PNG_SIGNATURE);

    // IHDR
    let mut ihdr = [0u8; 13];
    ihdr[0..4].copy_from_slice(&width.to_be_bytes());
    ihdr[4..8].copy_from_slice(&height.to_be_bytes());
    ihdr[8] = bit_depth as u8;
    ihdr[9] = 3; // ColorType::Indexed
    // compression=0, filter=0, interlace=0
    write_png_chunk(&mut out, b"IHDR", &ihdr);

    // Metadata chunks (only if not stripped)
    if !strip && let Some(meta) = metadata {
        // pHYs
        if let Some(pd) = meta.pixel_dims {
            let mut phys = [0u8; 9];
            phys[0..4].copy_from_slice(&pd.xppu.to_be_bytes());
            phys[4..8].copy_from_slice(&pd.yppu.to_be_bytes());
            phys[8] = match pd.unit {
                png::Unit::Meter => 1,
                png::Unit::Unspecified => 0,
            };
            write_png_chunk(&mut out, b"pHYs", &phys);
        }

        // Color space: sRGB takes precedence, otherwise gAMA/cHRM/iCCP
        if let Some(srgb) = meta.srgb {
            // sRGB chunk (1 byte: rendering intent)
            write_png_chunk(&mut out, b"sRGB", &[srgb as u8]);
            // When sRGB is set, omit gAMA and cHRM — they're implied by sRGB
            // and pngquant does the same. This saves ~20 bytes.
        } else {
            if let Some(gamma) = meta.source_gamma {
                write_png_chunk(&mut out, b"gAMA", &gamma.into_value().to_be_bytes());
            }
            if let Some(chrm) = meta.source_chromaticities {
                let mut data = [0u8; 32];
                data[0..4].copy_from_slice(&chrm.white.0.into_value().to_be_bytes());
                data[4..8].copy_from_slice(&chrm.white.1.into_value().to_be_bytes());
                data[8..12].copy_from_slice(&chrm.red.0.into_value().to_be_bytes());
                data[12..16].copy_from_slice(&chrm.red.1.into_value().to_be_bytes());
                data[16..20].copy_from_slice(&chrm.green.0.into_value().to_be_bytes());
                data[20..24].copy_from_slice(&chrm.green.1.into_value().to_be_bytes());
                data[24..28].copy_from_slice(&chrm.blue.0.into_value().to_be_bytes());
                data[28..32].copy_from_slice(&chrm.blue.1.into_value().to_be_bytes());
                write_png_chunk(&mut out, b"cHRM", &data);
            }
            if let Some(icc) = &meta.icc_profile {
                // iCCP: profile_name + null + compression_method(0) + compressed_profile
                let name = b"_\0\0"; // name "_", null, compression method 0
                let mut iccp_data = Vec::with_capacity(name.len() + icc.len());
                iccp_data.extend_from_slice(name);
                let mut icc_compressed = vec![0u8; zlib_rs::compress_bound(icc.len())];
                let (icc_out, _) = zlib_rs::compress_slice(
                    &mut icc_compressed,
                    icc,
                    zlib_rs::DeflateConfig::default(),
                );
                iccp_data.extend_from_slice(icc_out);
                write_png_chunk(&mut out, b"iCCP", &iccp_data);
            }
        }

        // eXIf
        if let Some(exif) = &meta.exif_metadata {
            write_png_chunk(&mut out, b"eXIf", exif);
        }
    }

    // PLTE
    let plte_data: Vec<u8> = palette_rgba
        .iter()
        .flat_map(|v| [v[0], v[1], v[2]])
        .collect();
    write_png_chunk(&mut out, b"PLTE", &plte_data);

    // tRNS (only if any non-opaque entries)
    if let Some(last_non_opaque) = palette_rgba.iter().rposition(|v| v[3] < 255) {
        let trns: Vec<u8> = palette_rgba
            .iter()
            .take(last_non_opaque + 1)
            .map(|v| v[3])
            .collect();
        write_png_chunk(&mut out, b"tRNS", &trns);
    }

    // Text chunks (before IDAT, per PNG spec recommendation)
    if !strip && let Some(meta) = metadata {
        use png::text_metadata::EncodableTextChunk;
        let mut text_buf = Vec::new();
        for chunk in &meta.uncompressed_latin1_text {
            text_buf.clear();
            if chunk.encode(&mut text_buf).is_ok() {
                out.extend_from_slice(&text_buf);
            }
        }
        for chunk in &meta.compressed_latin1_text {
            text_buf.clear();
            if chunk.encode(&mut text_buf).is_ok() {
                out.extend_from_slice(&text_buf);
            }
        }
        for chunk in &meta.utf8_text {
            text_buf.clear();
            if chunk.encode(&mut text_buf).is_ok() {
                out.extend_from_slice(&text_buf);
            }
        }
    }

    // IDAT (split into max 2GB chunks per PNG spec, but typically one chunk suffices)
    const MAX_IDAT_LEN: usize = (u32::MAX >> 1) as usize;
    for chunk in compressed[..compressed_len].chunks(MAX_IDAT_LEN) {
        write_png_chunk(&mut out, b"IDAT", chunk);
    }

    // IEND
    write_png_chunk(&mut out, b"IEND", &[]);

    Ok(out)
}

fn row_byte_count(width: u32, bit_depth: png::BitDepth) -> usize {
    let bits_per_pixel = match bit_depth {
        png::BitDepth::One => 1usize,
        png::BitDepth::Two => 2,
        png::BitDepth::Four => 4,
        png::BitDepth::Eight => 8,
        png::BitDepth::Sixteen => 16,
    };
    (width as usize * bits_per_pixel).div_ceil(8)
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
    let reader = decoder.read_info().ok()?;
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

fn normalize_rgba_to_srgb_if_needed(
    rgba: &mut [u8],
    input_metadata: Option<&PreservedMetadata>,
    output_metadata: Option<&mut PreservedMetadata>,
) -> Result<(), AppError> {
    let Some(input_metadata) = input_metadata else {
        return Ok(());
    };

    if let Some(icc_profile) = input_metadata.icc_profile.as_deref() {
        let Ok(input_profile) = Profile::new_icc(icc_profile) else {
            return Ok(());
        };
        return normalize_rgba_with_profile(rgba, &input_profile, output_metadata);
    }

    let Some(source_gamma) = input_metadata.source_gamma else {
        return Ok(());
    };
    let Some(source_chromaticities) = input_metadata.source_chromaticities else {
        return Ok(());
    };
    if input_metadata.srgb.is_some() {
        return Ok(());
    }

    let gamma = f64::from(source_gamma.into_value());
    if !(gamma > 0.0 && gamma <= 1.0) {
        return Ok(());
    }

    let input_profile = build_rgb_profile_from_png_chromaticities(source_chromaticities, gamma)
        .ok_or_else(|| {
            AppError::Decode("failed to build RGB profile from PNG gAMA/cHRM metadata".to_string())
        })?;
    normalize_rgba_with_profile(rgba, &input_profile, output_metadata)
}

fn normalize_rgba_with_profile(
    rgba: &mut [u8],
    input_profile: &Profile,
    output_metadata: Option<&mut PreservedMetadata>,
) -> Result<(), AppError> {
    let srgb_profile = Profile::new_srgb();
    let Ok(transform) = Transform::<u8, u8>::new(
        input_profile,
        PixelFormat::RGBA_8,
        &srgb_profile,
        PixelFormat::RGBA_8,
        Intent::Perceptual,
    ) else {
        return Ok(());
    };
    transform.transform_in_place(rgba);

    if let Some(output_metadata) = output_metadata {
        output_metadata.source_gamma = None;
        output_metadata.source_chromaticities = None;
        output_metadata.srgb = Some(png::SrgbRenderingIntent::Perceptual);
        output_metadata.icc_profile = None;
    }
    Ok(())
}

fn build_rgb_profile_from_png_chromaticities(
    chroma: png::SourceChromaticities,
    gamma: f64,
) -> Option<Profile> {
    let white_point = CIExyY {
        x: f64::from(chroma.white.0.into_value()),
        y: f64::from(chroma.white.1.into_value()),
        Y: 1.0,
    };
    let primaries = CIExyYTRIPLE {
        Red: CIExyY {
            x: f64::from(chroma.red.0.into_value()),
            y: f64::from(chroma.red.1.into_value()),
            Y: 1.0,
        },
        Green: CIExyY {
            x: f64::from(chroma.green.0.into_value()),
            y: f64::from(chroma.green.1.into_value()),
            Y: 1.0,
        },
        Blue: CIExyY {
            x: f64::from(chroma.blue.0.into_value()),
            y: f64::from(chroma.blue.1.into_value()),
            Y: 1.0,
        },
    };
    let curve = ToneCurve::new(1.0 / gamma);
    Profile::new_rgb(&white_point, &primaries, &[&curve, &curve, &curve]).ok()
}

#[cfg(test)]
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
    use lcms2::Profile;

    use super::{
        PreservedMetadata, apply_posterize_palette, indexed_bit_depth,
        normalize_rgba_to_srgb_if_needed, pack_indices_by_bit_depth, remapped_rgba_from_indices,
        skip_if_larger_max_file_size,
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

    #[test]
    fn skip_if_larger_requires_at_least_one_byte_of_savings_at_high_quality() {
        assert_eq!(skip_if_larger_max_file_size(1_000, 100), 999);
    }

    #[test]
    fn skip_if_larger_demands_stronger_savings_at_low_quality() {
        assert_eq!(skip_if_larger_max_file_size(1_000, 10), 499);
        assert_eq!(skip_if_larger_max_file_size(1_000, 75), 648);
    }

    #[test]
    fn invalid_icc_normalization_keeps_pixels_and_metadata_unchanged() {
        let icc_profile = vec![1u8, 2, 3, 4];
        let input = PreservedMetadata {
            icc_profile: Some(icc_profile),
            ..PreservedMetadata::default()
        };
        let mut output = input.clone();
        let original_rgba = vec![10u8, 20, 30, 255, 200, 210, 220, 255];
        let mut rgba = original_rgba.clone();

        normalize_rgba_to_srgb_if_needed(&mut rgba, Some(&input), Some(&mut output))
            .expect("normalize ICC");

        assert_eq!(rgba, original_rgba);
        assert_eq!(output.icc_profile, input.icc_profile);
        assert_eq!(output.srgb, input.srgb);
        assert_eq!(output.source_gamma, input.source_gamma);
        assert_eq!(output.source_chromaticities, input.source_chromaticities);
    }

    #[test]
    fn valid_icc_normalization_converts_metadata_to_srgb() {
        let icc_profile = Profile::new_srgb().icc().expect("serialize sRGB ICC");
        let input = PreservedMetadata {
            icc_profile: Some(icc_profile),
            ..PreservedMetadata::default()
        };
        let mut output = input.clone();
        let original_rgba = vec![10u8, 20, 30, 255, 200, 210, 220, 255];
        let mut rgba = original_rgba.clone();

        normalize_rgba_to_srgb_if_needed(&mut rgba, Some(&input), Some(&mut output))
            .expect("normalize valid ICC");

        assert_eq!(rgba, original_rgba);
        assert_eq!(output.icc_profile, None);
        assert_eq!(output.srgb, Some(png::SrgbRenderingIntent::Perceptual));
        assert_eq!(output.source_gamma, None);
        assert_eq!(output.source_chromaticities, None);
    }
}
