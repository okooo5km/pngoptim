# CLI Options Reference

## Synopsis

```
pngoptim [OPTIONS] INPUT...
```

## Arguments

| Argument | Description |
|----------|-------------|
| `INPUT...` | One or more input PNG files, or `-` for stdin. Required. |

## Options

### Output

| Option | Description | Default |
|--------|-------------|---------|
| `-o, --output FILE` | Output file path, or `-` for stdout. Only valid with a single input. | `<input>-mvp.png` |
| `--ext SUFFIX` | Output filename suffix for batch mode. Applied to each input file's stem. | `-mvp.png` |
| `--force` | Overwrite existing output files without prompting. | Off |
| `--skip-if-larger` | Skip writing if the output file would be larger than the input. | Off |

### Quality Control

| Option | Description | Default |
|--------|-------------|---------|
| `--quality MIN-MAX` | Quality range (0–100). See format table below. | Full range |
| `--speed N` | Speed/quality trade-off (1=slowest/best, 11=fastest). | `4` |

#### Quality Range Formats

| Format | Min | Max | Example |
|--------|-----|-----|---------|
| `MIN-MAX` | MIN | MAX | `65-80` → min=65, max=80 |
| `-MAX` | 0 | MAX | `-80` → min=0, max=80 |
| `MIN-` | MIN | 100 | `65-` → min=65, max=100 |
| `N` | ~90% of N | N | `70` → min=63, max=70 |

If the quantized quality falls below the minimum, pngoptim exits with code **99** without writing the output file.

#### Speed Levels

| Speed | Characteristics |
|-------|-----------------|
| 1–3 | More k-means iterations, finer palette refinement, full dithering |
| 4 | Default balanced mode |
| 5–8 | Fewer iterations, faster processing |
| 9 | Minimal refinement |
| 10–11 | Fastest; dithering automatically disabled |

### Dithering

| Option | Description | Default |
|--------|-------------|---------|
| `--floyd[=N]` | Floyd-Steinberg dithering strength (0.0–1.0). Use `--floyd` for full strength or `--floyd=0.5` for partial. | `1.0` |
| `--nofs` | Disable Floyd-Steinberg dithering entirely. Conflicts with `--floyd`. | Off |

Dithering smooths color transitions by adding controlled noise at palette boundaries. Higher values produce smoother gradients but slightly larger files. At `--speed 10+`, dithering is automatically disabled regardless of this setting.

### Post-processing

| Option | Description | Default |
|--------|-------------|---------|
| `--posterize N` | Posterize output by reducing to N bits (0–8). | Off |
| `--strip` | Strip all metadata chunks (ICC profiles, text, timestamps, etc.). | Off |

### Color Management

| Option | Description | Default |
|--------|-------------|---------|
| `--no-icc` | Skip ICC profile normalization. By default, embedded ICC profiles are converted to sRGB via lcms2. | Off |

### APNG

| Option | Description | Default |
|--------|-------------|---------|
| `--apng-mode MODE` | APNG optimization strategy: `safe` or `aggressive`. See [APNG docs](apng.md) for details. | `safe` |

APNG files are automatically detected via the `acTL` chunk — no special flag is needed.

### Other

| Option | Description | Default |
|--------|-------------|---------|
| `-q, --quiet` | Suppress non-error output. | Off |
| `-V, --version` | Print version information. | — |
| `-h, --help` | Print help message. | — |

## Exit Codes

| Code | Meaning |
|------|---------|
| 0 | Success |
| 2 | Invalid arguments |
| 3 | I/O error |
| 4 | Decode/encode error |
| 98 | Quality below minimum threshold |
| 99 | Output larger than input (with `--skip-if-larger`) |

## Examples

```bash
# Basic quantization
pngoptim photo.png -o photo-opt.png

# High quality with controlled dithering
pngoptim photo.png -o photo-opt.png --quality 80-95 --floyd=0.5

# Fast batch processing
pngoptim *.png --ext -opt.png --speed 8

# Pipeline usage
cat input.png | pngoptim - -o - > output.png

# Conservative: skip if no size benefit
pngoptim photo.png -o photo-opt.png --quality 65-80 --skip-if-larger

# Strip metadata for web delivery
pngoptim icon.png -o icon-opt.png --strip --force

# APNG optimization
pngoptim animated.png -o animated-opt.png --apng-mode aggressive
```
