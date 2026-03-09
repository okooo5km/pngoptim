# APNG Support

pngoptim supports APNG (Animated PNG) files with automatic detection and lossless structure optimization.

## Auto-Detection

APNG files are automatically detected by scanning for the `acTL` (Animation Control) chunk in the PNG stream. No special flag or file extension is needed — standard `.png` files containing animation data are recognized and routed to the APNG processing path.

```bash
# Just use pngoptim normally — APNG is auto-detected
pngoptim animated.png -o optimized.png
```

## Optimization Modes

### Safe Mode (default)

```bash
pngoptim animated.png -o optimized.png
# or explicitly:
pngoptim animated.png -o optimized.png --apng-mode safe
```

Safe mode performs **duplicate frame folding**: consecutive frames with identical pixel content are merged into a single frame with an extended duration. This is completely lossless and risk-free.

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

## Current Limitations

- APNG processing is currently **lossless only** — frames are not quantized. The output preserves the original color depth and reports `quality_score=100`.
- Lossy APNG quantization (per-frame quantization with shared palette) is planned for a future release (Phase H3).

## Integration with Other Options

| Option | Behavior with APNG |
|--------|-------------------|
| `--skip-if-larger` | Applied after APNG optimization; output is discarded if larger than input |
| `--strip` | Metadata chunks are stripped from the APNG output |
| `--quality` | Not applicable to APNG (lossless path); ignored silently |
| `--speed` | Not applicable to APNG (lossless path); ignored silently |
| `--floyd` / `--nofs` | Not applicable to APNG; ignored silently |

## Examples

```bash
# Default safe optimization
pngoptim animation.png -o animation-opt.png

# Aggressive with skip-if-larger safety
pngoptim animation.png -o animation-opt.png --apng-mode aggressive --skip-if-larger

# Batch process mixed PNG/APNG files
pngoptim *.png --ext -opt.png
# Static PNGs are quantized; APNG files are losslessly optimized
```
