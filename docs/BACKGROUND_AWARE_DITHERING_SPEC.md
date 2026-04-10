# Background-Aware Dithering for GIF Animation

## 1. Problem Statement

When `pngoptim` is used as the quantization engine for GIF animation (by `gifoptim`), each frame is quantized independently. The quantizer has no knowledge of the previous frame's screen state, causing **pixel churn**: pixels that appear visually unchanged between frames get mapped to slightly different palette indices, defeating frame differencing and inflating file size.

### Real-World Impact

Benchmark against gifski (which uses imagequant's `set_background()`) on 8 animations:

| Test Case | gifoptim vs gifski (default) |
|-----------|------------------------------|
| CG Heart (29 frames) | **+0%** (already matched) |
| Horse (15 frames) | +9% |
| Earth Tilt (24 frames) | +15% |
| Zipper (15 frames) | +41% |
| Earth Slow (44 frames) | +42% |
| Quicksort (70 frames) | +59% |
| Dancer (34 frames) | **+221%** |
| Cradle (48 frames) | **+405%** |

The large gaps (Dancer, Cradle) share a common trait: large areas of slowly-moving or static content, where per-frame independent quantization produces different palette indices for visually identical regions.

## 2. What gifski/imagequant Does

imagequant provides `Image::set_background(background_image)`, used during the **remap phase** (not palette selection). Behavior:

1. For each pixel, if its RGBA is within a threshold of the background RGBA:
   - Prefer the palette index that best matches the background pixel's color
   - Suppress dithering error diffusion (the pixel is "stable", spreading error would damage neighbors)
2. For pixels that differ from the background: normal remapping + full dithering

This is applied **during Floyd-Steinberg dithering**, not as a post-processing step. This matters because:
- Post-processing can't undo dithering artifacts already introduced
- Error diffusion from "stable" pixels contaminates neighboring pixels
- A pre-dither approach prevents the error from being created in the first place

## 3. Proposed API

### 3.1 New Public Function

```rust
/// Quantize with external importance map and background reference.
///
/// `background_rgba`: Previous frame's screen state (RGBA, same dimensions).
/// When provided, the remap/dithering phase biases unchanged pixels toward
/// palette entries matching the background, and suppresses dithering in
/// stable regions. This improves inter-frame coherence for animation.
pub fn quantize_indexed_with_background(
    rgba: &[u8],
    width: usize,
    height: usize,
    settings: QuantizerSettings,
    external_importance_map: Option<&[u8]>,
    background_rgba: Option<&[u8]>,        // NEW: previous frame screen state
) -> IndexedImage
```

The existing `quantize_indexed_with_importance` remains unchanged for backward compatibility.

### 3.2 Internal Data Flow

```
quantize_indexed_with_background()
  ├── build_histogram()          // unchanged
  ├── find_best_palette()        // unchanged
  └── remap_image()              // receives background_rgba
       ├── remap_image_plain()   // unchanged (plain remap doesn't dither)
       └── remap_image_dithered()
            └── dither_row()     // ** CORE CHANGE: background-aware **
```

Only the **dithered remap path** needs modification. The plain path (nearest-neighbor without error diffusion) doesn't need changes because it doesn't create dithering artifacts.

## 4. Core Algorithm Change: `dither_row()`

### 4.1 Current Behavior (per pixel)

```
1. pixel = original[x] + accumulated_error * dither_level
2. (idx, _) = tree.search(pixel, hint)
3. error = pixel - palette[idx]
4. distribute error to neighbors (Floyd-Steinberg 7/16, 3/16, 5/16, 1/16)
```

### 4.2 Proposed Behavior (per pixel)

```
1. pixel = original[x] + accumulated_error * dither_level
2. IF background is provided AND pixel_is_close_to_background(original[x], background[x]):
   a. bg_color = InternalPixel::from_rgba(background[x])
   b. (bg_idx, bg_diff) = tree.search(bg_color, hint)   // find best match for BG
   c. IF bg_diff is small enough:
      - idx = bg_idx                                       // USE background's match
      - error = pixel - palette[bg_idx]
      - distribute error * SUPPRESSION_FACTOR to neighbors  // REDUCE error spread
   d. ELSE:
      - fall through to normal path
3. ELSE (normal path):
   a. (idx, _) = tree.search(pixel, hint)
   b. error = pixel - palette[idx]
   c. distribute error normally
```

### 4.3 Key Parameters

| Parameter | Suggested Value | Purpose |
|-----------|----------------|---------|
| `BG_SIMILARITY_THRESHOLD` | `0.005` (in InternalPixel space, ~3-4 RGB units) | Max distance between source and background to trigger bias |
| `BG_PALETTE_MATCH_THRESHOLD` | `0.02` (in InternalPixel space, ~8 RGB units) | Max distance from background to palette entry to accept bias |
| `DITHER_SUPPRESSION_FACTOR` | `0.25` | Multiply error diffusion by this for background-matched pixels |

These thresholds should be in **perceptual InternalPixel space** (gamma-corrected, channel-weighted), not raw RGB, for consistent behavior across color ranges.

### 4.4 Similarity Check

```rust
/// Check if a source pixel is close enough to its background counterpart
/// to trigger background-aware remapping.
fn pixel_is_close_to_background(
    source: InternalPixel,
    background: InternalPixel,
) -> bool {
    // Both must be opaque (transparent pixels are handled separately)
    if source.a < 0.5 || background.a < 0.5 {
        return false;
    }
    source.diff(background) < BG_SIMILARITY_THRESHOLD
}
```

## 5. Modified Function Signatures

### 5.1 `remap_image()` (internal)

```rust
pub(crate) fn remap_image(
    rgba: &[u8],
    width: usize,
    height: usize,
    palette: &[(InternalPixel, [u8; 4])],
    palette_error: Option<f64>,
    settings: QuantizerSettings,
    importance_map: Option<&[u8]>,
    edges_map: Option<&[u8]>,
    contrast_pixels: Option<&[InternalPixel]>,
    background_rgba: Option<&[u8]>,           // NEW
) -> IndexedImage
```

### 5.2 `remap_image_dithered()` (internal)

```rust
fn remap_image_dithered(
    rgba: &[u8],
    width: usize,
    height: usize,
    palette: &[(InternalPixel, [u8; 4])],
    palette_error: Option<f64>,
    settings: QuantizerSettings,
    importance_map: Option<&[u8]>,
    edges_map: Option<&[u8]>,
    contrast_pixels: Option<&[InternalPixel]>,
    background_pixels: Option<&[InternalPixel]>,  // NEW: pre-converted
) -> (Vec<(InternalPixel, [u8; 4])>, Vec<u8>, Vec<usize>)
```

Note: convert `background_rgba` to `&[InternalPixel]` once in `remap_image()` before passing to dithered path.

### 5.3 `dither_row()` (internal)

```rust
fn dither_row(
    row_pixels: &[InternalPixel],
    output_row: &mut [u8],
    dither_map: &[u8],
    base_dithering_level: f32,
    max_dither_error: f32,
    tree: &NearestTree<'_>,
    palette_points: &[InternalPixel],
    output_image_is_remapped: bool,
    curr_errors: &mut Vec<InternalPixel>,
    next_errors: &mut Vec<InternalPixel>,
    even_row: bool,
    background_row: Option<&[InternalPixel]>,     // NEW
)
```

## 6. gifoptim Integration

Once the API is available, gifoptim's `src/quantize.rs` will call:

```rust
pub(crate) fn quantize_frame(
    image: &ImgVec<RGBA8>,
    quality: u8,
    fast: bool,
    needs_transparency: bool,
    dithering_level: f32,
    importance_map: Option<&[u8]>,
    max_colors_override: Option<usize>,
    background: Option<&[u8]>,          // screen_after_dispose RGBA bytes
) -> (ImgVec<u8>, Vec<RGBA8>) {
    // ...
    let indexed = palette_quant::quantize_indexed_with_background(
        rgba_bytes, width, height, settings,
        importance_map,
        background,
    );
    // ...
}
```

The background RGBA data comes from `screen_after_dispose.pixels_rgba()` in the pipeline's remap stage, converted to `&[u8]` via `rgb::bytemuck::cast_slice()`.

## 7. Testing Strategy

### 7.1 Unit Test: Background Suppresses Dithering

```rust
#[test]
fn background_pixels_get_same_index() {
    // Create a 4x4 image where all pixels match a known background
    let bg = vec![100u8, 150, 200, 255].repeat(16);  // solid blue
    let img = bg.clone();  // identical
    
    let settings = quantizer_settings(256, ...);
    let result = quantize_indexed_with_background(
        &img, 4, 4, settings, None, Some(&bg),
    );
    
    // All pixels should get the same palette index
    assert!(result.indices.iter().all(|&i| i == result.indices[0]));
}
```

### 7.2 Unit Test: Different Pixels Unaffected

```rust
#[test]
fn different_pixels_ignore_background() {
    let bg = vec![100u8, 150, 200, 255].repeat(16);   // solid blue
    let img = vec![200u8, 50, 50, 255].repeat(16);    // solid red
    
    let without_bg = quantize_indexed_with_importance(&img, 4, 4, settings, None);
    let with_bg = quantize_indexed_with_background(&img, 4, 4, settings, None, Some(&bg));
    
    // Results should be identical (background doesn't affect different pixels)
    assert_eq!(without_bg.indices, with_bg.indices);
}
```

### 7.3 Integration: gifoptim T01-T25 Must Still Pass

All existing gifoptim tests should pass with background-aware quantization enabled. The visual quality must not degrade.

## 8. Expected Performance Impact

- **CPU**: Negligible. The per-pixel background check is a single `diff()` comparison (~4 multiplies + additions). The `tree.search()` for background color may be avoided entirely if `bg_idx` is cached from the previous frame.
- **Memory**: One additional `Vec<InternalPixel>` for the background image (~16 bytes/pixel), allocated once per frame.
- **Compression**: Expected 10-30% reduction in default-mode GIF size for animations with large static/slow-moving areas. No impact on single-image quantization (background = None).
