# Library API

pngoptim exposes a Rust library crate for programmatic PNG quantization.

## Cargo Dependency

```toml
[dependencies]
pngoptim = { git = "https://github.com/okooo5km/pngoptim.git" }
```

## Core Functions

### `process_png_bytes`

Quantize PNG data from an in-memory byte slice.

```rust
use pngoptim::pipeline::{process_png_bytes, PipelineOptions};

let input = std::fs::read("input.png").unwrap();
let options = PipelineOptions::default();
let result = process_png_bytes(&input, options).unwrap();
std::fs::write("output.png", &result.png_data).unwrap();

println!("{}x{}, quality={}, {:.1}% reduction",
    result.width, result.height, result.quality_score,
    (1.0 - result.output_bytes as f64 / result.input_bytes as f64) * 100.0);
```

### `process_png_file`

Convenience wrapper that reads a file path and calls `process_png_bytes`.

```rust
use std::path::Path;
use pngoptim::pipeline::{process_png_file, PipelineOptions};

let result = process_png_file(Path::new("input.png"), PipelineOptions::default()).unwrap();
std::fs::write("output.png", &result.png_data).unwrap();
```

### `write_output_file`

Write output bytes to a file path, respecting the `force` flag for overwrite control.

```rust
use std::path::Path;
use pngoptim::pipeline::write_output_file;

write_output_file(Path::new("output.png"), &png_data, true /* force */).unwrap();
```

## Configuration: `PipelineOptions`

```rust
pub struct PipelineOptions {
    pub quality: Option<QualityRange>,  // Quality range (min-max)
    pub speed: u8,                      // 1-11, default 4
    pub dither_level: f32,              // 0.0-1.0, default 1.0
    pub posterize: Option<u8>,          // 0-8 bits, default None
    pub strip: bool,                    // Strip metadata, default false
    pub skip_if_larger: bool,           // Skip if output > input, default false
    pub no_icc: bool,                   // Skip ICC normalization, default false
    pub apng_mode: ApngMode,            // Safe or Aggressive, default Safe
}
```

`PipelineOptions::default()` provides sensible defaults (speed=4, full dithering, ICC normalization enabled, safe APNG mode).

## Result: `PipelineResult`

```rust
pub struct PipelineResult {
    pub width: u32,
    pub height: u32,
    pub input_bytes: u64,
    pub output_bytes: u64,
    pub quality_score: u8,    // 0-100, quality of the quantized output
    pub quality_mse: f64,     // Mean squared error
    pub png_data: Vec<u8>,    // The quantized PNG bytes
    pub metrics: PipelineMetrics,
}

pub struct PipelineMetrics {
    pub decode_ms: f64,
    pub quantize_ms: f64,
    pub encode_ms: f64,
    pub total_ms: f64,
}
```

## Error Handling

All functions return `Result<_, AppError>`. The `AppError` enum covers:

```rust
pub enum AppError {
    Arg(String),                    // Invalid arguments (exit code 2)
    Io { path, source },            // I/O errors with optional path context (exit code 3)
    Decode(String),                 // PNG decode failures (exit code 4)
    Encode(String),                 // PNG encode failures (exit code 4)
    QualityTooLow { minimum, actual },  // Quality below threshold (exit code 98)
    SkipIfLargerRejected { .. },    // Output larger than input (exit code 99)
}
```

`AppError` implements `std::error::Error` and `Display`. Use `exit_code()` to get the appropriate process exit code.

## APNG Handling

APNG files are automatically detected and routed to the lossless optimization path. The `apng_mode` field in `PipelineOptions` controls the optimization level (see [APNG docs](apng.md)).

## Swift Integration

For Swift/Apple platform integration, see [pngoptim-swift](https://github.com/okooo5km/pngoptim-swift) — a Swift package wrapping the pngoptim C API via FFI.
