// Copyright (c) 2026 okooo5km(十里). All rights reserved.
// Licensed under the MIT License.

//! Palette construction for opaque photos.
//!
//! The default palette search (median cut + k-means in libimagequant's
//! gamma-weighted ARGB space) is tuned for the general case. On opaque photos
//! with many colors, two textbook alternatives reach a higher 256-color ceiling:
//!
//! - Oklab (Björn Ottosson, 2020) as the space colors are clustered in, since
//!   equal distances there look about equally different, unlike gamma RGB
//!   where dark and saturated colors get too few entries;
//! - Wu's quantizer (Xiaolin Wu, "Efficient Statistical Computations for
//!   Optimal Color Quantization", Graphics Gems II, 1991) as the starting
//!   palette, refined by online k-means (MacQueen 1967, learning rate
//!   count^-1/2), whose large early steps let centers leave poor starting
//!   points that batch k-means keeps.
//!
//! Everything here is opaque-only (three coordinates) and deterministic:
//! transcendental functions come from `libm` (bit-identical on every
//! platform), sampling uses a fixed SplitMix64 sequence, and the only
//! parallel step (color conversion) is a pure per-entry map.

use rayon::prelude::*;

pub(crate) type Point = [f32; 3];

/// Oklab a/b span about ±0.4; this scale puts them on L's 0–255 footing.
const OKLAB_CHROMA_SCALE: f32 = 255.0;

/// Wu histogram resolution per axis: L gets twice the bins of a and b, over
/// the box sRGB colors can reach (a full-range grid wastes most bins on
/// impossible colors).
const WU_BINS: [usize; 3] = [32, 16, 16];
const WU_RANGE: [(f32, f32); 3] = [
    (0.0, 255.0),
    (-0.234 * OKLAB_CHROMA_SCALE, 0.277 * OKLAB_CHROMA_SCALE),
    (-0.312 * OKLAB_CHROMA_SCALE, 0.199 * OKLAB_CHROMA_SCALE),
];

/// Samples drawn by online k-means. pngdeer uses 2^18; on the evaluation
/// photos 2^17 scores the same (−0.04 SSIMULACRA2 on average, within noise)
/// at half the cost, while 2^16 starts to lose (−0.2).
pub(crate) const ONLINE_SAMPLES: usize = 1 << 17;

fn srgb_to_linear(v: u8) -> f32 {
    static TABLE: std::sync::OnceLock<[f32; 256]> = std::sync::OnceLock::new();
    TABLE.get_or_init(|| {
        std::array::from_fn(|i| {
            let s = i as f32 / 255.0;
            if s <= 0.04045 {
                s / 12.92
            } else {
                libm::powf((s + 0.055) / 1.055, 2.4)
            }
        })
    })[v as usize]
}

fn linear_to_srgb(v: f32) -> u8 {
    let v = v.clamp(0.0, 1.0);
    let s = if v <= 0.003_130_8 {
        12.92 * v
    } else {
        1.055 * libm::powf(v, 1.0 / 2.4) - 0.055
    };
    (s * 255.0).round().clamp(0.0, 255.0) as u8
}

// Matrices as published by Ottosson (M1 folded with sRGB → LMS).
#[allow(clippy::excessive_precision)]
pub(crate) fn oklab(rgb: [u8; 3]) -> Point {
    let [r, g, b] = rgb.map(srgb_to_linear);
    let l = libm::cbrtf(0.412_221_470_8 * r + 0.536_332_536_3 * g + 0.051_445_992_9 * b);
    let m = libm::cbrtf(0.211_903_498_2 * r + 0.680_699_545_1 * g + 0.107_396_956_6 * b);
    let s = libm::cbrtf(0.088_302_461_9 * r + 0.281_718_837_6 * g + 0.629_978_700_5 * b);
    [
        (0.210_454_255_3 * l + 0.793_617_785_0 * m - 0.004_072_046_8 * s) * 255.0,
        (1.977_998_495_1 * l - 2.428_592_205_0 * m + 0.450_593_709_9 * s) * OKLAB_CHROMA_SCALE,
        (0.025_904_037_1 * l + 0.782_771_766_2 * m - 0.808_675_766_0 * s) * OKLAB_CHROMA_SCALE,
    ]
}

#[allow(clippy::excessive_precision)]
pub(crate) fn oklab_to_rgb(p: Point) -> [u8; 3] {
    let (lab_l, lab_a, lab_b) = (
        p[0] / 255.0,
        p[1] / OKLAB_CHROMA_SCALE,
        p[2] / OKLAB_CHROMA_SCALE,
    );
    let cube = |x: f32| x * x * x;
    let l = cube(lab_l + 0.396_337_777_4 * lab_a + 0.215_803_757_3 * lab_b);
    let m = cube(lab_l - 0.105_561_345_8 * lab_a - 0.063_854_172_8 * lab_b);
    let s = cube(lab_l - 0.089_484_177_5 * lab_a - 1.291_485_548_0 * lab_b);
    [
        linear_to_srgb(4.076_741_662_1 * l - 3.307_711_591_3 * m + 0.230_969_929_2 * s),
        linear_to_srgb(-1.268_438_004_6 * l + 2.609_757_401_1 * m - 0.341_319_396_5 * s),
        linear_to_srgb(-0.004_196_086_3 * l - 0.703_418_614_7 * m + 1.707_614_701_0 * s),
    ]
}

fn dist(p: &Point, q: &Point) -> f32 {
    let d0 = p[0] - q[0];
    let d1 = p[1] - q[1];
    let d2 = p[2] - q[2];
    d0 * d0 + d1 * d1 + d2 * d2
}

/// Histogram colors (opaque RGB + weight) as Oklab points, in input order.
pub(crate) fn entries(colors: &[([u8; 3], f32)]) -> Vec<(Point, f32)> {
    colors.par_iter().map(|&(rgb, w)| (oklab(rgb), w)).collect()
}

/// Weighted moments of a set of points: count, per-axis sum, sum of squares.
#[derive(Clone, Copy, Default)]
struct Moments {
    w: f64,
    m: [f64; 3],
    q: f64,
}

impl Moments {
    fn add(&mut self, o: &Moments) {
        self.w += o.w;
        self.m[0] += o.m[0];
        self.m[1] += o.m[1];
        self.m[2] += o.m[2];
        self.q += o.q;
    }

    fn sub(&mut self, o: &Moments) {
        self.w -= o.w;
        self.m[0] -= o.m[0];
        self.m[1] -= o.m[1];
        self.m[2] -= o.m[2];
        self.q -= o.q;
    }

    /// |Σx|² / Σw — the part of Σ|x|² explained by the mean. Splits maximize it.
    fn explained(&self) -> f64 {
        (self.m[0] * self.m[0] + self.m[1] * self.m[1] + self.m[2] * self.m[2]) / self.w
    }

    fn sse(&self) -> f64 {
        if self.w > 0.0 {
            (self.q - self.explained()).max(0.0)
        } else {
            0.0
        }
    }
}

/// Wu's quantizer: bin the weighted points on a fixed grid, keep 3-D prefix
/// sums of their moments so any box's moments cost eight lookups, then
/// repeatedly split the box with the largest squared error at the axis/plane
/// that leaves the two halves the least total error. Returns up to `k` box
/// means.
pub(crate) fn wu(entries: &[(Point, f32)], k: usize) -> Vec<Point> {
    let (bins, ranges) = (WU_BINS, WU_RANGE);
    let n = bins.map(|b| b + 1);
    let at = |i: usize, j: usize, l: usize| (i * n[1] + j) * n[2] + l;
    let bin_of = |v: f32, axis: usize| {
        let (lo, hi) = ranges[axis];
        let x = ((v - lo) / (hi - lo) * bins[axis] as f32).max(0.0) as usize;
        x.min(bins[axis] - 1) + 1
    };

    let mut cells = vec![Moments::default(); n[0] * n[1] * n[2]];
    for (p, w) in entries {
        let w = f64::from(*w);
        let c = &mut cells[at(bin_of(p[0], 0), bin_of(p[1], 1), bin_of(p[2], 2))];
        c.w += w;
        let x = p.map(f64::from);
        c.m[0] += w * x[0];
        c.m[1] += w * x[1];
        c.m[2] += w * x[2];
        c.q += w * (x[0] * x[0] + x[1] * x[1] + x[2] * x[2]);
    }
    // Prefix sums axis by axis: cells[i][j][l] becomes the moments of (0,i]×(0,j]×(0,l].
    for i in 1..n[0] {
        for j in 0..n[1] {
            for l in 0..n[2] {
                let prev = cells[at(i - 1, j, l)];
                cells[at(i, j, l)].add(&prev);
            }
        }
    }
    for i in 0..n[0] {
        for j in 1..n[1] {
            for l in 0..n[2] {
                let prev = cells[at(i, j - 1, l)];
                cells[at(i, j, l)].add(&prev);
            }
        }
    }
    for i in 0..n[0] {
        for j in 0..n[1] {
            for l in 1..n[2] {
                let prev = cells[at(i, j, l - 1)];
                cells[at(i, j, l)].add(&prev);
            }
        }
    }

    // A box is a half-open range (lo, hi] of prefix indices per axis.
    type WuBox = [[usize; 2]; 3];
    let moments = |b: &WuBox| {
        let mut out = Moments::default();
        for corner in 0..8usize {
            let i = b[0][corner & 1];
            let j = b[1][(corner >> 1) & 1];
            let l = b[2][(corner >> 2) & 1];
            // Inclusion–exclusion: corners with an odd number of lower bounds subtract.
            let lower = 3 - (corner.count_ones() as usize);
            if lower.is_multiple_of(2) {
                out.add(&cells[at(i, j, l)]);
            } else {
                out.sub(&cells[at(i, j, l)]);
            }
        }
        out
    };

    let mut boxes: Vec<WuBox> = vec![[[0, bins[0]], [0, bins[1]], [0, bins[2]]]];
    let mut errors = vec![moments(&boxes[0]).sse()];
    while boxes.len() < k {
        let Some(target) = (0..boxes.len())
            .filter(|&i| errors[i] > 0.0)
            .max_by(|&a, &b| errors[a].total_cmp(&errors[b]).then(b.cmp(&a)))
        else {
            break;
        };
        let whole = moments(&boxes[target]);
        let mut best: Option<(f64, usize, usize)> = None;
        for axis in 0..3 {
            let [lo, hi] = boxes[target][axis];
            for cut in lo + 1..hi {
                let mut half = boxes[target];
                half[axis][1] = cut;
                let first = moments(&half);
                let mut rest = whole;
                rest.sub(&first);
                if first.w <= 0.0 || rest.w <= 0.0 {
                    continue;
                }
                let gain = first.explained() + rest.explained();
                if best.is_none_or(|(g, _, _)| gain > g) {
                    best = Some((gain, axis, cut));
                }
            }
        }
        let Some((_, axis, cut)) = best else {
            errors[target] = 0.0;
            continue;
        };
        let mut upper = boxes[target];
        boxes[target][axis][1] = cut;
        upper[axis][0] = cut;
        errors[target] = moments(&boxes[target]).sse();
        errors.push(moments(&upper).sse());
        boxes.push(upper);
    }

    boxes
        .iter()
        .map(moments)
        .filter(|m| m.w > 0.0)
        .map(|m| m.m.map(|s| (s / m.w) as f32))
        .collect()
}

/// SplitMix64 (Steele, Lea & Flood 2014): a fixed, platform-independent sequence.
struct SplitMix64(u64);

impl SplitMix64 {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform index below `n` (multiply-shift).
    #[cfg(test)]
    fn below(&mut self, n: usize) -> usize {
        (((self.next() >> 32) * n as u64) >> 32) as usize
    }
}

/// Online k-means drawing samples from weighted entries (probability
/// proportional to weight) instead of uniformly from pixels, so histogram
/// importance (contrast map, local-error boost) steers the centers too.
pub(crate) fn online_kmeans_weighted(
    entries: &[(Point, f32)],
    centers: &mut [Point],
    samples: usize,
) -> Vec<u32> {
    let mut cumulative = Vec::with_capacity(entries.len());
    let mut total = 0.0f64;
    for (_, w) in entries {
        total += f64::from(*w);
        cumulative.push(total);
    }
    if entries.is_empty() || total <= 0.0 {
        return vec![0; centers.len()];
    }
    online_kmeans_with(centers, samples, |rng| {
        // 53 random bits → uniform in [0, total).
        let u = (rng.next() >> 11) as f64 / (1u64 << 53) as f64 * total;
        let i = cumulative
            .partition_point(|&c| c <= u)
            .min(entries.len() - 1);
        entries[i].0
    })
}

/// Online (MacQueen) k-means: each sample from `draw` (a fixed SplitMix64
/// sequence) pulls its nearest center count^-1/2 of the way toward itself.
/// Nearest centers are found exactly by walking outward in lightness order;
/// the order is repaired by local swaps after each move, since a center only
/// moves a little per step. Returns how many samples each center took.
fn online_kmeans_with(
    centers: &mut [Point],
    samples: usize,
    mut draw: impl FnMut(&mut SplitMix64) -> Point,
) -> Vec<u32> {
    let mut counts = vec![0u32; centers.len()];
    if centers.is_empty() {
        return counts;
    }
    // Centers ordered by lightness; `slot[c]` is center c's position in it.
    let mut order: Vec<usize> = (0..centers.len()).collect();
    order.sort_by(|&a, &b| centers[a][0].total_cmp(&centers[b][0]).then(a.cmp(&b)));
    let mut keys: Vec<f32> = order.iter().map(|&c| centers[c][0]).collect();
    let mut slot = vec![0usize; centers.len()];
    for (s, &c) in order.iter().enumerate() {
        slot[c] = s;
    }
    let mut rng = SplitMix64(0);
    for _ in 0..samples {
        let p = draw(&mut rng);
        let start = keys.partition_point(|&e| e < p[0]);
        let (mut lo, mut hi) = (start, start);
        let mut best = usize::MAX;
        let mut best_d = f32::MAX;
        // |ΔL|² bounds the distance from below: walk outward until neither
        // side can beat the best so far.
        loop {
            let below = lo > 0 && {
                let dl = p[0] - keys[lo - 1];
                dl * dl < best_d
            };
            let above = hi < keys.len() && {
                let dl = keys[hi] - p[0];
                dl * dl < best_d
            };
            if !below && !above {
                break;
            }
            if below {
                lo -= 1;
                let c = order[lo];
                let d = dist(&p, &centers[c]);
                if d < best_d || (d == best_d && c < best) {
                    best = c;
                    best_d = d;
                }
            }
            if above {
                let c = order[hi];
                let d = dist(&p, &centers[c]);
                if d < best_d || (d == best_d && c < best) {
                    best = c;
                    best_d = d;
                }
                hi += 1;
            }
        }
        counts[best] += 1;
        let rate = 1.0 / (counts[best] as f32).sqrt();
        let center = &mut centers[best];
        for axis in 0..3 {
            center[axis] += rate * (p[axis] - center[axis]);
        }
        // Repair the lightness order around the moved center.
        let mut s = slot[best];
        keys[s] = center[0];
        while s > 0 && keys[s - 1] > keys[s] {
            order.swap(s - 1, s);
            keys.swap(s - 1, s);
            slot[order[s]] = s;
            s -= 1;
        }
        while s + 1 < order.len() && keys[s + 1] < keys[s] {
            order.swap(s + 1, s);
            keys.swap(s + 1, s);
            slot[order[s]] = s;
            s += 1;
        }
        slot[best] = s;
    }
    counts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oklab_round_trip_is_close() {
        for r in (0..=255).step_by(17) {
            for g in (0..=255).step_by(15) {
                for b in (0..=255).step_by(51) {
                    let rgb = [r as u8, g as u8, b as u8];
                    let back = oklab_to_rgb(oklab(rgb));
                    for k in 0..3 {
                        assert!(rgb[k].abs_diff(back[k]) <= 1, "{rgb:?} -> {back:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn oklab_grid_covers_srgb() {
        let ranges = WU_RANGE;
        for rgb in [
            [255, 0, 0],
            [0, 255, 0],
            [0, 0, 255],
            [255, 255, 0],
            [255, 0, 255],
        ] {
            let p = oklab(rgb);
            for axis in 0..3 {
                assert!(p[axis] >= ranges[axis].0 - 1.0 && p[axis] <= ranges[axis].1 + 1.0);
            }
        }
    }

    #[test]
    fn online_kmeans_matches_brute_force() {
        let rgba: Vec<u8> = (0..4096u32)
            .flat_map(|i| {
                [
                    (i * 37 % 256) as u8,
                    (i * 91 % 256) as u8,
                    (i * 13 % 256) as u8,
                    255,
                ]
            })
            .collect();
        let entries: Vec<(Point, f32)> = rgba
            .chunks(4)
            .map(|p| (oklab([p[0], p[1], p[2]]), 1.0))
            .collect();
        let start = wu(&entries, 32);
        let mut fast = start.clone();
        online_kmeans_with(&mut fast, 5000, |rng| entries[rng.below(entries.len())].0);
        // Reference: same sequence, brute-force nearest.
        let mut brute = start;
        let mut counts = vec![0u32; brute.len()];
        let mut rng = SplitMix64(0);
        for _ in 0..5000 {
            let p = entries[rng.below(entries.len())].0;
            let best = (0..brute.len())
                .min_by(|&a, &b| dist(&p, &brute[a]).total_cmp(&dist(&p, &brute[b])))
                .unwrap();
            counts[best] += 1;
            let rate = 1.0 / (counts[best] as f32).sqrt();
            for axis in 0..3 {
                brute[best][axis] += rate * (p[axis] - brute[best][axis]);
            }
        }
        assert_eq!(fast, brute);
    }

    #[test]
    fn wu_separates_distinct_clusters() {
        let colors = [
            [250u8, 10, 10],
            [10, 250, 10],
            [10, 10, 250],
            [128, 128, 128],
        ];
        let entries: Vec<(Point, f32)> = colors.iter().map(|&c| (oklab(c), 10.0)).collect();
        let centers = wu(&entries, 4);
        assert_eq!(centers.len(), 4);
        for (p, _) in &entries {
            assert!(centers.iter().any(|c| dist(c, p) < 1e-3));
        }
    }
}
