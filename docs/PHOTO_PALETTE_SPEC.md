# Opaque Photo Palette (Oklab + Wu)

## 1. Problem Statement

Measured against pngdeer (MIT, github.com/heydeer/pngdeer) with the shared SSIMULACRA2 evaluation kit,
the highest score pngoptim can reach at 256 colors on opaque photos is 2–5 points lower
(kodim03 +4.9, kodim05 +5.1, test_10 +6.9, test_9 +2.9 for pngdeer). The gap comes from palette
construction, not from encoding or dithering.

## 2. Differences: pngoptim (libimagequant) vs pngdeer

| Aspect | pngoptim (libimagequant style) | pngdeer `palette.rs` / `color.rs` |
|--------|--------------------------------|-----------------------------------|
| Color space | `InternalPixel`: ARGB, channels `gamma^0.57`, weighted A 0.625 / R 0.5 / G 1.0 / B 0.45, alpha-premultiplied; distance = max over black/white background | Oklab (linear sRGB → LMS → cbrt), L × 255, a/b × 255; translucent pixels as two composites (gray 24 / 232), 6 coordinates |
| Histogram | Exact RGBA keys, weight = contrast importance map (capped), posterized when > `max_histogram_entries` | Exact RGBA keys, weight = pixel count (optionally error-feedback map); merged by dropping low RGB bits past 2^18 entries |
| Initial palette | Median cut on variance-weighted channels, split box chosen by `total_error`/variance with target-MSE early stop; feedback loop of up to `feedback_loop_trials` median cut + k-means passes with "reflected color" weight adjustment | (a) Variance median cut splitting at the SSE-minimizing position; (b) Wu's quantizer on a fixed 32×16×16 Oklab grid over the sRGB gamut box (opaque images only) |
| Refinement | Batch k-means (`kmeans_iteration`) until error delta < limit, plus one more k-means step inside remap (`finalize_plain_remap`) | (a) Weighted Lloyd, 5–8 rounds; (b) online k-means (MacQueen), 2^18 pixels sampled with fixed SplitMix64, learning rate count^-1/2 |
| Color count | Driven by target MSE (`--quality max`) inside median cut | Picked by an outer search on SSIMULACRA2 |
| Remap | Gamma ARGB nearest (VP-tree), selective Floyd | Oklab nearest, own dithering |
| Determinism | Not thread-count independent today (rayon `fold/reduce` float sums in k-means, histogram `HashMap` order, dither chunk count = thread count) | Fixed chunks merged in order; `libm` for transcendental functions (native == WASM bit-identical) |

## 3. Attribution Experiments

Set G = photo-like opaque images the gate admits (anime-girl, kodim01/03/05/07/08/13, test_2/7/8/9/10);
set U = opaque UI screenshots (test_3/4/6). Size = geometric mean of equal-SSIMULACRA2 size ratio vs
the pre-change binary; ceiling = mean change of the best score reachable at 256 colors.

| Variant | G size | G ceiling | U size | U ceiling |
|---------|-------:|----------:|-------:|----------:|
| Control: internal space, variance median cut + Lloyd | 93.3% | +2.16 | 94.9% | +0.10 |
| ① Oklab only (median cut + Lloyd in Oklab) | 91.7% | +2.43 | 101.7% | +0.34 |
| ② Wu + online k-means only (internal space) | 91.3% | +2.41 | 97.7% | −1.50 |
| ③ Wu + online k-means in Oklab, pixel-uniform samples | 89.9% | +2.97 | 102.6% | −2.81 |
| ③ with batch Lloyd instead of online k-means | 91.5% | +2.64 | 109.3% | −3.25 |
| **③ with histogram-weighted samples (chosen)** | **90.2%** | **+3.03** | 99.4% | −2.65 |

Findings:

- On kodim most of the gain comes from the algorithm (Wu start + online k-means); Oklab adds on
  large photos and mixed screenshots (test_8: 83.6% with ③ vs 96.8% for the internal-space control).
- Online k-means beats batch Lloyd from the same start: the per-sample updates escape the
  local minimum Lloyd settles in.
- Sampling histogram entries by their importance weight (instead of uniform pixels) keeps the
  contrast importance map and the local-error feedback boost effective, for the same score.
- Wu on a fixed grid hurts UI screenshots: exact flat colors that share a cell get merged.
  This is what the gate below excludes.

## 4. Decision and Integration

- `palette_quant::photo_palette` runs after the normal `find_best_palette` search when
  `QuantizerSettings::photo_palette` is set (default on in `quantizer_settings`).
- Gate: every histogram entry opaque, at least 4096 distinct colors, and either ≤100 pixels per
  color or the 16 most important colors hold ≤25% of the weight (UI: 266–4138 px/color,
  top-16 share 0.52–0.93; test_8 photo: 182 px/color but share 0.055).
- Guard: the photo palette is used only if its histogram error is ≤1.5× the default palette's and
  ≤ the `--quality max` target MSE.
- Quality gate semantics are unchanged: `--quality` is still checked on the final output in
  libimagequant's internal space (`quality_to_mse`). Because the photo palette minimizes Oklab
  error, it can land a few points lower in that metric; when it misses the minimum by ≤3 points
  the pipeline re-runs with the default palette and keeps the lower-MSE result
  (anime-girl `--quality 80-90`: 79 → fallback 81, same output as before).
- APNG keeps the default global palette (photo palette is not used there).
- Wu grid: 32×16×16 over the Oklab box of the sRGB gamut; online k-means: 2^17 samples
  (pngdeer uses 2^18; 2^17 scored the same at half the cost, 2^16 lost 0.2), SplitMix64 fixed seed,
  rate count^-1/2, exact nearest via a lightness-ordered walk.

## 5. Results (vs baseline, eval kit, SSIMULACRA2)

| Category | Equal-quality size (median / geomean) | Ceiling | Spot 65-75 bytes / score | Spot 80-90 bytes / score |
|----------|---------------------------------------|--------:|-------------------------|-------------------------|
| photos | 95.5% / 93.9% | +1.61 | −3.3% / +1.73 | −6.1% / +1.97 |
| test | 97.5% / 92.8% | +2.43 | −1.5% / +1.02 | −1.0% / +0.85 |
| illustrations | — / 98.2% | +0.39 | | |
| 3d | 100.1% | −0.07 (noise) | | |
| icons, photos-alpha, UI | 100% | 0 | unchanged | unchanged |

All 40 images: geomean 97.0%. Per image (size / ceiling): anime 89.6% / +2.34, kodim01 89.2% / +0.23,
kodim03 94.5% / +2.06, kodim05 96.7% / +2.72, kodim07 86.2% / +2.68, kodim08 96.5% / +0.90,
kodim13 101.0% / +1.07, test_2 76.8% / +8.43, test_7 91.6% / +2.72, test_8 84.4% / +4.41,
test_9 95.0% / +1.97, test_10 83.2% / +6.82.

## 6. Visual Check

- kodim01/03/05/07: fewer dither speckles at equal size, no hue shift, no banding.
- Smooth skies (test_2, test_10): a slight warm cast where the sky gets fewer palette entries
  (9 vs 18 in a test_2 crop): mean R +3 / B −2.4, ΔE_ok 0.0048 (max ~0.0075), well below the
  ~0.02 Oklab JND; invisible at normal viewing. Extra Lloyd rounds did not change it.

## 7. Cost

Back-to-back under the eval lock, min of 5, 16 images × {65-75, 0-100}: wall 21.80 → 25.34 s
(+16%), CPU 61.42 → 66.77 s (+9%). Per kodim ~+35 ms (online k-means ~25–30 ms, Oklab entries
~6 ms, Wu < 3 ms, error check 2–4 ms). Some images take one more local-error-feedback round
because the palette's error distribution changes (kodim13 at 65-75: 1 → 2 passes, 68 → 171 ms).
A SoA brute-force nearest search was only ~25% faster than the walk and was not adopted.
UI and translucent images: unchanged.

## 8. Determinism

Histogram chunking, k-means accumulation and dither chunking now use fixed chunk counts
merged in order, and Oklab uses `libm` (`powf`, `cbrtf`), so output is identical for any
`RAYON_NUM_THREADS` (checked 1/3/8 threads, dithered and `--nofs`, 7 images × 2 quality ranges)
and does not depend on the platform's libm.
