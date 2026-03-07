// Copyright (c) 2026 okooo5km(十里). All rights reserved.
// Licensed under the MIT License.

//! PNGOptim — PNG quantization (lossy compression) library.
//!
//! Core functionality for quantizing PNG images, replicating and surpassing
//! pngquant/libimagequant in both speed and compression ratio.
//!
//! # Example
//!
//! ```no_run
//! use pngoptim::pipeline::{process_png_bytes, PipelineOptions};
//!
//! let input = std::fs::read("input.png").unwrap();
//! let options = PipelineOptions::default();
//! let result = process_png_bytes(&input, options).unwrap();
//! std::fs::write("output.png", &result.png_data).unwrap();
//! ```

pub mod apng;
pub mod cli;
pub mod error;
pub mod palette_quant;
pub mod pipeline;
pub mod quality;
