# PNGOptim - Project Guide

## Overview

PNGOptim is a Rust CLI tool for PNG quantization (lossy compression), aiming to replicate and surpass pngquant/libimagequant. The project follows a "replicate first, optimize later" strategy across phases A-H.

**Current status**: Phases A-G complete. Phase H (APNG) blocked pending shadow-banding fix in default dither path. Active work: Algorithm Replication track (RF-1 through RF-7).

## Build / Test / Run

```bash
# Build
cargo build --release

# Run tests
cargo test

# Spot check (primary validation)
./target/release/pngoptim /Users/5km/Downloads/demo.png -o /tmp/demo-current.png --quality 65-75 --force
pngquant /Users/5km/Downloads/demo.png -o /tmp/demo-pngquant.png --quality 65-75 --force

# Full regression suites (via xtask)
cargo run --release --bin xtask -- smoke --run-id <id>
cargo run --release --bin xtask -- compat --run-id <id>
cargo run --release --bin xtask -- quality-size --run-id <id>
cargo run --release --bin xtask -- perf --run-id <id>
cargo run --release --bin xtask -- stability --run-id <id>
```

## Code Structure

| File | Lines | Purpose |
|------|-------|---------|
| `src/palette_quant.rs` | ~2462 | Core quantization: histogram, median cut, k-means, VP-tree nearest search, remap (plain + Floyd dithering), dither map, contrast maps |
| `src/pipeline.rs` | ~700 | Processing pipeline: decode -> color management -> quantize -> encode. Quality gating, metadata preservation, ICC/gAMA normalization |
| `src/quality.rs` | ~257 | InternalPixel (gamma-weighted ARGB), quality<->MSE mapping, SpeedSettings, quality evaluation |
| `src/cli.rs` | ~293 | CLI argument parsing (clap), QualityRange, output path logic |
| `src/main.rs` | ~252 | Entry point, batch processing, exit codes |
| `src/error.rs` | ~79 | Error types (AppError) |
| `src/apng.rs` | ~567 | APNG support (Phase H, currently blocked) |
| `src/quant.rs` | ~115 | Legacy quantizer bridge (mostly unused) |
| `src/bin/xtask.rs` | ~3148 | Test harness: smoke, compat, quality-size, perf, stability, cross-platform |

## Key Algorithm Modules (in `palette_quant.rs`)

1. **Histogram** (`build_histogram`, `finalize_histogram`): RGBA -> gamma-weighted InternalPixel histogram with importance weighting and cluster indexing
2. **Median Cut** (`median_cut_palette`, `ColorBox::split`): Weighted median cut with variance-based split, quality-target early termination
3. **K-Means** (`kmeans_iteration`, `refine_palette`): Parallelized k-means with reflected-color weight adjustment, unused color replacement via VP-tree
4. **VP-Tree** (`NearestTree`): Vantage-point tree with popularity-based vantage selection, likely-index early exit, nearest-other-color pruning
5. **Plain Remap** (`remap_image_plain`, `finalize_plain_remap`): Row-hint nearest search with importance-weighted feedback
6. **Floyd Dithering** (`remap_image_dithered`, `dither_row`): Selective Floyd-Steinberg with dither map, serpentine scan, chunked parallelism
7. **Contrast Maps** (`compute_contrast_maps`, `build_dither_map`): Edge detection + noise estimation for selective dithering

## Reference-First Discipline

All algorithm work follows reference-first methodology: read the reference implementation first, document differences, then align.

**Reference implementation (local)**:
- `/Users/5km/Dev/C/libimagequant/src/hist.rs` - Histogram
- `/Users/5km/Dev/C/libimagequant/src/mediancut.rs` - Median cut
- `/Users/5km/Dev/C/libimagequant/src/kmeans.rs` - K-means
- `/Users/5km/Dev/C/libimagequant/src/nearest.rs` - VP-tree nearest search
- `/Users/5km/Dev/C/libimagequant/src/remap.rs` - Remap (plain + Floyd)
- `/Users/5km/Dev/C/libimagequant/src/quant.rs` - Orchestration

## Algorithm Replication Status

| Sub-phase | Status | Key |
|-----------|--------|-----|
| RF-1 | Done | quality/speed semantics |
| RF-2 | Partially Done | feedback loop, palette search |
| RF-3 | Done | VP-tree nearest search |
| RF-4 | Partially Done | remap plain + k-means finalize |
| RF-5 | Partially Done | dither map + selective Floyd |
| RF-6 | Done | skip-if-larger heuristics |
| RF-7 | Done | regression gates |

## Current Blocker

`demo.png --quality 65-75` default dither: output has 18 colors vs pngquant's 19 colors, missing one mid-gray that causes visible shadow banding. Root cause: remap-phase k-means finalize not fully aligned with reference `remap_to_palette()` which runs full-image k-means feedback before final remap.

## Engineering Constraints

1. Rust-only toolchain; no Python in mainline
2. Algorithm alignment with pngquant/libimagequant, not blind invention
3. MIT license; reference code not directly copied (license policy pending)
4. Every change must pass regression gates before merging
5. No bit-exact output required; statistical equivalence is the goal
