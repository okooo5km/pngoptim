use std::cmp::Ordering;
use std::collections::HashMap;

use crate::quality::{InternalPixel, SRGB_OUTPUT_GAMMA, SpeedSettings, gamma_lut};

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
    remap_image(
        rgba,
        width,
        height,
        &final_palette,
        settings.input_posterize_bits,
        settings.dither,
    )
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
    input_posterize_bits: u8,
    dither: bool,
) -> IndexedImage {
    let (palette, mut indices, counts) = if dither {
        let (indices, counts) = remap_image_dithered(rgba, width, height, palette);
        (palette.to_vec(), indices, counts)
    } else {
        remap_image_plain(rgba, palette, input_posterize_bits)
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
) -> (Vec<u8>, Vec<usize>) {
    let gamma = gamma_lut(SRGB_OUTPUT_GAMMA);
    let palette_points = palette.iter().map(|entry| entry.0).collect::<Vec<_>>();
    let tree = NearestTree::new(&palette_points);
    let pixels = rgba
        .chunks_exact(4)
        .map(|px| InternalPixel::from_rgba(&gamma, px))
        .collect::<Vec<_>>();
    let mut indices = vec![0u8; pixels.len()];
    let mut counts = vec![0usize; palette.len()];
    let mut next_errors = vec![InternalPixel::default(); width + 2];
    let mut curr_errors = vec![InternalPixel::default(); width + 2];

    for row in 0..height {
        std::mem::swap(&mut curr_errors, &mut next_errors);
        next_errors.fill(InternalPixel::default());

        let even = row % 2 == 0;
        let mut last_match = 0usize;
        for offset in 0..width {
            let x = if even { offset } else { width - 1 - offset };
            let idx = row * width + x;
            let mut color = pixels[idx];
            let err = curr_errors[x + 1];
            color.a = (color.a + err.a).clamp(0.0, 1.0);
            color.r = (color.r + err.r).clamp(0.0, 1.1);
            color.g = (color.g + err.g).clamp(0.0, 1.1);
            color.b = (color.b + err.b).clamp(0.0, 1.1);

            let pal_idx = tree.search(color, last_match).0;
            last_match = pal_idx;
            indices[idx] = pal_idx as u8;
            counts[pal_idx] += 1;

            let out = palette[pal_idx].0;
            let diff = InternalPixel {
                a: color.a - out.a,
                r: color.r - out.r,
                g: color.g - out.g,
                b: color.b - out.b,
            };

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
