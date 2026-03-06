use std::cmp::Ordering;
use std::collections::HashMap;

use crate::quality::{
    DitherMapMode, InternalPixel, SRGB_OUTPUT_GAMMA, SpeedSettings, gamma_lut, quality_to_mse,
};

#[derive(Debug, Clone)]
pub struct IndexedImage {
    pub palette: Vec<[u8; 4]>,
    pub indices: Vec<u8>,
}

#[derive(Debug, Clone, Copy)]
pub struct QuantizerSettings {
    pub max_colors: usize,
    pub input_posterize_bits: u8,
    pub max_histogram_entries: u32,
    pub kmeans_iterations: u16,
    pub feedback_loop_trials: u16,
    pub target_mse: Option<f64>,
    pub dither: bool,
    pub use_dither_map: DitherMapMode,
}

pub fn quantize_indexed(
    rgba: &[u8],
    width: usize,
    height: usize,
    settings: QuantizerSettings,
) -> IndexedImage {
    let pixel_count = width.saturating_mul(height);
    if pixel_count == 0 {
        return IndexedImage {
            palette: vec![[0, 0, 0, 0]],
            indices: Vec::new(),
        };
    }

    let gamma = gamma_lut(SRGB_OUTPUT_GAMMA);
    let histogram = build_histogram(
        rgba,
        settings.input_posterize_bits,
        settings.max_histogram_entries,
        &gamma,
    );

    let mut palette = find_best_palette(&histogram, settings);
    if palette.is_empty() {
        palette = vec![InternalPixel::default()];
    }
    if histogram.len() <= 4_096 {
        refine_palette_from_pixels(rgba, &mut palette, settings.input_posterize_bits, 1);
    }

    let final_palette = dedup_palette(&palette);
    remap_image(rgba, width, height, &final_palette, settings)
}

pub fn max_colors_from_quality_speed(quality_target: u8, speed: u8) -> usize {
    let quality_component = 16 + (usize::from(quality_target) * 180 / 100);
    let speed_penalty = usize::from(speed.saturating_sub(1)) * 14;
    quality_component
        .saturating_sub(speed_penalty)
        .clamp(16, 256)
}

pub fn quantizer_settings(
    max_colors: usize,
    speed: SpeedSettings,
    target_mse: Option<f64>,
    dither: bool,
) -> QuantizerSettings {
    QuantizerSettings {
        max_colors,
        input_posterize_bits: speed.input_posterize_bits,
        max_histogram_entries: speed.max_histogram_entries,
        kmeans_iterations: speed.kmeans_iterations,
        feedback_loop_trials: speed.feedback_loop_trials,
        target_mse,
        dither,
        use_dither_map: speed.use_dither_map,
    }
}

#[derive(Debug, Clone, Copy)]
struct HistItem {
    color: InternalPixel,
    weight: f32,
    adjusted_weight: f32,
    likely_palette_index: u16,
}

#[derive(Default)]
struct HistAccumulator {
    count: u32,
    sum_a: f64,
    sum_r: f64,
    sum_g: f64,
    sum_b: f64,
}

#[derive(Debug, Clone, Copy)]
struct ColorBox {
    start: usize,
    end: usize,
    average: InternalPixel,
    total_weight: f64,
    variance: [f64; 4],
}

impl ColorBox {
    fn new(items: &[HistItem], start: usize, end: usize) -> Option<Self> {
        if start >= end || end > items.len() {
            return None;
        }

        let mut total_weight = 0.0f64;
        let mut sum_a = 0.0f64;
        let mut sum_r = 0.0f64;
        let mut sum_g = 0.0f64;
        let mut sum_b = 0.0f64;

        for item in &items[start..end] {
            let weight = f64::from(item.adjusted_weight);
            total_weight += weight;
            sum_a += f64::from(item.color.a) * weight;
            sum_r += f64::from(item.color.r) * weight;
            sum_g += f64::from(item.color.g) * weight;
            sum_b += f64::from(item.color.b) * weight;
        }

        if total_weight == 0.0 {
            return None;
        }

        let average = InternalPixel {
            a: (sum_a / total_weight) as f32,
            r: (sum_r / total_weight) as f32,
            g: (sum_g / total_weight) as f32,
            b: (sum_b / total_weight) as f32,
        };

        let mut variance = [0.0; 4];
        for item in &items[start..end] {
            let weight = f64::from(item.adjusted_weight);
            variance[0] += f64::from((item.color.a - average.a).powi(2)) * weight;
            variance[1] += f64::from((item.color.r - average.r).powi(2)) * weight;
            variance[2] += f64::from((item.color.g - average.g).powi(2)) * weight;
            variance[3] += f64::from((item.color.b - average.b).powi(2)) * weight;
        }

        Some(Self {
            start,
            end,
            average,
            total_weight,
            variance,
        })
    }

    fn len(self) -> usize {
        self.end - self.start
    }

    fn dominant_channel(self) -> usize {
        self.variance
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(Ordering::Equal))
            .map(|(idx, _)| idx)
            .unwrap_or(0)
    }

    fn score(self) -> f64 {
        self.total_weight * self.variance[self.dominant_channel()]
    }
}

fn build_histogram(
    rgba: &[u8],
    initial_posterize_bits: u8,
    max_histogram_entries: u32,
    gamma: &[f32; 256],
) -> Vec<HistItem> {
    let mut bits = initial_posterize_bits.min(4);
    loop {
        let mut map: HashMap<u32, HistAccumulator> = HashMap::new();
        for px in rgba.chunks_exact(4) {
            let key = pack_rgba_key(px, bits);
            let entry = map.entry(key).or_default();
            let posterized = posterized_rgba(px, bits);
            let color = InternalPixel::from_rgba(gamma, &posterized);
            entry.count = entry.count.saturating_add(1);
            entry.sum_a += f64::from(color.a);
            entry.sum_r += f64::from(color.r);
            entry.sum_g += f64::from(color.g);
            entry.sum_b += f64::from(color.b);
        }

        if map.len() <= max_histogram_entries as usize || bits >= 4 {
            return finalize_histogram(map);
        }
        bits += 1;
    }
}

fn finalize_histogram(map: HashMap<u32, HistAccumulator>) -> Vec<HistItem> {
    let total_count = map.values().map(|item| u64::from(item.count)).sum::<u64>() as f32;
    let max_weight = ((0.1 / 255.0) * f64::from(total_count)) as f32;

    let mut out = map
        .into_values()
        .filter_map(|acc| {
            if acc.count == 0 {
                return None;
            }
            let inv = 1.0 / f64::from(acc.count);
            let color = InternalPixel {
                a: (acc.sum_a * inv) as f32,
                r: (acc.sum_r * inv) as f32,
                g: (acc.sum_g * inv) as f32,
                b: (acc.sum_b * inv) as f32,
            };
            let weight = ((acc.count as f32) / 255.0).min(max_weight.max(1.0 / 255.0));
            Some(HistItem {
                color,
                weight,
                adjusted_weight: weight,
                likely_palette_index: 0,
            })
        })
        .collect::<Vec<_>>();

    out.sort_by(|a, b| b.weight.partial_cmp(&a.weight).unwrap_or(Ordering::Equal));
    out
}

fn median_cut_palette(histogram: &mut [HistItem], target_colors: usize) -> Vec<InternalPixel> {
    let items = histogram;
    let mut boxes = vec![ColorBox::new(items, 0, items.len()).expect("non-empty histogram")];

    while boxes.len() < target_colors {
        let Some((box_index, selected)) = boxes
            .iter()
            .copied()
            .enumerate()
            .filter(|(_, b)| b.len() > 1)
            .max_by(|(_, a), (_, b)| a.score().partial_cmp(&b.score()).unwrap_or(Ordering::Equal))
        else {
            break;
        };

        let channel = selected.dominant_channel();
        items[selected.start..selected.end].sort_by(|a, b| {
            channel_value(a.color, channel)
                .partial_cmp(&channel_value(b.color, channel))
                .unwrap_or(Ordering::Equal)
        });

        let mut cumulative = 0.0f64;
        let half = selected.total_weight / 2.0;
        let mut split = selected.start + 1;
        for idx in selected.start..selected.end - 1 {
            cumulative += f64::from(items[idx].adjusted_weight);
            if cumulative >= half {
                split = idx + 1;
                break;
            }
        }
        split = split.clamp(selected.start + 1, selected.end - 1);

        let left = ColorBox::new(&items, selected.start, split);
        let right = ColorBox::new(&items, split, selected.end);
        let (Some(left), Some(right)) = (left, right) else {
            break;
        };

        boxes.swap_remove(box_index);
        boxes.push(left);
        boxes.push(right);
    }

    boxes.iter().map(|b| b.average).collect()
}

fn find_best_palette(histogram: &[HistItem], settings: QuantizerSettings) -> Vec<InternalPixel> {
    if histogram.is_empty() {
        return vec![InternalPixel::default()];
    }

    let max_colors = settings.max_colors.clamp(2, 256);
    let mut hist = histogram.to_vec();
    let hist_items = hist.len();
    let total_trials = effective_feedback_trials(settings.feedback_loop_trials, hist_items);
    let trial_iterations = trial_kmeans_iterations(settings.kmeans_iterations, hist_items);
    let final_iterations = final_kmeans_iterations(settings.kmeans_iterations, hist_items);

    if hist.len() <= max_colors || total_trials <= 1 || settings.target_mse.is_none() {
        let mut palette = if hist.len() <= max_colors {
            hist.iter().map(|item| item.color).collect::<Vec<_>>()
        } else {
            median_cut_palette(&mut hist, max_colors)
        };
        let _ = refine_palette(&mut hist, &mut palette, final_iterations, false);
        return palette;
    }

    let target_mse = settings.target_mse.unwrap_or(f64::INFINITY);
    let mut current_max_colors = max_colors;
    let mut trials_left = total_trials as i32;
    let mut best_palette = None;
    let mut best_error = f64::INFINITY;
    let mut best_used_colors = usize::MAX;
    let mut fails_in_a_row = 0i32;

    while trials_left > 0 && current_max_colors >= 2 {
        let mut palette = median_cut_palette(&mut hist, current_max_colors);
        let first_target_run = best_palette.is_none();
        let stats = refine_palette(&mut hist, &mut palette, trial_iterations, !first_target_run);
        let used_colors = stats.used_colors.max(2).min(palette.len());
        let better = best_palette.is_none()
            || stats.error < best_error
            || (stats.error <= target_mse && used_colors < best_used_colors);

        if better {
            best_error = stats.error;
            best_used_colors = used_colors;
            best_palette = Some(palette);
            fails_in_a_row = 0;
            trials_left -= 1;

            if stats.error <= target_mse && current_max_colors > 2 {
                current_max_colors = current_max_colors
                    .min(used_colors.saturating_add(1))
                    .saturating_sub(1)
                    .max(2);
            }
        } else {
            fails_in_a_row += 1;
            trials_left -= 1 + fails_in_a_row.min(2);
        }
    }

    let mut palette = best_palette.unwrap_or_else(|| median_cut_palette(&mut hist, max_colors));
    let mut final_hist = histogram.to_vec();
    let _ = refine_palette(&mut final_hist, &mut palette, final_iterations, false);
    palette
}

#[derive(Debug, Clone, Copy)]
struct PaletteStats {
    error: f64,
    used_colors: usize,
}

fn refine_palette(
    histogram: &mut [HistItem],
    palette: &mut [InternalPixel],
    iterations: u16,
    adjust_weights: bool,
) -> PaletteStats {
    if palette.is_empty() {
        return PaletteStats {
            error: 0.0,
            used_colors: 0,
        };
    }

    let mut stats = kmeans_iteration(histogram, palette, adjust_weights);
    let iteration_limit = 1e-7;
    for _ in 0..iterations.saturating_sub(1) {
        let previous_error = stats.error;
        stats = kmeans_iteration(histogram, palette, false);
        if (previous_error - stats.error).abs() < iteration_limit {
            break;
        }
    }
    stats
}

fn kmeans_iteration(
    histogram: &mut [HistItem],
    palette: &mut [InternalPixel],
    adjust_weights: bool,
) -> PaletteStats {
    let tree = NearestTree::new(palette);
    let mut sums = vec![[0.0f64; 4]; palette.len()];
    let mut weights = vec![0.0f64; palette.len()];
    let mut usage = vec![0usize; palette.len()];
    let total_weight = histogram
        .iter()
        .map(|item| f64::from(item.weight))
        .sum::<f64>()
        .max(1e-9);
    let mut total_error = 0.0f64;

    for item in histogram.iter_mut() {
        let hint = usize::from(item.likely_palette_index).min(palette.len().saturating_sub(1));
        let (nearest, diff_sq) = tree.search(item.color, hint);
        let diff = f64::from(diff_sq);
        item.likely_palette_index = nearest as u16;
        total_error += diff * f64::from(item.weight);

        let weight = if adjust_weights {
            let reflected = reflected_color(item.color, palette[nearest]);
            let reflected_diff = f64::from(tree.search(reflected, nearest).1);
            let adjusted = (2.0 * f64::from(item.adjusted_weight) + f64::from(item.weight))
                * (0.5 + reflected_diff.sqrt());
            item.adjusted_weight = adjusted.min(f64::from(item.weight) * 32.0) as f32;
            item.adjusted_weight
        } else {
            item.adjusted_weight = item.weight;
            item.weight
        };

        let weight = f64::from(weight);
        usage[nearest] += 1;
        weights[nearest] += weight;
        sums[nearest][0] += f64::from(item.color.a) * weight;
        sums[nearest][1] += f64::from(item.color.r) * weight;
        sums[nearest][2] += f64::from(item.color.g) * weight;
        sums[nearest][3] += f64::from(item.color.b) * weight;
    }

    for idx in 0..palette.len() {
        if weights[idx] == 0.0 {
            continue;
        }
        palette[idx] = InternalPixel {
            a: (sums[idx][0] / weights[idx]) as f32,
            r: (sums[idx][1] / weights[idx]) as f32,
            g: (sums[idx][2] / weights[idx]) as f32,
            b: (sums[idx][3] / weights[idx]) as f32,
        };
    }

    replace_unused_palette_entries(histogram, palette, &usage);

    PaletteStats {
        error: total_error / total_weight,
        used_colors: usage.iter().filter(|&&count| count > 0).count(),
    }
}

fn replace_unused_palette_entries(
    histogram: &[HistItem],
    palette: &mut [InternalPixel],
    usage: &[usize],
) {
    for (pal_idx, &count) in usage.iter().enumerate() {
        if count > 0 {
            continue;
        }

        let worst_idx = {
            let tree = NearestTree::new(palette);
            histogram
                .iter()
                .enumerate()
                .filter_map(|(item_idx, item)| {
                    if palette.is_empty() {
                        return None;
                    }
                    let hint =
                        usize::from(item.likely_palette_index).min(palette.len().saturating_sub(1));
                    let diff = tree.search(item.color, hint).1;
                    Some((item_idx, diff))
                })
                .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(Ordering::Equal))
                .map(|(item_idx, _)| item_idx)
        };

        if let Some(worst_idx) = worst_idx {
            palette[pal_idx] = histogram[worst_idx].color;
        }
    }
}

fn effective_feedback_trials(base_trials: u16, hist_items: usize) -> u16 {
    let mut trials = base_trials.max(1);
    if hist_items > 5_000 {
        trials = (trials * 3 + 3) / 4;
    }
    if hist_items > 25_000 {
        trials = (trials * 3 + 3) / 4;
    }
    if hist_items > 50_000 {
        trials = (trials * 3 + 3) / 4;
    }
    if hist_items > 100_000 {
        trials = (trials * 3 + 3) / 4;
    }
    if hist_items > 100_000 {
        trials = 1;
    } else if hist_items > 10_000 {
        trials = trials.min(2);
    } else {
        trials = trials.min(3);
    }
    trials.clamp(1, 3)
}

fn trial_kmeans_iterations(base_iterations: u16, hist_items: usize) -> u16 {
    if hist_items > 100_000 {
        base_iterations.clamp(1, 2)
    } else if hist_items > 10_000 {
        base_iterations.clamp(1, 4)
    } else {
        base_iterations.min(6)
    }
}

fn final_kmeans_iterations(base_iterations: u16, hist_items: usize) -> u16 {
    if hist_items > 100_000 {
        base_iterations.clamp(1, 3)
    } else if hist_items > 10_000 {
        base_iterations.clamp(1, 5)
    } else {
        base_iterations.min(8)
    }
}

fn reflected_color(color: InternalPixel, mapped: InternalPixel) -> InternalPixel {
    InternalPixel {
        a: (color.a + (color.a - mapped.a)).clamp(0.0, 1.0),
        r: (color.r + (color.r - mapped.r)).clamp(0.0, 1.1),
        g: (color.g + (color.g - mapped.g)).clamp(0.0, 1.1),
        b: (color.b + (color.b - mapped.b)).clamp(0.0, 1.1),
    }
}

const LEAF_MAX_SIZE: usize = 6;

struct NearestTree<'a> {
    root: SearchNode,
    points: &'a [InternalPixel],
    nearest_other_color_dist: Vec<f32>,
}

struct SearchNode {
    idx: usize,
    vantage_point: InternalPixel,
    inner: SearchNodeInner,
}

enum SearchNodeInner {
    Branch {
        radius: f32,
        radius_squared: f32,
        near: Box<SearchNode>,
        far: Box<SearchNode>,
    },
    Leaf {
        idxs: Box<[usize]>,
    },
}

struct SearchVisitor {
    idx: usize,
    distance: f32,
    distance_squared: f32,
    exclude: Option<usize>,
}

impl SearchVisitor {
    fn visit(&mut self, idx: usize, distance_squared: f32) {
        if self.exclude == Some(idx) || distance_squared >= self.distance_squared {
            return;
        }

        self.idx = idx;
        self.distance_squared = distance_squared;
        self.distance = distance_squared.sqrt();
    }
}

impl<'a> NearestTree<'a> {
    fn new(points: &'a [InternalPixel]) -> Self {
        debug_assert!(!points.is_empty());
        let mut indexes = (0..points.len()).collect::<Vec<_>>();
        let root = build_search_node(points, &mut indexes);
        let mut tree = Self {
            root,
            points,
            nearest_other_color_dist: vec![f32::INFINITY; points.len()],
        };

        if points.len() > 1 {
            for (idx, point) in points.iter().copied().enumerate() {
                let mut visitor = SearchVisitor {
                    idx: 0,
                    distance: f32::INFINITY,
                    distance_squared: f32::INFINITY,
                    exclude: Some(idx),
                };
                search_node(&tree.root, tree.points, point, &mut visitor);
                tree.nearest_other_color_dist[idx] = visitor.distance_squared / 4.0;
            }
        }

        tree
    }

    fn search(&self, needle: InternalPixel, likely_index: usize) -> (usize, f32) {
        let mut visitor = if let Some(point) = self.points.get(likely_index) {
            let guess_diff = needle.diff(*point);
            if guess_diff < self.nearest_other_color_dist[likely_index] {
                return (likely_index, guess_diff);
            }
            SearchVisitor {
                idx: likely_index,
                distance: guess_diff.sqrt(),
                distance_squared: guess_diff,
                exclude: None,
            }
        } else {
            SearchVisitor {
                idx: 0,
                distance: f32::INFINITY,
                distance_squared: f32::INFINITY,
                exclude: None,
            }
        };

        search_node(&self.root, self.points, needle, &mut visitor);
        (visitor.idx, visitor.distance_squared)
    }
}

fn build_search_node(points: &[InternalPixel], indexes: &mut [usize]) -> SearchNode {
    debug_assert!(!indexes.is_empty());
    if indexes.len() == 1 {
        let idx = indexes[0];
        return SearchNode {
            idx,
            vantage_point: points[idx],
            inner: SearchNodeInner::Leaf { idxs: Box::new([]) },
        };
    }

    let idx = indexes[0];
    let vantage_point = points[idx];
    let rest = &mut indexes[1..];
    rest.sort_by(|left, right| {
        vantage_point
            .diff(points[*left])
            .partial_cmp(&vantage_point.diff(points[*right]))
            .unwrap_or(Ordering::Equal)
    });

    let inner = if rest.len() <= LEAF_MAX_SIZE {
        SearchNodeInner::Leaf {
            idxs: rest.to_vec().into_boxed_slice(),
        }
    } else {
        let split = rest.len() / 2;
        let (near_idx, far_idx) = rest.split_at_mut(split);
        let radius_squared = vantage_point.diff(points[far_idx[0]]);
        let radius = radius_squared.sqrt();
        SearchNodeInner::Branch {
            radius,
            radius_squared,
            near: Box::new(build_search_node(points, near_idx)),
            far: Box::new(build_search_node(points, far_idx)),
        }
    };

    SearchNode {
        idx,
        vantage_point,
        inner,
    }
}

fn search_node(
    node: &SearchNode,
    points: &[InternalPixel],
    needle: InternalPixel,
    visitor: &mut SearchVisitor,
) {
    let distance_squared = node.vantage_point.diff(needle);
    visitor.visit(node.idx, distance_squared);

    match &node.inner {
        SearchNodeInner::Branch {
            radius,
            radius_squared,
            near,
            far,
        } => {
            let distance = distance_squared.sqrt();
            if distance_squared < *radius_squared {
                search_node(near, points, needle, visitor);
                if distance >= *radius - visitor.distance {
                    search_node(far, points, needle, visitor);
                }
            } else {
                search_node(far, points, needle, visitor);
                if distance <= *radius + visitor.distance {
                    search_node(near, points, needle, visitor);
                }
            }
        }
        SearchNodeInner::Leaf { idxs } => {
            for &idx in idxs.iter() {
                visitor.visit(idx, points[idx].diff(needle));
            }
        }
    }
}

fn refine_palette_from_pixels(
    rgba: &[u8],
    palette: &mut [InternalPixel],
    input_posterize_bits: u8,
    iterations: u8,
) {
    if palette.is_empty() || iterations == 0 {
        return;
    }

    let gamma = gamma_lut(SRGB_OUTPUT_GAMMA);
    for _ in 0..iterations {
        let tree = NearestTree::new(palette);
        let mut sums = vec![[0.0f64; 4]; palette.len()];
        let mut weights = vec![0.0f64; palette.len()];
        let mut cache: HashMap<u32, usize> = HashMap::new();
        let mut worst_color = None;
        let mut worst_diff = 0.0f32;

        for px in rgba.chunks_exact(4) {
            let cache_key = pack_rgba_key(px, input_posterize_bits.min(4));
            let color = InternalPixel::from_rgba(&gamma, px);
            let nearest = if let Some(&idx) = cache.get(&cache_key) {
                idx
            } else {
                let idx = tree.search(color, 0).0;
                cache.insert(cache_key, idx);
                idx
            };

            let diff = color.diff(palette[nearest]);
            if diff > worst_diff {
                worst_diff = diff;
                worst_color = Some(color);
            }

            weights[nearest] += 1.0;
            sums[nearest][0] += f64::from(color.a);
            sums[nearest][1] += f64::from(color.r);
            sums[nearest][2] += f64::from(color.g);
            sums[nearest][3] += f64::from(color.b);
        }

        for idx in 0..palette.len() {
            if weights[idx] == 0.0 {
                if let Some(color) = worst_color {
                    palette[idx] = color;
                }
                continue;
            }

            palette[idx] = InternalPixel {
                a: (sums[idx][0] / weights[idx]) as f32,
                r: (sums[idx][1] / weights[idx]) as f32,
                g: (sums[idx][2] / weights[idx]) as f32,
                b: (sums[idx][3] / weights[idx]) as f32,
            };
        }
    }
}

fn dedup_palette(palette: &[InternalPixel]) -> Vec<(InternalPixel, [u8; 4])> {
    let mut out = Vec::new();
    for &color in palette {
        let rgba = color.to_rgba(SRGB_OUTPUT_GAMMA);
        if out.iter().all(|(_, existing)| *existing != rgba) {
            out.push((color, rgba));
        }
    }
    if out.is_empty() {
        out.push((InternalPixel::default(), [0, 0, 0, 0]));
    }
    out
}

fn remap_image(
    rgba: &[u8],
    width: usize,
    height: usize,
    palette: &[(InternalPixel, [u8; 4])],
    settings: QuantizerSettings,
) -> IndexedImage {
    let (palette, mut indices, counts) = if settings.dither {
        let (indices, counts) = remap_image_dithered(rgba, width, height, palette, settings);
        (palette.to_vec(), indices, counts)
    } else {
        remap_image_plain(rgba, palette, settings.input_posterize_bits)
    };

    let mut order = (0..palette.len())
        .filter(|&idx| counts[idx] > 0)
        .collect::<Vec<_>>();
    order.sort_by(|&left, &right| {
        let left_transparent = palette[left].1[3] < 255;
        let right_transparent = palette[right].1[3] < 255;
        left_transparent
            .cmp(&right_transparent)
            .then_with(|| counts[right].cmp(&counts[left]))
    });

    let mut remap = vec![0u8; palette.len()];
    let mut reordered_palette = Vec::with_capacity(palette.len());
    for (new_idx, old_idx) in order.into_iter().enumerate() {
        remap[old_idx] = new_idx as u8;
        reordered_palette.push(palette[old_idx].1);
    }
    for idx in &mut indices {
        *idx = remap[*idx as usize];
    }

    IndexedImage {
        palette: reordered_palette,
        indices,
    }
}

fn remap_image_plain(
    rgba: &[u8],
    palette: &[(InternalPixel, [u8; 4])],
    _input_posterize_bits: u8,
) -> (Vec<(InternalPixel, [u8; 4])>, Vec<u8>, Vec<usize>) {
    let mut palette_points = palette.iter().map(|entry| entry.0).collect::<Vec<_>>();
    if palette_points.len() > 1 {
        let feedback = remap_image_plain_pass(rgba, &palette_points);
        apply_remap_feedback(&mut palette_points, &feedback);
    }

    let final_pass = remap_image_plain_pass(rgba, &palette_points);

    let remapped_palette = palette_points
        .iter()
        .map(|&color| (color, color.to_rgba(SRGB_OUTPUT_GAMMA)))
        .collect::<Vec<_>>();

    (remapped_palette, final_pass.indices, final_pass.counts)
}

struct PlainRemapPass {
    indices: Vec<u8>,
    counts: Vec<usize>,
    sums: Vec<[f64; 4]>,
    weights: Vec<f64>,
    worst_color: Option<InternalPixel>,
}

fn remap_image_plain_pass(rgba: &[u8], palette_points: &[InternalPixel]) -> PlainRemapPass {
    let mut indices = Vec::with_capacity(rgba.len() / 4);
    let gamma = gamma_lut(SRGB_OUTPUT_GAMMA);
    let mut counts = vec![0usize; palette_points.len()];
    let mut sums = vec![[0.0f64; 4]; palette_points.len()];
    let mut weights = vec![0.0f64; palette_points.len()];
    let mut worst_color = None;
    let mut worst_diff = 0.0f32;
    let tree = NearestTree::new(palette_points);
    let mut last_idx = 0usize;

    for px in rgba.chunks_exact(4) {
        let color = InternalPixel::from_rgba(&gamma, px);
        let (idx, diff) = tree.search(color, last_idx);
        last_idx = idx;
        counts[idx] += 1;
        weights[idx] += 1.0;
        sums[idx][0] += f64::from(color.a);
        sums[idx][1] += f64::from(color.r);
        sums[idx][2] += f64::from(color.g);
        sums[idx][3] += f64::from(color.b);
        if diff > worst_diff {
            worst_diff = diff;
            worst_color = Some(color);
        }
        indices.push(idx as u8);
    }

    PlainRemapPass {
        indices,
        counts,
        sums,
        weights,
        worst_color,
    }
}

fn apply_remap_feedback(palette_points: &mut [InternalPixel], pass: &PlainRemapPass) {
    for idx in 0..palette_points.len() {
        if pass.weights[idx] == 0.0 {
            if let Some(color) = pass.worst_color {
                palette_points[idx] = color;
            }
            continue;
        }

        palette_points[idx] = InternalPixel {
            a: (pass.sums[idx][0] / pass.weights[idx]) as f32,
            r: (pass.sums[idx][1] / pass.weights[idx]) as f32,
            g: (pass.sums[idx][2] / pass.weights[idx]) as f32,
            b: (pass.sums[idx][3] / pass.weights[idx]) as f32,
        };
    }
}

fn remap_image_dithered(
    rgba: &[u8],
    width: usize,
    height: usize,
    palette: &[(InternalPixel, [u8; 4])],
    settings: QuantizerSettings,
) -> (Vec<u8>, Vec<usize>) {
    let gamma = gamma_lut(SRGB_OUTPUT_GAMMA);
    let palette_points = palette.iter().map(|entry| entry.0).collect::<Vec<_>>();
    let tree = NearestTree::new(&palette_points);
    let pixels = rgba
        .chunks_exact(4)
        .map(|px| InternalPixel::from_rgba(&gamma, px))
        .collect::<Vec<_>>();
    let plain_pass = remap_image_plain_pass(rgba, &palette_points);
    let dither_map = if settings.use_dither_map != DitherMapMode::None {
        build_dither_map(&pixels, width, height, &plain_pass.indices, &palette_points)
    } else {
        Vec::new()
    };
    let mut indices = vec![0u8; pixels.len()];
    let mut counts = vec![0usize; palette.len()];
    let mut next_errors = vec![InternalPixel::default(); width + 2];
    let mut curr_errors = vec![InternalPixel::default(); width + 2];
    let mut base_dithering_level = 15.0f32 / 16.0f32;
    if !dither_map.is_empty() {
        base_dithering_level *= 1.0 / 255.0;
    }
    let max_dither_error = settings
        .target_mse
        .unwrap_or_else(|| quality_to_mse(80))
        .mul_add(2.4, 0.0)
        .max(quality_to_mse(35)) as f32;

    for row in 0..height {
        std::mem::swap(&mut curr_errors, &mut next_errors);
        next_errors.fill(InternalPixel::default());

        let even = row % 2 == 0;
        let mut last_match = 0usize;
        for offset in 0..width {
            let x = if even { offset } else { width - 1 - offset };
            let idx = row * width + x;
            let mut dither_level = base_dithering_level;
            if let Some(&level) = dither_map.get(idx) {
                dither_level *= f32::from(level);
            }

            let color = get_dithered_pixel(
                dither_level,
                max_dither_error,
                curr_errors[x + 1],
                pixels[idx],
            );

            let guess = if !dither_map.is_empty() {
                plain_pass.indices[idx] as usize
            } else {
                last_match
            };
            let (pal_idx, _) = tree.search(color, guess);
            last_match = pal_idx;
            indices[idx] = pal_idx as u8;
            counts[pal_idx] += 1;

            let out = palette[pal_idx].0;
            let mut diff = InternalPixel {
                a: color.a - out.a,
                r: color.r - out.r,
                g: color.g - out.g,
                b: color.b - out.b,
            };
            if diff.r.mul_add(diff.r, diff.g * diff.g) + diff.b.mul_add(diff.b, diff.a * diff.a)
                > max_dither_error
            {
                diff.a *= 0.75;
                diff.r *= 0.75;
                diff.g *= 0.75;
                diff.b *= 0.75;
            }

            if even {
                add_scaled_error(&mut curr_errors[x + 2], diff, 7.0 / 16.0);
                add_scaled_error(&mut next_errors[x], diff, 3.0 / 16.0);
                add_scaled_error(&mut next_errors[x + 1], diff, 5.0 / 16.0);
                add_scaled_error(&mut next_errors[x + 2], diff, 1.0 / 16.0);
            } else {
                add_scaled_error(&mut curr_errors[x], diff, 7.0 / 16.0);
                add_scaled_error(&mut next_errors[x + 2], diff, 3.0 / 16.0);
                add_scaled_error(&mut next_errors[x + 1], diff, 5.0 / 16.0);
                add_scaled_error(&mut next_errors[x], diff, 1.0 / 16.0);
            }
        }
    }

    (indices, counts)
}

fn get_dithered_pixel(
    dither_level: f32,
    max_dither_error: f32,
    err: InternalPixel,
    px: InternalPixel,
) -> InternalPixel {
    let scaled = InternalPixel {
        a: err.a * dither_level,
        r: err.r * dither_level,
        g: err.g * dither_level,
        b: err.b * dither_level,
    };
    let dither_error = scaled.r.mul_add(scaled.r, scaled.g * scaled.g)
        + scaled.b.mul_add(scaled.b, scaled.a * scaled.a);
    if dither_error < 2.0 / 256.0 / 256.0 {
        return px;
    }

    let mut ratio = 1.0f32;
    const MAX_OVERFLOW: f32 = 1.1;
    const MAX_UNDERFLOW: f32 = -0.1;
    ratio = clamp_dither_ratio(px.r, scaled.r, ratio, MAX_OVERFLOW, MAX_UNDERFLOW);
    ratio = clamp_dither_ratio(px.g, scaled.g, ratio, MAX_OVERFLOW, MAX_UNDERFLOW);
    ratio = clamp_dither_ratio(px.b, scaled.b, ratio, MAX_OVERFLOW, MAX_UNDERFLOW);
    if dither_error > max_dither_error {
        ratio *= 0.8;
    }

    InternalPixel {
        a: (px.a + scaled.a).clamp(0.0, 1.0),
        r: scaled.r.mul_add(ratio, px.r),
        g: scaled.g.mul_add(ratio, px.g),
        b: scaled.b.mul_add(ratio, px.b),
    }
}

fn clamp_dither_ratio(
    value: f32,
    delta: f32,
    current_ratio: f32,
    max_overflow: f32,
    max_underflow: f32,
) -> f32 {
    if delta == 0.0 {
        return current_ratio;
    }
    if value + delta > max_overflow {
        current_ratio.min((max_overflow - value) / delta)
    } else if value + delta < max_underflow {
        current_ratio.min((max_underflow - value) / delta)
    } else {
        current_ratio
    }
}

fn build_dither_map(
    pixels: &[InternalPixel],
    width: usize,
    height: usize,
    remapped_indices: &[u8],
    palette: &[InternalPixel],
) -> Vec<u8> {
    if width < 4 || height < 4 || pixels.len() != width.saturating_mul(height) {
        return Vec::new();
    }

    let (noise, mut edges) = compute_contrast_maps(pixels, width, height);
    if edges.is_empty() {
        return edges;
    }

    for row in 0..height {
        let row_start = row * width;
        let row_pixels = &remapped_indices[row_start..row_start + width];
        let mut last_pixel = row_pixels[0];
        let mut last_col = 0usize;

        for col in 1..width {
            let px = row_pixels[col];
            let transparent = palette[px as usize].a <= (1.0 / 255.0 * 0.625) as f32;
            if transparent {
                continue;
            }
            if px != last_pixel || col == width - 1 {
                let mut neighbor_count = 10usize * (col - last_col);
                for i in last_col..col {
                    if row > 0 && remapped_indices[(row - 1) * width + i] == last_pixel {
                        neighbor_count += 15;
                    }
                    if row + 1 < height && remapped_indices[(row + 1) * width + i] == last_pixel {
                        neighbor_count += 15;
                    }
                }

                for i in last_col..=col {
                    let edge = edges[row_start + i];
                    let adjusted = (f32::from(u16::from(edge) + 128)
                        * (255.0 / (255.0 + 128.0))
                        * (1.0 - 20.0 / (20.0 + neighbor_count as f32)))
                        as u8;
                    edges[row_start + i] = adjusted.min(noise[row_start + i]);
                }

                last_col = col;
                last_pixel = px;
            }
        }
    }

    edges
}

fn compute_contrast_maps(
    pixels: &[InternalPixel],
    width: usize,
    height: usize,
) -> (Vec<u8>, Vec<u8>) {
    let mut noise = vec![0u8; width * height];
    let mut edges = vec![0u8; width * height];

    for row in 0..height {
        let prev_row = row.saturating_sub(1);
        let next_row = (row + 1).min(height - 1);
        for col in 0..width {
            let prev = pixels[row * width + col.saturating_sub(1)];
            let curr = pixels[row * width + col];
            let next = pixels[row * width + (col + 1).min(width - 1)];
            let prev_line = pixels[prev_row * width + col];
            let next_line = pixels[next_row * width + col];

            let horiz = InternalPixel {
                a: (prev.a + next.a - curr.a * 2.0).abs(),
                r: (prev.r + next.r - curr.r * 2.0).abs(),
                g: (prev.g + next.g - curr.g * 2.0).abs(),
                b: (prev.b + next.b - curr.b * 2.0).abs(),
            };
            let vert = InternalPixel {
                a: (prev_line.a + next_line.a - curr.a * 2.0).abs(),
                r: (prev_line.r + next_line.r - curr.r * 2.0).abs(),
                g: (prev_line.g + next_line.g - curr.g * 2.0).abs(),
                b: (prev_line.b + next_line.b - curr.b * 2.0).abs(),
            };

            let horiz_max = horiz.a.max(horiz.r).max(horiz.g.max(horiz.b));
            let vert_max = vert.a.max(vert.r).max(vert.g.max(vert.b));
            let edge = horiz_max.max(vert_max);
            let mut z = (horiz_max - vert_max).abs().mul_add(-0.5, edge);
            z = 1.0 - z.max(horiz_max.min(vert_max));
            z *= z;
            z *= z;
            let idx = row * width + col;
            noise[idx] = z.mul_add(176.0, 80.0) as u8;
            edges[idx] = ((1.0 - edge).clamp(0.0, 1.0) * 256.0) as u8;
        }
    }

    let mut tmp = vec![0u8; width * height];
    max3(&noise, &mut tmp, width, height);
    max3(&tmp, &mut noise, width, height);
    blur(&mut noise, &mut tmp, width, height, 3);
    max3(&noise, &mut tmp, width, height);
    min3(&tmp, &mut noise, width, height);
    min3(&noise, &mut tmp, width, height);
    min3(&tmp, &mut noise, width, height);
    min3(&edges, &mut tmp, width, height);
    max3(&tmp, &mut edges, width, height);
    for idx in 0..edges.len() {
        edges[idx] = edges[idx].min(noise[idx]);
    }

    (noise, edges)
}

fn max3(src: &[u8], dst: &mut [u8], width: usize, height: usize) {
    op3(src, dst, width, height, |a, b| a.max(b));
}

fn min3(src: &[u8], dst: &mut [u8], width: usize, height: usize) {
    op3(src, dst, width, height, |a, b| a.min(b));
}

fn op3(src: &[u8], dst: &mut [u8], width: usize, height: usize, op: impl Fn(u8, u8) -> u8) {
    for row in 0..height {
        let row_slice = &src[row * width..][..width];
        let dst_slice = &mut dst[row * width..][..width];
        let prev_row = &src[row.saturating_sub(1) * width..][..width];
        let next_row = &src[(row + 1).min(height - 1) * width..][..width];

        let mut curr = row_slice[0];
        let mut next = row_slice[0];
        for col in 0..width - 1 {
            let prev = curr;
            curr = next;
            next = row_slice[col + 1];
            let t1 = op(prev, next);
            let t2 = op(next_row[col], prev_row[col]);
            dst_slice[col] = op(curr, op(t1, t2));
        }
        let t1 = op(curr, next);
        let t2 = op(next_row[width - 1], prev_row[width - 1]);
        dst_slice[width - 1] = op(curr, op(t1, t2));
    }
}

fn blur(src_dst: &mut [u8], tmp: &mut [u8], width: usize, height: usize, size: u16) {
    transposing_1d_blur(src_dst, tmp, width, height, size);
    transposing_1d_blur(tmp, src_dst, height, width, size);
}

fn transposing_1d_blur(src: &[u8], dst: &mut [u8], width: usize, height: usize, size: u16) {
    let radius = size as usize;
    if width < 2 * radius + 1 || height < 2 * radius + 1 {
        return;
    }

    for (row_idx, row) in src.chunks_exact(width).enumerate() {
        let mut sum = u16::from(row[0]) * size;
        for &value in &row[..radius] {
            sum += u16::from(value);
        }
        for col in 0..radius {
            sum -= u16::from(row[0]);
            sum += u16::from(row[col + radius]);
            dst[col * height + row_idx] = (sum / (size * 2)) as u8;
        }
        for col in radius..width - radius {
            sum -= u16::from(row[col - radius]);
            sum += u16::from(row[col + radius]);
            dst[col * height + row_idx] = (sum / (size * 2)) as u8;
        }
        for col in width - radius..width {
            sum -= u16::from(row[col - radius]);
            sum += u16::from(row[width - 1]);
            dst[col * height + row_idx] = (sum / (size * 2)) as u8;
        }
    }
}

fn add_scaled_error(target: &mut InternalPixel, diff: InternalPixel, scale: f32) {
    target.a += diff.a * scale;
    target.r += diff.r * scale;
    target.g += diff.g * scale;
    target.b += diff.b * scale;
}

fn channel_value(color: InternalPixel, channel: usize) -> f32 {
    match channel {
        0 => color.a,
        1 => color.r,
        2 => color.g,
        _ => color.b,
    }
}

fn pack_rgba_key(rgba: &[u8], posterize_bits: u8) -> u32 {
    let px = posterized_rgba(rgba, posterize_bits);
    u32::from_be_bytes(px)
}

fn posterized_rgba(rgba: &[u8], bits: u8) -> [u8; 4] {
    [
        posterize_channel(rgba[0], bits),
        posterize_channel(rgba[1], bits),
        posterize_channel(rgba[2], bits),
        posterize_channel(rgba[3], bits),
    ]
}

fn posterize_channel(channel: u8, bits: u8) -> u8 {
    if bits == 0 {
        channel
    } else {
        channel & !((1u8 << bits) - 1)
    }
}

#[cfg(test)]
mod tests {
    use crate::quality::SpeedSettings;

    use super::{max_colors_from_quality_speed, quantize_indexed, quantizer_settings};

    #[test]
    fn max_colors_in_range() {
        for q in 0..=100u8 {
            for s in 1..=11u8 {
                let n = max_colors_from_quality_speed(q, s);
                assert!((16..=256).contains(&n));
            }
        }
    }

    #[test]
    fn quantize_indexed_runs() {
        let rgba = vec![
            255u8, 0, 0, 255, 250, 0, 0, 255, 0, 255, 0, 255, 0, 250, 0, 255, 0, 0, 255, 255, 0, 0,
            250, 255,
        ];
        let settings = quantizer_settings(16, SpeedSettings::from_speed(4), None, false);
        let out = quantize_indexed(&rgba, 3, 2, settings);
        assert_eq!(out.indices.len(), 6);
        assert!(!out.palette.is_empty());
    }

    #[test]
    fn input_posterize_reduces_palette_variety() {
        let rgba = vec![
            255u8, 0, 0, 255, 254, 1, 0, 255, 253, 2, 0, 255, 252, 3, 0, 255,
        ];
        let mut direct_settings = quantizer_settings(16, SpeedSettings::from_speed(4), None, false);
        direct_settings.input_posterize_bits = 0;
        let direct = quantize_indexed(&rgba, 2, 2, direct_settings);

        let mut posterized_settings =
            quantizer_settings(16, SpeedSettings::from_speed(4), None, false);
        posterized_settings.input_posterize_bits = 2;
        let posterized = quantize_indexed(&rgba, 2, 2, posterized_settings);

        assert!(posterized.palette.len() <= direct.palette.len());
    }

    #[test]
    fn palette_respects_max_colors() {
        let rgba = (0..64u8)
            .flat_map(|v| [v, 255 - v, v / 2, 255])
            .collect::<Vec<_>>();
        let settings = quantizer_settings(4, SpeedSettings::from_speed(4), None, false);
        let out = quantize_indexed(&rgba, 8, 8, settings);
        assert!(out.palette.len() <= 4);
    }
}
