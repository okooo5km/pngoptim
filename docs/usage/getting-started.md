# Getting Started

## Installation

### Homebrew (macOS)

```bash
brew install okooo5km/tap/pngoptim
```

### GitHub Releases

Download prebuilt binaries for macOS (Universal), Linux (x86_64/arm64), and Windows (x86_64) from [Releases](https://github.com/okooo5km/pngoptim/releases/latest).

### Build from Source

Requires Rust 1.85+ (edition 2024).

```bash
# Via cargo install
cargo install --git https://github.com/okooo5km/pngoptim.git

# Or clone and build
git clone https://github.com/okooo5km/pngoptim.git
cd pngoptim
cargo build --release
# Binary at ./target/release/pngoptim
```

The `lcms2-sys` dependency compiles vendored C sources via the `cc` crate — no system library installation needed.

## Basic Usage

### Single File

```bash
pngoptim input.png -o output.png
```

### Batch Processing

```bash
# Process multiple files with a suffix
pngoptim *.png --ext -opt.png
```

### stdin/stdout

```bash
cat input.png | pngoptim - -o - > output.png
```

## Core Concepts

### Quantization

PNG quantization reduces the number of colors in an image (typically to 256 or fewer), converting from 32-bit RGBA to an 8-bit indexed palette. This is a lossy process but usually produces visually identical results at dramatically smaller file sizes (60–80% reduction).

### Quality Range

The `--quality` option controls the minimum acceptable quality and target quality as a range `MIN-MAX` (0–100):

```bash
# Target quality 80, reject below 65
pngoptim input.png -o output.png --quality 65-80
```

If the quantized output falls below the minimum quality, pngoptim exits with code 99 instead of writing a poor-quality file. Supported formats:

| Format | Meaning |
|--------|---------|
| `65-80` | Min 65, target 80 |
| `-80` | Min 0, target 80 |
| `65-` | Min 65, target 100 |
| `70` | Target 70, min auto-calculated (~90% of target) |

### Speed vs Quality

The `--speed` option (1–11) trades processing time for output quality:

- **1–3**: Slowest, best quality. More k-means iterations, finer dithering.
- **4** (default): Balanced.
- **5–8**: Faster, slightly less optimal palette.
- **9–11**: Fastest. Dithering disabled at speed 10+.

### Skip-if-Larger

Use `--skip-if-larger` to avoid writing output when the quantized file is larger than the original — common with already-optimized or very small images.

```bash
pngoptim input.png -o output.png --skip-if-larger
```
