# APNG Support

pngoptim supports APNG (Animated PNG) files with automatic detection, lossy quantization with a global shared palette, and lossless structure optimization.

## Auto-Detection

APNG files are automatically detected by scanning for the `acTL` (Animation Control) chunk in the PNG stream. No special flag or file extension is needed — standard `.png` files containing animation data are recognized and routed to the APNG processing path.

```bash
# Just use pngoptim normally — APNG is auto-detected
pngoptim animated.png -o optimized.png
```

## Lossy Quantization

APNG files are quantized using a **global shared palette** strategy:

1. **Lossless structure optimization** runs first (duplicate frame folding, optional rect minimization)
2. Per-frame histograms are built and **merged into a single global histogram**
3. A single optimal palette (up to 256 colors) is generated from the merged histogram
4. Each frame is independently **remapped** to the global palette (with optional dithering)
5. Quality is evaluated per-frame; the **worst frame's score** determines the overall quality
6. Quality gating applies: if the worst frame falls below `--quality` minimum, the operation fails

This approach ensures all frames share a single `PLTE` chunk (as required by the APNG spec) while preserving per-frame visual quality.

```bash
# Quantize APNG with quality control
pngoptim animated.png -o optimized.png --quality 65-75

# Quantize with specific speed/dither settings
pngoptim animated.png -o optimized.png --quality 60-80 --speed 4 --floyd 0.5
```

## Optimization Modes

### Safe Mode (default)

```bash
pngoptim animated.png -o optimized.png
# or explicitly:
pngoptim animated.png -o optimized.png --apng-mode safe
```

Safe mode performs **duplicate frame folding**: consecutive frames with identical pixel content are merged into a single frame with an extended duration. This runs before quantization.

### Aggressive Mode

```bash
pngoptim animated.png -o optimized.png --apng-mode aggressive
```

Aggressive mode includes everything in safe mode, plus **frame rectangle minimization**: each frame's bounding rectangle is shrunk to cover only the pixels that actually changed from the previous frame. This can significantly reduce file size for animations with small per-frame changes.

Aggressive mode includes a **post-verification rollback** safety net: after minimizing frame rectangles, the output is validated. If the optimization would increase the file size (e.g., for already-optimized inputs), the optimization is rolled back automatically.

## Input Protection

pngoptim detects already-optimized APNG inputs by scanning chunk-level characteristics:

- **Indexed color** APNG (already palette-based)
- **Sub-rect frames** (frames with non-full-size rectangles)

When these characteristics are detected, re-optimization is skipped to prevent size regression. This means it's always safe to run pngoptim on APNG files — it won't make them larger.

## Integration with Other Options

| Option | Behavior with APNG |
|--------|-------------------|
| `--quality` | Controls quantization quality range; worst-frame quality is reported |
| `--speed` | Controls quantization speed/quality tradeoff |
| `--floyd` / `--nofs` | Controls dithering for per-frame remapping |
| `--skip-if-larger` | Applied after quantization; output is discarded if larger than input |
| `--strip` | Not yet applied to APNG metadata (planned) |

## Examples

```bash
# Default safe optimization with lossy quantization
pngoptim animation.png -o animation-opt.png --quality 65-75

# Aggressive structural optimization + quantization
pngoptim animation.png -o animation-opt.png --apng-mode aggressive --quality 60-80

# With skip-if-larger safety
pngoptim animation.png -o animation-opt.png --quality 65-75 --skip-if-larger

# Batch process mixed PNG/APNG files
pngoptim *.png --ext -opt.png --quality 65-75
# Static PNGs and APNG files are both quantized
```
