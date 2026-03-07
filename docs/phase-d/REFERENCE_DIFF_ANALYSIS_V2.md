# Reference Implementation Diff Analysis V2

> Date: 2026-03-07
> Scope: Full module-by-module comparison of pngoptim vs libimagequant
> Author: okooo5km

## Methodology

Each module in `src/palette_quant.rs` was compared line-by-line against the corresponding reference file in `/Users/5km/Dev/C/libimagequant/src/`. Differences were classified as P0 (directly affects demo.png shadow quality), P1 (may affect cross-sample generalization), P2 (APNG or edge cases only).

## Module 1: Histogram (`hist.rs` vs `finalize_histogram`)

| Aspect | Reference | Ours | Status |
|--------|-----------|------|--------|
| `max_perceptual_weight` | `0.1/255 * total_importance` | Same | Aligned |
| Weight calculation | `(importance_sum / 255).min(max_perceptual_weight)` | Same | Aligned |
| Fixed color weight | `max_perceptual_weight * 16` | Not supported | N/A (no fixed colors) |
| Cluster index | `(r>>7)<<3 \| (g>>7)<<2 \| (b>>7)<<1 \| (a>>7)` | Same | Aligned |
| HashMap hasher | fxhash-style multiply | Same `U32HashBuilder` | Aligned |
| Posterize overflow | `requested_bits + 1` max | Same | Aligned |

**Conclusion**: Histogram module is fully aligned for our use case (no fixed colors).

## Module 2: Median Cut (`mediancut.rs` vs `median_cut_palette`)

| Aspect | Reference | Ours | Status |
|--------|-----------|------|--------|
| Representative color | Closest-to-average if len > 2 | Same | Aligned |
| Variance calculation | `(avg - item)^2 * adjusted_weight` per channel | Same | Aligned |
| Sort value | `(primary << 16) \| secondary` | Same | Aligned |
| Split scoring | `weight_sum * variance_sum`, boosted by `max_error/max_mse` | Same | Aligned |
| `prepare_color_weight_total` | `(median.diff(item).sqrt() * (2 + adjusted_weight)).sqrt()` | Same | Aligned |
| `hist_item_sort_half` | Quickselect partition with mc_color_weight | Same | Aligned |
| Split result clamping | `.max(1)` only | `.max(1).min(len-1)` | Minor diff |

**Conclusion**: Mostly aligned. The `.min(len-1)` clamping is a safety guard that shouldn't affect results in practice, but could theoretically change split decisions at boundaries.

## Module 3: K-Means (`kmeans.rs` vs `kmeans_iteration`)

| Aspect | Reference | Ours | Status |
|--------|-----------|------|--------|
| Reflected color | `px + px - remapped` | Same | Aligned |
| Weight adjustment | `2*adj_weight + perceptual_weight) * (0.5 + reflected_diff)` | Same | Aligned |
| Accumulation | Sum weighted colors, divide by total weight | Same | Aligned |
| Finalize skip fixed | `!pop.is_fixed()` filter | No fixed colors | N/A |
| Unused replacement | Find worst-fitting histogram entry via VP-tree | Same | Aligned |
| Parallelization | `par_chunks_mut(256)` with thread-local | Same | Aligned |

**Conclusion**: Fully aligned for our use case.

## Module 4: Remap Plain (`remap.rs::remap_to_palette` vs `remap_image_plain`)

| Aspect | Reference | Ours (before fix) | Ours (after fix) |
|--------|-----------|-------------------|-------------------|
| K-Means finalize | Always runs `kmeans.finalize(palette)` | Only when `generate_dither_map=true` | **Always runs** |
| init_int_palette timing | Before remap (dither=0), after remap (dither>0) | Before remap | **Matched for both paths** |
| importance weight | `f32::from(importance_map.get(col).unwrap_or(1))` | `f64::from(...)` | Same semantics |
| Background support | Full background comparison | Not supported | P2 (APNG) |

**Conclusion**: P0 difference fixed. K-Means finalize now always runs. init_int_palette timing aligned.

## Module 5: Remap Floyd (`remap.rs::remap_to_palette_floyd` vs `remap_image_dithered`)

| Aspect | Reference | Ours (before fix) | Ours (after fix) |
|--------|-----------|-------------------|-------------------|
| K-Means finalize before Floyd | Always (via remap_to_palette) | Only when dither map generated | **Always runs** |
| Output palette timing | After finalize | Before finalize | **After finalize** |
| `guess_from_remapped_pixels` | `output_image_is_remapped && background.is_none()` | `output_image_is_remapped` | Same (no background) |
| Background 3-branch | Full implementation | Not supported | P2 (APNG) |
| `base_dithering_level` | Same formula | Same | Aligned |
| `max_dither_error` | `(palette_error * 2.4).max(quality_to_mse(35))` | Same | Aligned |
| Error diffusion coefficients | 7/16, 3/16, 5/16, 1/16 | Same | Aligned |
| Error clamping | `err *= 0.75` when error > max | Same | Aligned |
| Chunked parallelism | Via scope + spawn | Via `par_chunks_mut` | Equivalent |

**Conclusion**: P0 difference fixed. Background support deferred to Phase H.

## Module 6: Orchestration (`quant.rs` vs `find_best_palette`)

| Aspect | Reference | Ours | Status |
|--------|-----------|------|--------|
| Feedback loop | median_cut -> kmeans_iteration -> compare -> repeat | Same | Aligned |
| `target_mse_overshoot` | `1.05` initial, `*1.25` on success | Same | Aligned |
| Fail penalty | `5 + fails_in_a_row` | Same | Aligned |
| `max_colors` shrink | `min(max_colors, palette.len() + 1)` | Same | Aligned |
| Final refinement | `refine_palette` with iteration limit | Same | Aligned |
| Quality gating | After palette search, before remap | Same | Aligned |

**Conclusion**: Fully aligned.

## Remaining Differences

### demo.png 18 vs 19 colors — ROOT CAUSE IDENTIFIED

**Root cause: ICC color profile handling.**

demo.png contains an Apple Display ICC profile (DM73u pro). pngoptim normalizes ICC to sRGB before quantization (correct behavior), while pngquant quantizes raw Display-profile pixel values and discards the ICC profile (incorrect color management).

**Evidence:**
- With ICC profile: pngoptim → 18 colors, pngquant → 19 colors
- Without ICC profile (stripped): pngoptim → 18 colors, pngquant → **18 colors**
- The algorithms are equivalent; the difference is in input pixel values

The Display profile's tone curve changes the shadow gray distribution, requiring one extra color in pngquant's non-normalized path. After sRGB normalization, 18 colors are sufficient to meet the MSE target (margin ≈ 15%).

**Conclusion:** Not a quantization algorithm bug. Our ICC→sRGB normalization is the correct approach — the output is color-accurate on all standard displays, while pngquant's output is only approximately correct on the original capture display.

### Performance Impact

The K-Means finalize changes add one extra plain remap pass in the dithered path when `generate_dither_map=false`. This increases quantize time for large images but is necessary for algorithm correctness.
