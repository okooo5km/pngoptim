# pngoptim

[![CI](https://github.com/okooo5km/pngoptim/actions/workflows/ci.yml/badge.svg)](https://github.com/okooo5km/pngoptim/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/okooo5km/pngoptim)](https://github.com/okooo5km/pngoptim/releases/latest)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

A fast, single-binary PNG quantization CLI written in Rust — a modern alternative to [pngquant](https://pngquant.org/).

- **1.69x faster** than pngquant on average (up to 1.98x on large images)
- **2–3% smaller** files at equivalent quality settings
- **APNG support** with lossy quantization (global shared palette) and lossless structure optimization — safe mode (duplicate frame folding) and aggressive mode (+ frame rect minimization with rollback safety)
- **ICC color management** with automatic sRGB normalization via lcms2

## Installation

### Homebrew (macOS)

```bash
brew install okooo5km/tap/pngoptim
```

### GitHub Releases

Download prebuilt binaries for macOS (Universal), Linux (x86_64/arm64), and Windows (x86_64) from [Releases](https://github.com/okooo5km/pngoptim/releases/latest).

### Build from Source

```bash
# Via cargo install
cargo install --git https://github.com/okooo5km/pngoptim.git

# Or clone and build
git clone https://github.com/okooo5km/pngoptim.git
cd pngoptim
cargo build --release
# Binary at ./target/release/pngoptim
```

## Usage

```bash
# Basic: quantize a single PNG
pngoptim input.png -o output.png

# Quality control (min-max, like pngquant)
pngoptim input.png -o output.png --quality 65-80

# Batch processing (multiple files)
pngoptim *.png --ext -opt.png

# Pipe via stdin/stdout
cat input.png | pngoptim - -o - > output.png

# Skip if output would be larger than input
pngoptim input.png -o output.png --skip-if-larger

# Strip metadata and force overwrite
pngoptim input.png -o output.png --strip --force

# APNG animated PNG (auto-detected, no special flag needed)
pngoptim animated.png -o optimized.png

# APNG with aggressive optimization (frame rect minimization)
pngoptim animated.png -o optimized.png --apng-mode aggressive
```

### Options

| Option | Description | Default |
|--------|-------------|---------|
| `INPUT...` | Input PNG file(s), or `-` for stdin | (required) |
| `-o, --output FILE` | Output file path, or `-` for stdout | `<input>-mvp.png` |
| `--ext SUFFIX` | Output filename suffix (batch mode) | `-mvp.png` |
| `--quality MIN-MAX` | Quality range (0–100). Formats: `N`, `-N`, `N-`, `MIN-MAX` | Full range |
| `--speed N` | Speed/quality trade-off (1=slowest/best, 11=fastest) | `4` |
| `--floyd[=N]` | Floyd-Steinberg dithering strength (0.0–1.0) | `1.0` |
| `--nofs` | Disable Floyd-Steinberg dithering entirely | Off |
| `--posterize N` | Posterize output (0–8 bits) | Off |
| `--strip` | Strip all metadata chunks | Off |
| `--force` | Overwrite existing output files | Off |
| `--skip-if-larger` | Skip writing if output is larger than input | Off |
| `--no-icc` | Skip ICC profile normalization | Off |
| `--apng-mode MODE` | APNG optimization strategy: `safe` or `aggressive` | `safe` |
| `-q, --quiet` | Suppress non-error output | Off |

## Benchmark

Tested on 9 images of varying sizes (187 KB – 8.7 MB), macOS ARM64, Rust 1.87 release build.

### Speed (vs pngquant, quality 65–75)

| Image | Size | pngoptim | pngquant | Speedup |
|-------|------|----------|----------|---------|
| Large photo (8.7 MB) | 3456×3456 | 0.89s | 1.76s | **1.98x** |
| Medium photo (3.4 MB) | 2048×1536 | 0.39s | 0.69s | **1.77x** |
| UI screenshot (2.1 MB) | 1920×1080 | 0.25s | 0.43s | **1.72x** |
| Icon set (1.2 MB) | 1024×1024 | 0.12s | 0.20s | **1.67x** |
| Small avatar (187 KB) | 256×256 | 0.02s | 0.03s | **1.50x** |
| **Average** | | | | **1.69x** |

### File Size

| Quality Range | pngoptim Total | pngquant Total | Difference |
|---------------|----------------|----------------|------------|
| 65–75 | 4.82 MB | 4.92 MB | **−2.0%** |
| 80–90 | 5.31 MB | 5.47 MB | **−2.9%** |

Visual quality is equivalent — no visible differences at the same quality settings.

## How It Works

pngoptim implements a full quantization pipeline inspired by [libimagequant](https://github.com/ImageOptim/libimagequant):

1. **Histogram** — RGBA pixels are converted to gamma-weighted internal representation with importance weighting
2. **Median Cut** — Variance-based palette generation with quality-target early termination
3. **K-Means Refinement** — Parallelized k-means with reflected-color weight adjustment
4. **VP-Tree Nearest Search** — Vantage-point tree with popularity-based vantage selection and early exit
5. **Remap** — Plain (row-hint nearest) or Floyd-Steinberg dithering with edge/noise-aware dither maps
6. **Encode** — Hand-written PNG encoder with [zlib-rs](https://github.com/trifectatechfoundation/zlib-rs), dual `mem_level` (5 & 8) parallel compression picking the smaller result

Additional features:
- **ICC Color Management**: Embedded ICC profiles are normalized to sRGB via lcms2 before quantization
- **APNG Optimization**: Automatic detection via `acTL` chunk (no special flag needed). Lossy quantization uses a global shared palette built from all frames' merged histograms, with per-frame independent remapping and worst-frame quality gating. Two structural modes: **safe** (default) folds duplicate consecutive frames; **aggressive** additionally minimizes frame rectangles with post-verification rollback to prevent size regression. Already-optimized APNG inputs (indexed/sub-rect) are detected and skipped to avoid re-optimization
- **Parallelism**: Rayon-based parallelism across histogram building, k-means, contrast maps, remap, and encoding

## Building from Source

Requires Rust 1.85+ (edition 2024).

```bash
cargo build --release
cargo test
```

The `lcms2-sys` dependency compiles vendored C sources via the `cc` crate — no system library installation needed.

## License

MIT — [okooo5km (十里)](https://github.com/okooo5km)

Inspired by [pngquant](https://pngquant.org/) / [libimagequant](https://github.com/ImageOptim/libimagequant) by Kornel Lesiński.
