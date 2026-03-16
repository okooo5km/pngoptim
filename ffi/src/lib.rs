// Copyright (c) 2026 okooo5km(十里). All rights reserved.
// Licensed under the MIT License.

//! C FFI bindings for the pngoptim PNG quantization engine.
//!
//! Exports three functions:
//! - `pngoptim_default_options()` — returns default options
//! - `pngoptim_process()` — processes PNG data
//! - `pngoptim_result_free()` — frees the result

// FFI extern functions intentionally take raw pointers
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use std::ffi::{CString, c_char};
use std::ptr;
use std::slice;

use pngoptim::cli::{ApngMode, QualityRange};
use pngoptim::error::AppError;
use pngoptim::pipeline::{PipelineOptions, process_png_bytes};

// ── Error codes ──

const ERROR_SUCCESS: i32 = 0;
const ERROR_ARG: i32 = 1;
const ERROR_IO: i32 = 2;
const ERROR_DECODE: i32 = 3;
const ERROR_ENCODE: i32 = 4;
const ERROR_QUALITY_TOO_LOW: i32 = 98;
const ERROR_SKIP_IF_LARGER: i32 = 99;

// ── C structs ──

/// Input options for PNG optimization.
#[repr(C)]
pub struct PNGOptimOptions {
    /// Whether a quality constraint is set.
    pub has_quality: bool,
    /// Minimum acceptable quality (0-100).
    pub quality_min: u8,
    /// Maximum (target) quality (0-100).
    pub quality_max: u8,
    /// Speed preset (1-11, 0 = default of 4).
    pub speed: u8,
    /// Dither level (0.0-1.0, negative = default of 1.0).
    pub dither_level: f32,
    /// Posterize bits (0 = disabled).
    pub posterize: u8,
    /// Strip metadata from output.
    pub strip: bool,
    /// Skip output if it would be larger than input.
    pub skip_if_larger: bool,
    /// Skip ICC profile color-space normalization.
    pub no_icc: bool,
    /// APNG handling mode (0 = safe, 1 = aggressive).
    pub apng_mode: u8,
}

/// Result of PNG optimization.
#[repr(C)]
pub struct PNGOptimResult {
    /// Output PNG data (owned by this struct).
    pub data: *mut u8,
    /// Length of data in bytes.
    pub data_len: usize,
    /// Image width in pixels.
    pub width: u32,
    /// Image height in pixels.
    pub height: u32,
    /// Input file size in bytes.
    pub input_bytes: u64,
    /// Output file size in bytes.
    pub output_bytes: u64,
    /// Quality score (0-100).
    pub quality_score: u8,
    /// Quality MSE (mean squared error).
    pub quality_mse: f64,
    /// Decode time in milliseconds.
    pub decode_ms: f64,
    /// Quantize time in milliseconds.
    pub quantize_ms: f64,
    /// Encode time in milliseconds.
    pub encode_ms: f64,
    /// Total processing time in milliseconds.
    pub total_ms: f64,
    /// Error code (0 = success).
    pub error_code: i32,
    /// Error message (null on success, owned CString on error).
    pub error_message: *mut c_char,
    /// For QualityTooLow: the minimum quality that was required.
    pub quality_minimum: u8,
    /// For SkipIfLargerRejected: the maximum file size threshold.
    pub maximum_file_size: u64,
}

// ── Exported functions ──

/// Returns default optimization options.
#[unsafe(no_mangle)]
pub extern "C" fn pngoptim_default_options() -> PNGOptimOptions {
    PNGOptimOptions {
        has_quality: false,
        quality_min: 0,
        quality_max: 100,
        speed: 4,
        dither_level: 1.0,
        posterize: 0,
        strip: false,
        skip_if_larger: false,
        no_icc: false,
        apng_mode: 0,
    }
}

/// Process PNG data with the given options.
///
/// Returns a heap-allocated `PNGOptimResult` that must be freed with
/// `pngoptim_result_free`. Returns null only if `input` is null.
#[unsafe(no_mangle)]
pub extern "C" fn pngoptim_process(
    input: *const u8,
    input_len: usize,
    options: *const PNGOptimOptions,
) -> *mut PNGOptimResult {
    if input.is_null() {
        return ptr::null_mut();
    }

    let input_slice = unsafe { slice::from_raw_parts(input, input_len) };

    let opts = if options.is_null() {
        PipelineOptions::default()
    } else {
        let c_opts = unsafe { &*options };
        convert_options(c_opts)
    };

    match process_png_bytes(input_slice, opts) {
        Ok(result) => {
            let mut png_data = result.png_data.into_boxed_slice();
            let data_ptr = png_data.as_mut_ptr();
            let data_len = png_data.len();
            std::mem::forget(png_data);

            let out = Box::new(PNGOptimResult {
                data: data_ptr,
                data_len,
                width: result.width,
                height: result.height,
                input_bytes: result.input_bytes,
                output_bytes: result.output_bytes,
                quality_score: result.quality_score,
                quality_mse: result.quality_mse,
                decode_ms: result.metrics.decode_ms,
                quantize_ms: result.metrics.quantize_ms,
                encode_ms: result.metrics.encode_ms,
                total_ms: result.metrics.total_ms,
                error_code: ERROR_SUCCESS,
                error_message: ptr::null_mut(),
                quality_minimum: 0,
                maximum_file_size: 0,
            });
            Box::into_raw(out)
        }
        Err(err) => make_error_result(&err),
    }
}

/// Free a result previously returned by `pngoptim_process`.
#[unsafe(no_mangle)]
pub extern "C" fn pngoptim_result_free(result: *mut PNGOptimResult) {
    if result.is_null() {
        return;
    }

    let result = unsafe { Box::from_raw(result) };

    // Free output data
    if !result.data.is_null() && result.data_len > 0 {
        unsafe {
            let _ = Box::from_raw(std::ptr::slice_from_raw_parts_mut(result.data, result.data_len));
        }
    }

    // Free error message
    if !result.error_message.is_null() {
        unsafe {
            let _ = CString::from_raw(result.error_message);
        }
    }

    // Box::from_raw above already reclaimed the PNGOptimResult itself
}

// ── Internal helpers ──

fn convert_options(c_opts: &PNGOptimOptions) -> PipelineOptions {
    let quality = if c_opts.has_quality {
        Some(QualityRange {
            raw: format!("{}-{}", c_opts.quality_min, c_opts.quality_max),
            min: c_opts.quality_min,
            max: c_opts.quality_max,
        })
    } else {
        None
    };

    let speed = if c_opts.speed == 0 { 4 } else { c_opts.speed };

    let dither_level = if c_opts.dither_level < 0.0 {
        1.0
    } else {
        c_opts.dither_level
    };

    let posterize = if c_opts.posterize == 0 {
        None
    } else {
        Some(c_opts.posterize)
    };

    let apng_mode = match c_opts.apng_mode {
        1 => ApngMode::Aggressive,
        _ => ApngMode::Safe,
    };

    PipelineOptions {
        quality,
        speed,
        dither_level,
        posterize,
        strip: c_opts.strip,
        skip_if_larger: c_opts.skip_if_larger,
        no_icc: c_opts.no_icc,
        apng_mode,
    }
}

fn make_error_result(err: &AppError) -> *mut PNGOptimResult {
    let (error_code, quality_score, quality_minimum, input_bytes, output_bytes, maximum_file_size) =
        match err {
            AppError::Arg(_) => (ERROR_ARG, 0u8, 0u8, 0u64, 0u64, 0u64),
            AppError::Io { .. } => (ERROR_IO, 0, 0, 0, 0, 0),
            AppError::Decode(_) => (ERROR_DECODE, 0, 0, 0, 0, 0),
            AppError::Encode(_) => (ERROR_ENCODE, 0, 0, 0, 0, 0),
            AppError::QualityTooLow { minimum, actual } => {
                (ERROR_QUALITY_TOO_LOW, *actual, *minimum, 0, 0, 0)
            }
            AppError::SkipIfLargerRejected {
                input_bytes,
                output_bytes,
                maximum_file_size,
                quality_score,
            } => (
                ERROR_SKIP_IF_LARGER,
                *quality_score,
                0,
                *input_bytes,
                *output_bytes,
                *maximum_file_size,
            ),
        };

    let msg = CString::new(err.to_string()).unwrap_or_default();

    let out = Box::new(PNGOptimResult {
        data: ptr::null_mut(),
        data_len: 0,
        width: 0,
        height: 0,
        input_bytes,
        output_bytes,
        quality_score,
        quality_mse: 0.0,
        decode_ms: 0.0,
        quantize_ms: 0.0,
        encode_ms: 0.0,
        total_ms: 0.0,
        error_code,
        error_message: msg.into_raw(),
        quality_minimum,
        maximum_file_size,
    });
    Box::into_raw(out)
}
