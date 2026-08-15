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
    IndexedImage, build_histogram_map, finalize_histogram, find_best_palette, merge_histogram_maps,
    quantize_indexed, quantizer_settings, remap_to_fixed_palette, reposterize_histogram_map,
    sort_palette_entries,
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
    /// Coding-independent code points (cICP): primaries/transfer/matrix/range.
    /// Not covered by the ICC/gAMA/cHRM normalization path below — cICP is an
    /// independent, coexisting signal (e.g. HDR PQ/HLG transfer characteristics)
    /// that must be preserved verbatim regardless of what happens to ICC/gAMA/cHRM.
    cicp: Option<png::CodingIndependentCodePoints>,
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
    // HDR guard: a cICP chunk with a PQ (16) or HLG (18) transfer function marks the
    // pixel data as HDR. `normalize_rgba_to_srgb_if_needed` assumes an SDR tone
    // response (it builds an ICC transform straight to sRGB via lcms2), so running
    // it on HDR samples would silently corrupt them — clipping/misinterpreting the
    // PQ/HLG-encoded values as if they were gamma-encoded SDR. We skip normalization
    // whenever cICP says the source is HDR, independent of `--no-icc`, and leave the
    // original iCCP/gAMA/cHRM/cICP chunks untouched so the pixel data and its
    // declared color metadata stay consistent. This is a defensive guard for direct
    // users of pngoptim; Zipic's own pipeline already tone-maps HDR sources upstream
    // before handing pixels to pngoptim (see P0), so no quantization math changes.
    //
    // When cICP is present but declares an SDR transfer function (e.g. sRGB=13,
    // BT.709=1), normalization still runs as before (existing behavior), and cICP is
    // still passed through unchanged in `metadata` below. Note this is technically
    // imprecise if the source's cICP primaries differ from sRGB (rare in practice for
    // SDR PNGs): the pixels get converted to the sRGB gamut but the passed-through
    // cICP chunk keeps describing the original primaries. Fixing that fully would
    // require rewriting cICP to sRGB's code points (1/13/0/1) post-normalization,
    // which is out of scope here — flagged as a TODO.
    let is_hdr_cicp = input_metadata
        .as_ref()
        .and_then(|m| m.cicp)
        .is_some_and(|c| matches!(c.transfer_function, 16 | 18));
    if !options.no_icc && !is_hdr_cicp {
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

    // Step 3: Find best palette and sort.
    // If any frame uses Over blend, reserve one transparent slot up front so
    // background-aware dithering can map unchanged pixels to transparency
    // without evicting a real color after quantization.
    let reserve_transparent = apng
        .frames
        .iter()
        .any(|frame| frame.blend_op == png::BlendOp::Over)
        && quantizer.max_colors > 2;
    let palette_settings = if reserve_transparent {
        crate::palette_quant::QuantizerSettings {
            max_colors: quantizer.max_colors - 1,
            ..quantizer
        }
    } else {
        quantizer
    };
    let (mut palette, _palette_error) = find_best_palette(&histogram, palette_settings);
    if palette.is_empty() {
        palette = vec![crate::palette_quant::PaletteEntry {
            color: InternalPixel::default(),
            popularity: 0.0,
        }];
    }
    sort_palette_entries(&mut palette);

    // Append the reserved transparent entry after sorting so remap can locate it
    // without sacrificing a real palette color.
    if reserve_transparent && !palette.iter().any(|e| e.color.is_fully_transparent()) {
        palette.push(crate::palette_quant::PaletteEntry {
            color: InternalPixel::default(),
            popularity: 0.0,
        });
    }

    let global_palette: Vec<(InternalPixel, [u8; 4])> = palette
        .iter()
        .map(|entry| (entry.color, entry.color.to_rgba(SRGB_OUTPUT_GAMMA)))
        .collect();
    let global_rgba_palette: Vec<[u8; 4]> = global_palette.iter().map(|e| e.1).collect();

    // Step 4: Remap each frame independently using the fixed global palette.
    // Uses remap_to_fixed_palette() which does NOT run k-means feedback or
    // reorder the palette, so indices directly reference the global palette.
    //
    // Background-aware dithering: maintain a canvas tracking the screen state before
    // each frame is blended. Pixels matching the background are mapped to transparent,
    // eliminating pixel churn and improving frame differencing in GIF/APNG.
    let mut indexed_frames = Vec::with_capacity(apng.frames.len());
    let mut worst_quality = QualityMetrics {
        internal_mse: 0.0,
        standard_mse: 0.0,
        quality_score: 100,
    };

    let canvas_len = crate::apng::rgba_len(apng.width, apng.height)?;
    let mut bg_canvas = vec![0u8; canvas_len];
    let mut saved_before_previous: Option<Vec<u8>> = None;

    for (i, frame) in apng.frames.iter().enumerate() {
        let fw = frame.width as usize;
        let fh = frame.height as usize;

        // Apply previous frame's disposal to get screen state before current frame
        if i > 0 {
            let prev = &apng.frames[i - 1];
            match crate::apng::effective_dispose(prev, i - 1) {
                png::DisposeOp::None => {}
                png::DisposeOp::Background => {
                    crate::apng::clear_region(
                        &mut bg_canvas,
                        apng.width,
                        prev.x_offset,
                        prev.y_offset,
                        prev.width,
                        prev.height,
                    )?;
                }
                png::DisposeOp::Previous => {
                    if let Some(saved) = saved_before_previous.take() {
                        bg_canvas = saved;
                    } else {
                        crate::apng::clear_region(
                            &mut bg_canvas,
                            apng.width,
                            prev.x_offset,
                            prev.y_offset,
                            prev.width,
                            prev.height,
                        )?;
                    }
                }
            }
        }

        // bg_canvas is the screen state before current frame is blended.
        // Extract the sub-rect matching this frame's region as the background.
        let bg_subrect = crate::apng::extract_subrect_rgba(
            &bg_canvas,
            apng.width,
            frame.x_offset,
            frame.y_offset,
            frame.width,
            frame.height,
        );
        let bg_pixels: Vec<InternalPixel> = bg_subrect
            .chunks_exact(4)
            .map(|px| InternalPixel::from_rgba(&gamma, px))
            .collect();

        // Only pass background for Over-blend frames. With Source blend,
        // transparent pixels mean "clear to transparent" (not "keep background"),
        // so background-aware dithering would break the composited output.
        let bg_ref = if frame.blend_op == png::BlendOp::Over {
            Some(bg_pixels.as_slice())
        } else {
            None
        };

        let indices =
            remap_to_fixed_palette(&frame.rgba, fw, fh, &global_palette, quantizer, bg_ref);

        // Quality evaluation using global palette (matches actual output)
        let remapped_rgba = remapped_rgba_from_indices(&indices, &global_rgba_palette);
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

        // Save canvas state before blending (for DisposeOp::Previous)
        if crate::apng::effective_dispose(frame, i) == png::DisposeOp::Previous {
            saved_before_previous = Some(bg_canvas.clone());
        } else {
            saved_before_previous = None;
        }
        // Blend original (pre-quantization) frame onto canvas for next iteration
        crate::apng::blend_frame(&mut bg_canvas, apng.width, frame)?;

        indexed_frames.push(IndexedApngFrame {
            width: frame.width,
            height: frame.height,
            x_offset: frame.x_offset,
            y_offset: frame.y_offset,
            delay_num: frame.delay_num,
            delay_den: frame.delay_den,
            dispose_op: frame.dispose_op,
            blend_op: frame.blend_op,
            indices,
        });
    }

    // Step 5: Remap default image if present
    let default_image_indices = if let Some(default_image) = &apng.default_image {
        let dw = apng.width as usize;
        let dh = apng.height as usize;
        Some(remap_to_fixed_palette(
            &default_image.rgba,
            dw,
            dh,
            &global_palette,
            quantizer,
            None,
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
        color_metadata: apng.color_metadata.clone(),
    };

    Ok((indexed_apng, worst_quality))
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

pub(crate) fn write_png_chunk(out: &mut Vec<u8>, chunk_type: &[u8; 4], data: &[u8]) {
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
        // cICP — must precede PLTE and IDAT (spec-enforced by decoders, including
        // the `png` crate we use for reading). Written first among metadata chunks
        // so a future chunk insertion never accidentally lands it after PLTE.
        if let Some(cicp) = meta.cicp {
            let data = [
                cicp.color_primaries,
                cicp.transfer_function,
                cicp.matrix_coefficients,
                cicp.is_video_full_range_image as u8,
            ];
            write_png_chunk(&mut out, b"cICP", &data);
        }

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
                // PNG spec: gAMA is a 4-byte big-endian *unsigned integer*, the gamma
                // value scaled by 100000 (`ScaledFloat::into_scaled`) — not the raw
                // IEEE754 float bit pattern (`into_value().to_be_bytes()`, which was
                // the bug here: dead code until gAMA/cHRM passthrough started working,
                // since `source_gamma`/`source_chromaticities` used to always be
                // `None` after a decode).
                write_png_chunk(&mut out, b"gAMA", &gamma.into_scaled().to_be_bytes());
            }
            if let Some(chrm) = meta.source_chromaticities {
                let mut data = [0u8; 32];
                data[0..4].copy_from_slice(&chrm.white.0.into_scaled().to_be_bytes());
                data[4..8].copy_from_slice(&chrm.white.1.into_scaled().to_be_bytes());
                data[8..12].copy_from_slice(&chrm.red.0.into_scaled().to_be_bytes());
                data[12..16].copy_from_slice(&chrm.red.1.into_scaled().to_be_bytes());
                data[16..20].copy_from_slice(&chrm.green.0.into_scaled().to_be_bytes());
                data[20..24].copy_from_slice(&chrm.green.1.into_scaled().to_be_bytes());
                data[24..28].copy_from_slice(&chrm.blue.0.into_scaled().to_be_bytes());
                data[28..32].copy_from_slice(&chrm.blue.1.into_scaled().to_be_bytes());
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
        // `Info::source_gamma`/`source_chromaticities` are write-only fields the `png`
        // crate's *encoder* uses to serialize gAMA/cHRM — the decoder never populates
        // them (confirmed against png 0.18.1's decoder). On decode, the parsed chunk
        // values live in `gama_chunk`/`chrm_chunk` instead; we read those here and
        // feed them back into the encoder-facing `source_gamma`/`source_chromaticities`
        // fields on write, mirroring `apng::decode_apng`.
        source_gamma: info.gama_chunk,
        source_chromaticities: info.chrm_chunk,
        srgb: info.srgb,
        pixel_dims: info.pixel_dims,
        icc_profile: info.icc_profile.as_ref().map(|v| v.as_ref().to_vec()),
        cicp: info.coding_independent_code_points,
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
        PipelineOptions, PreservedMetadata, apply_posterize_palette, indexed_bit_depth,
        normalize_rgba_to_srgb_if_needed, pack_indices_by_bit_depth, process_png_bytes,
        remapped_rgba_from_indices, skip_if_larger_max_file_size,
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

    /// Builds a minimal RGBA PNG carrying an optional cICP chunk and/or ICC profile,
    /// for exercising the static-PNG cICP passthrough / HDR-guard path end to end.
    /// RGBA has no PLTE chunk, so writing cICP right after `write_header()` (before
    /// `write_image_data`) is a valid position (still before IDAT).
    fn encode_test_rgba_png(
        width: u32,
        height: u32,
        rgba: &[u8],
        icc_profile: Option<Vec<u8>>,
        cicp: Option<png::CodingIndependentCodePoints>,
    ) -> Vec<u8> {
        encode_test_rgba_png_with_gama_chrm(width, height, rgba, icc_profile, cicp, None, None)
    }

    /// Same as [`encode_test_rgba_png`] but also allows setting gAMA/cHRM (as they
    /// would be parsed from a real-world PNG without an sRGB chunk or ICC profile).
    #[allow(clippy::too_many_arguments)]
    fn encode_test_rgba_png_with_gama_chrm(
        width: u32,
        height: u32,
        rgba: &[u8],
        icc_profile: Option<Vec<u8>>,
        cicp: Option<png::CodingIndependentCodePoints>,
        gamma: Option<png::ScaledFloat>,
        chromaticities: Option<png::SourceChromaticities>,
    ) -> Vec<u8> {
        let mut out = Vec::new();
        let mut info = png::Info::default();
        info.width = width;
        info.height = height;
        info.color_type = png::ColorType::Rgba;
        info.bit_depth = png::BitDepth::Eight;
        if let Some(icc) = icc_profile {
            info.icc_profile = Some(std::borrow::Cow::Owned(icc));
        }
        if let Some(gamma) = gamma {
            info.source_gamma = Some(gamma);
        }
        if let Some(chromaticities) = chromaticities {
            info.source_chromaticities = Some(chromaticities);
        }
        let encoder = png::Encoder::with_info(&mut out, info).expect("with_info");
        let mut writer = encoder.write_header().expect("write header");
        if let Some(cicp) = cicp {
            let data = [
                cicp.color_primaries,
                cicp.transfer_function,
                cicp.matrix_coefficients,
                cicp.is_video_full_range_image as u8,
            ];
            writer
                .write_chunk(png::chunk::cICP, &data)
                .expect("write cICP chunk");
        }
        writer.write_image_data(rgba).expect("write image data");
        drop(writer);
        out
    }

    /// Parse the top-level chunk type sequence of an encoded PNG, for asserting
    /// chunk ordering (e.g. "cICP comes before PLTE").
    fn chunk_types(png_bytes: &[u8]) -> Vec<[u8; 4]> {
        let mut types = Vec::new();
        let mut pos = 8usize; // skip signature
        while pos + 8 <= png_bytes.len() {
            let len = u32::from_be_bytes(png_bytes[pos..pos + 4].try_into().unwrap()) as usize;
            let mut t = [0u8; 4];
            t.copy_from_slice(&png_bytes[pos + 4..pos + 8]);
            let is_iend = &t == b"IEND";
            types.push(t);
            pos += 8 + len + 4;
            if is_iend {
                break;
            }
        }
        types
    }

    fn hdr_pq_cicp() -> png::CodingIndependentCodePoints {
        png::CodingIndependentCodePoints {
            color_primaries: 9,     // BT.2020
            transfer_function: 16,  // PQ (SMPTE ST 2084)
            matrix_coefficients: 0, // RGB
            is_video_full_range_image: true,
        }
    }

    fn sdr_srgb_cicp() -> png::CodingIndependentCodePoints {
        png::CodingIndependentCodePoints {
            color_primaries: 1,     // BT.709
            transfer_function: 13,  // sRGB
            matrix_coefficients: 0, // RGB
            is_video_full_range_image: true,
        }
    }

    #[test]
    fn cicp_round_trips_through_static_pipeline_and_precedes_plte() {
        let rgba = vec![
            10u8, 20, 30, 255, // px0
            200, 210, 220, 255, // px1
            5, 6, 7, 255, // px2
            250, 240, 230, 255, // px3
        ];
        let input = encode_test_rgba_png(2, 2, &rgba, None, Some(hdr_pq_cicp()));

        let result = process_png_bytes(&input, PipelineOptions::default()).expect("process PNG");

        let types = chunk_types(&result.png_data);
        let cicp_pos = types
            .iter()
            .position(|t| t == b"cICP")
            .expect("cICP present in output");
        let plte_pos = types
            .iter()
            .position(|t| t == b"PLTE")
            .expect("PLTE present in output");
        assert!(cicp_pos < plte_pos, "cICP must precede PLTE: {types:?}");

        // Verify the cICP payload survived byte-for-byte by decoding the output.
        let decoder = png::Decoder::new(std::io::Cursor::new(&result.png_data));
        let reader = decoder.read_info().expect("read output info");
        assert_eq!(
            reader.info().coding_independent_code_points,
            Some(hdr_pq_cicp())
        );
    }

    #[test]
    fn stripped_output_omits_cicp() {
        let rgba = vec![10u8, 20, 30, 255, 200, 210, 220, 255, 5, 6, 7, 255, 250, 240, 230, 255];
        let input = encode_test_rgba_png(2, 2, &rgba, None, Some(hdr_pq_cicp()));

        let result = process_png_bytes(
            &input,
            PipelineOptions {
                strip: true,
                ..PipelineOptions::default()
            },
        )
        .expect("process PNG");

        assert!(!chunk_types(&result.png_data).contains(b"cICP"));
    }

    #[test]
    fn pq_cicp_skips_srgb_normalization_and_keeps_iccp() {
        let rgba = vec![10u8, 20, 30, 255, 200, 210, 220, 255, 5, 6, 7, 255, 250, 240, 230, 255];
        let icc = Profile::new_srgb().icc().expect("serialize sRGB ICC");
        let input = encode_test_rgba_png(2, 2, &rgba, Some(icc), Some(hdr_pq_cicp()));

        let result = process_png_bytes(&input, PipelineOptions::default()).expect("process PNG");

        let types = chunk_types(&result.png_data);
        assert!(
            types.contains(b"iCCP"),
            "HDR (PQ cICP) input must keep its iCCP chunk untouched: {types:?}"
        );
        assert!(
            !types.contains(b"sRGB"),
            "HDR (PQ cICP) input must not be normalized into an sRGB chunk: {types:?}"
        );

        // Same check with --no-icc: behavior must be identical (guard is unconditional).
        let result_no_icc = process_png_bytes(
            &input,
            PipelineOptions {
                no_icc: true,
                ..PipelineOptions::default()
            },
        )
        .expect("process PNG with --no-icc");
        let types_no_icc = chunk_types(&result_no_icc.png_data);
        assert!(types_no_icc.contains(b"iCCP"));
        assert!(!types_no_icc.contains(b"sRGB"));
    }

    #[test]
    fn sdr_cicp_still_normalizes_icc_but_keeps_cicp_passthrough() {
        let rgba = vec![10u8, 20, 30, 255, 200, 210, 220, 255, 5, 6, 7, 255, 250, 240, 230, 255];
        let icc = Profile::new_srgb().icc().expect("serialize sRGB ICC");
        let input = encode_test_rgba_png(2, 2, &rgba, Some(icc), Some(sdr_srgb_cicp()));

        let result = process_png_bytes(&input, PipelineOptions::default()).expect("process PNG");

        let types = chunk_types(&result.png_data);
        // SDR cICP: normalization still runs as before (iCCP -> sRGB chunk)...
        assert!(types.contains(b"sRGB"));
        assert!(!types.contains(b"iCCP"));
        // ...but cICP itself is still passed through unchanged.
        let decoder = png::Decoder::new(std::io::Cursor::new(&result.png_data));
        let reader = decoder.read_info().expect("read output info");
        assert_eq!(
            reader.info().coding_independent_code_points,
            Some(sdr_srgb_cicp())
        );
    }

    /// Standard sRGB primaries/white point, as would appear in a real-world PNG's
    /// cHRM chunk when it carries explicit (redundant) sRGB chromaticities instead
    /// of an sRGB chunk or ICC profile.
    fn srgb_chromaticities() -> png::SourceChromaticities {
        png::SourceChromaticities::new(
            (0.3127, 0.3290),
            (0.6400, 0.3300),
            (0.3000, 0.6000),
            (0.1500, 0.0600),
        )
    }

    #[test]
    fn gama_chrm_are_read_from_decoder_chunk_fields_not_encoder_only_fields() {
        // Regression test for the pre-existing bug: `extract_metadata` used to read
        // `info.source_gamma`/`info.source_chromaticities` (encoder-only fields,
        // always `None` after a decode) instead of `info.gama_chunk`/`info.chrm_chunk`
        // (what the decoder actually populates). Verify a round trip through the
        // real `png` decoder yields non-`None` values for a PNG carrying gAMA/cHRM.
        let decoder = png::Decoder::new(std::io::Cursor::new(encode_test_rgba_png_with_gama_chrm(
            1,
            1,
            &[128, 128, 128, 255],
            None,
            None,
            Some(png::ScaledFloat::new(1.0 / 2.2)),
            Some(srgb_chromaticities()),
        )));
        let reader = decoder.read_info().expect("read info");
        let info = reader.info();
        assert!(info.source_gamma.is_none(), "encoder-only field stays None on decode");
        assert!(info.gama_chunk.is_some(), "decoder populates gama_chunk");
        assert!(info.chrm_chunk.is_some(), "decoder populates chrm_chunk");
    }

    #[test]
    fn srgb_like_gama_chrm_normalizes_to_near_identity_and_writes_srgb_chunk() {
        // gAMA ~= 1/2.2 with sRGB-standard chromaticities: the synthesized RGB
        // profile is approximately sRGB itself, so the lcms2 round trip should be
        // a near-identity transform. Output should carry an sRGB chunk (and drop
        // gAMA/cHRM), matching the iCCP-branch's normalization contract.
        //
        // Pixel values are kept away from near-black (>= 0x50): a pure gamma-2.2
        // power curve (what `build_rgb_profile_from_png_chromaticities` synthesizes)
        // legitimately diverges from real sRGB's TRC in its linear toe segment below
        // ~0.0031 linear light, so very dark pixels see a real several-ULP shift
        // there (e.g. 10 -> 3) that is not a bug — just where a plain 1/2.2 gamma
        // stops being a good approximation of sRGB. Mid-to-high tones (used here)
        // are where the "near-identity" claim actually holds.
        let rgba = vec![
            100u8, 150, 200, 255, // px0
            220, 230, 240, 255, // px1
            90, 110, 130, 255, // px2
            245, 235, 225, 128, // px3 (also exercises alpha passthrough)
        ];
        let input = encode_test_rgba_png_with_gama_chrm(
            2,
            2,
            &rgba,
            None,
            None,
            Some(png::ScaledFloat::new(1.0 / 2.2)),
            Some(srgb_chromaticities()),
        );

        let result = process_png_bytes(&input, PipelineOptions::default()).expect("process PNG");

        let types = chunk_types(&result.png_data);
        assert!(types.contains(b"sRGB"), "expected sRGB chunk: {types:?}");
        assert!(!types.contains(b"gAMA"), "gAMA should be dropped: {types:?}");
        assert!(!types.contains(b"cHRM"), "cHRM should be dropped: {types:?}");

        let decoded = image::load_from_memory_with_format(&result.png_data, image::ImageFormat::Png)
            .expect("decode output PNG")
            .to_rgba8();
        let out_rgba = decoded.into_raw();
        assert_eq!(out_rgba.len(), rgba.len());
        let max_diff = rgba
            .iter()
            .zip(out_rgba.iter())
            .map(|(a, b)| (*a as i16 - *b as i16).unsigned_abs())
            .max()
            .unwrap_or(0);
        assert!(
            max_diff <= 2,
            "near-identity sRGB-like gAMA/cHRM normalization should barely move pixels, \
             got max_diff={max_diff}: input={rgba:?} output={out_rgba:?}"
        );
    }

    #[test]
    fn linear_gama_brightens_mid_gray_toward_srgb_encoding() {
        // gAMA=1.0 (linear light, no encoding gamma) with sRGB chromaticities: a raw
        // sample of 128 (~0.502) represents *linear* light of ~0.502, which the sRGB
        // transfer function encodes to ~0.735 (~187/255) — brighter, not darker.
        // This pins down the conversion direction for non-sRGB-equivalent gAMA/cHRM.
        let rgba = vec![128u8, 128, 128, 255];
        let input = encode_test_rgba_png_with_gama_chrm(
            1,
            1,
            &rgba,
            None,
            None,
            Some(png::ScaledFloat::new(1.0)),
            Some(srgb_chromaticities()),
        );

        let result = process_png_bytes(&input, PipelineOptions::default()).expect("process PNG");

        let types = chunk_types(&result.png_data);
        assert!(types.contains(b"sRGB"), "expected sRGB chunk: {types:?}");

        let decoded = image::load_from_memory_with_format(&result.png_data, image::ImageFormat::Png)
            .expect("decode output PNG")
            .to_rgba8();
        let out = decoded.into_raw();
        assert_eq!(out[3], 255, "alpha untouched");
        for channel in &out[0..3] {
            assert!(
                (183..=192).contains(channel),
                "expected mid-gray to brighten to ~187/255, got {channel} (full={out:?})"
            );
            assert!(
                *channel > rgba[0],
                "linear-to-sRGB normalization must brighten, not darken: {channel} <= {}",
                rgba[0]
            );
        }
    }

    #[test]
    fn no_icc_flag_passes_through_gama_chrm_unchanged() {
        let rgba = vec![128u8, 128, 128, 255];
        let gamma = png::ScaledFloat::new(1.0);
        let chroma = srgb_chromaticities();
        let input =
            encode_test_rgba_png_with_gama_chrm(1, 1, &rgba, None, None, Some(gamma), Some(chroma));

        let result = process_png_bytes(
            &input,
            PipelineOptions {
                no_icc: true,
                ..PipelineOptions::default()
            },
        )
        .expect("process PNG with --no-icc");

        let types = chunk_types(&result.png_data);
        assert!(types.contains(b"gAMA"), "gAMA must pass through: {types:?}");
        assert!(types.contains(b"cHRM"), "cHRM must pass through: {types:?}");
        assert!(!types.contains(b"sRGB"), "no normalization under --no-icc: {types:?}");

        // Pixels must be byte-identical (decoding treats them as literal 8-bit
        // samples regardless of the declared gAMA/cHRM; only the chunk passthrough
        // matters here).
        let decoded = image::load_from_memory_with_format(&result.png_data, image::ImageFormat::Png)
            .expect("decode output PNG")
            .to_rgba8();
        assert_eq!(decoded.into_raw(), rgba);

        let decoder = png::Decoder::new(std::io::Cursor::new(&result.png_data));
        let reader = decoder.read_info().expect("read output info");
        assert_eq!(reader.info().gama_chunk, Some(gamma));
        assert_eq!(reader.info().chrm_chunk, Some(chroma));
    }

    #[test]
    fn plain_png_without_color_chunks_is_unaffected_by_gama_chrm_fix() {
        // No gAMA/cHRM/sRGB/iCCP at all: normalization must stay a no-op (both
        // before and after the fix), and no color chunks should appear in the output.
        let rgba = vec![
            10u8, 20, 30, 255, 200, 210, 220, 255, 5, 6, 7, 255, 250, 240, 230, 255,
        ];
        let input = encode_test_rgba_png(2, 2, &rgba, None, None);

        let result = process_png_bytes(&input, PipelineOptions::default()).expect("process PNG");

        let types = chunk_types(&result.png_data);
        assert!(!types.contains(b"gAMA"));
        assert!(!types.contains(b"cHRM"));
        assert!(!types.contains(b"sRGB"));
        assert!(!types.contains(b"iCCP"));

        let decoded = image::load_from_memory_with_format(&result.png_data, image::ImageFormat::Png)
            .expect("decode output PNG")
            .to_rgba8();
        assert_eq!(decoded.into_raw(), rgba);
    }
}
