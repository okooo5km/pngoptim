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
    pub kmeans_iteration_limit: f64,
    pub feedback_loop_trials: u16,
    pub target_mse: Option<f64>,
    pub dither: bool,
    pub use_dither_map: DitherMapMode,
    pub use_contrast_maps: bool,
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
    let contrast_pixels =
        if settings.use_contrast_maps || settings.use_dither_map != DitherMapMode::None {
            Some(
                rgba.chunks_exact(4)
                    .map(|px| InternalPixel::from_rgba(&gamma, px))
                    .collect::<Vec<_>>(),
            )
        } else {
            None
        };
    let importance_map = if settings.use_contrast_maps {
        contrast_pixels
            .as_deref()
            .and_then(|pixels| build_importance_map(pixels, width, height))
    } else {
        None
    };
    let histogram = build_histogram(
        rgba,
        width,
        settings.input_posterize_bits,
        settings.max_histogram_entries,
        &gamma,
        importance_map.as_deref(),
    );

    let mut palette = find_best_palette(&histogram, settings);
    if palette.is_empty() {
        palette = vec![InternalPixel::default()];
    }
    if histogram.items.len() <= 4_096 {
        refine_palette_from_pixels(rgba, &mut palette, settings.input_posterize_bits, 1);
    }

    let final_palette = dedup_palette(&palette);
    remap_image(
        rgba,
        width,
        height,
        &final_palette,
        settings,
        importance_map.as_deref(),
        contrast_pixels.as_deref(),
    )
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
        kmeans_iteration_limit: speed.kmeans_iteration_limit,
        feedback_loop_trials: speed.feedback_loop_trials,
        target_mse,
        dither,
        use_dither_map: speed.use_dither_map,
        use_contrast_maps: speed.use_contrast_maps,
    }
}

#[derive(Debug, Clone, Copy)]
struct HistItem {
    color: InternalPixel,
    adjusted_weight: f32,
    perceptual_weight: f32,
    mc_color_weight: f32,
    sort_value: u32,
    likely_palette_index: u16,
}

#[derive(Default)]
struct HistAccumulator {
    importance_sum: f64,
    representative: [u8; 4],
    cluster_index: u8,
    initialized: bool,
}

#[derive(Debug, Clone, Copy, Default)]
struct Cluster {
    begin: usize,
    end: usize,
}

#[derive(Debug, Clone, Default)]
struct HistogramData {
    items: Vec<HistItem>,
    total_perceptual_weight: f64,
    clusters: [Cluster; MAX_CLUSTERS],
}

#[derive(Debug, Clone, Copy)]
struct ColorBox {
    start: usize,
    end: usize,
    average: InternalPixel,
    adjusted_weight_sum: f64,
    variance: [f64; 4],
    total_error: Option<f64>,
    max_error: f32,
}

impl ColorBox {
    fn new(items: &[HistItem], start: usize, end: usize) -> Option<Self> {
        if start >= end || end > items.len() {
            return None;
        }

        let mut adjusted_weight_sum = 0.0f64;
        let mut sum_a = 0.0f64;
        let mut sum_r = 0.0f64;
        let mut sum_g = 0.0f64;
        let mut sum_b = 0.0f64;

        for item in &items[start..end] {
            let weight = f64::from(item.adjusted_weight);
            adjusted_weight_sum += weight;
            sum_a += f64::from(item.color.a) * weight;
            sum_r += f64::from(item.color.r) * weight;
            sum_g += f64::from(item.color.g) * weight;
            sum_b += f64::from(item.color.b) * weight;
        }

        if adjusted_weight_sum == 0.0 {
            return None;
        }

        let average = InternalPixel {
            a: (sum_a / adjusted_weight_sum) as f32,
            r: (sum_r / adjusted_weight_sum) as f32,
            g: (sum_g / adjusted_weight_sum) as f32,
            b: (sum_b / adjusted_weight_sum) as f32,
        };

        let mut variance = [0.0; 4];
        let mut max_error = 0.0f32;
        for item in &items[start..end] {
            let weight = f64::from(item.adjusted_weight);
            variance[0] += f64::from((item.color.a - average.a).powi(2)) * weight;
            variance[1] += f64::from((item.color.r - average.r).powi(2)) * weight;
            variance[2] += f64::from((item.color.g - average.g).powi(2)) * weight;
            variance[3] += f64::from((item.color.b - average.b).powi(2)) * weight;
            max_error = max_error.max(average.diff(item.color));
        }

        Some(Self {
            start,
            end,
            average,
            adjusted_weight_sum,
            variance,
            total_error: None,
            max_error,
        })
    }

    fn len(self) -> usize {
        self.end - self.start
    }

    fn prepare_sort(self, items: &mut [HistItem]) {
        let mut channels = [
            (0usize, self.variance[0] as f32),
            (1usize, self.variance[1] as f32),
            (2usize, self.variance[2] as f32),
            (3usize, self.variance[3] as f32),
        ];
        channels.sort_unstable_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(Ordering::Equal));

        for item in &mut items[self.start..self.end] {
            let comps = color_components(item.color);
            let primary = (comps[channels[0].0] * 65535.0).clamp(0.0, 65535.0) as u16;
            let secondary =
                ((comps[channels[2].0] + comps[channels[1].0] / 2.0 + comps[channels[3].0] / 4.0)
                    * 65535.0)
                    .clamp(0.0, 65535.0) as u16;
            item.sort_value = (u32::from(primary) << 16) | u32::from(secondary);
        }
    }

    fn compute_total_error(&mut self, items: &[HistItem]) -> f64 {
        let avg = self.average;
        let total_error = items[self.start..self.end]
            .iter()
            .map(|item| f64::from(avg.diff(item.color)) * f64::from(item.perceptual_weight))
            .sum::<f64>();
        self.total_error = Some(total_error);
        total_error
    }

    fn prepare_color_weight_total(self, items: &mut [HistItem]) -> f64 {
        let len = self.len();
        let median = {
            let slice = &mut items[self.start..self.end];
            let (_, mid, _) = slice.select_nth_unstable_by_key(len / 2, |item| item.sort_value);
            mid.color
        };
        items[self.start..self.end]
            .iter_mut()
            .map(|item| {
                let weight = (median.diff(item.color).sqrt() * (2.0 + item.adjusted_weight)).sqrt();
                item.mc_color_weight = weight;
                f64::from(weight)
            })
            .sum()
    }

    fn split(self, items: &mut [HistItem]) -> Option<(Self, Self)> {
        if self.len() <= 1 {
            return None;
        }

        self.prepare_sort(items);
        let half_weight = self.prepare_color_weight_total(items) / 2.0;

        let local_split = {
            let slice = &mut items[self.start..self.end];
            slice.sort_unstable_by_key(|item| std::cmp::Reverse(item.sort_value));
            let mut cumulative = 0.0f64;
            let mut split = 1usize;
            for idx in 0..slice.len() - 1 {
                cumulative += f64::from(slice[idx].mc_color_weight);
                if cumulative >= half_weight {
                    split = idx + 1;
                    break;
                }
            }
            split.clamp(1, slice.len() - 1)
        };

        let split = self.start + local_split;
        let left = Self::new(items, self.start, split)?;
        let right = Self::new(items, split, self.end)?;
        Some((left, right))
    }
}

const MAX_CLUSTERS: usize = 16;

fn build_histogram(
    rgba: &[u8],
    _width: usize,
    initial_posterize_bits: u8,
    max_histogram_entries: u32,
    gamma: &[f32; 256],
    importance_map: Option<&[u8]>,
) -> HistogramData {
    let mut bits = initial_posterize_bits.min(4);
    loop {
        let mut map: HashMap<u32, HistAccumulator> = HashMap::new();
        for (pixel_idx, px) in rgba.chunks_exact(4).enumerate() {
            let key = pack_rgba_key(px, bits);
            let entry = map.entry(key).or_default();
            let importance = f64::from(
                importance_map
                    .and_then(|map| map.get(pixel_idx))
                    .copied()
                    .unwrap_or(255),
            );
            entry.importance_sum += importance;
            if !entry.initialized {
                let rgba = [px[0], px[1], px[2], px[3]];
                entry.representative = rgba;
                entry.cluster_index = cluster_index(rgba);
                entry.initialized = true;
            }
        }

        if map.len() <= max_histogram_entries as usize || bits >= 4 {
            return finalize_histogram(map, gamma);
        }
        bits += 1;
    }
}

fn finalize_histogram(map: HashMap<u32, HistAccumulator>, gamma: &[f32; 256]) -> HistogramData {
    if map.is_empty() {
        return HistogramData::default();
    }

    let temp = map
        .into_values()
        .filter(|acc| acc.initialized && acc.importance_sum > 0.0)
        .collect::<Vec<_>>();
    if temp.is_empty() {
        return HistogramData::default();
    }

    let mut counts = [0usize; MAX_CLUSTERS];
    for item in &temp {
        counts[item.cluster_index as usize] += 1;
    }

    let mut clusters = [Cluster::default(); MAX_CLUSTERS];
    let mut next_begin = 0usize;
    for (cluster, count) in clusters.iter_mut().zip(counts) {
        cluster.begin = next_begin;
        cluster.end = next_begin;
        next_begin += count;
    }

    let max_perceptual_weight =
        ((0.1 / 255.0) * temp.iter().map(|item| item.importance_sum).sum::<f64>()) as f32;
    let mut items = vec![
        HistItem {
            color: InternalPixel::default(),
            adjusted_weight: 0.0,
            perceptual_weight: 0.0,
            mc_color_weight: 0.0,
            sort_value: 0,
            likely_palette_index: 0,
        };
        temp.len()
    ];
    let mut total_perceptual_weight = 0.0f64;

    for item in temp {
        let cluster = &mut clusters[item.cluster_index as usize];
        let next_index = cluster.end;
        cluster.end += 1;

        let weight = ((item.importance_sum as f32) / 255.0).min(max_perceptual_weight);
        let weight = weight.max(1.0 / 255.0);
        total_perceptual_weight += f64::from(weight);

        items[next_index] = HistItem {
            color: InternalPixel::from_rgba(gamma, &item.representative),
            adjusted_weight: weight,
            perceptual_weight: weight,
            mc_color_weight: 0.0,
            sort_value: 0,
            likely_palette_index: 0,
        };
    }

    HistogramData {
        items,
        total_perceptual_weight,
        clusters,
    }
}

fn median_cut_palette(
    histogram: &mut HistogramData,
    target_colors: usize,
    target_mse: f64,
    max_mse_per_color: f64,
) -> Vec<InternalPixel> {
    if histogram.items.is_empty() {
        return vec![InternalPixel::default()];
    }

    let mut boxes = initial_color_boxes(histogram, target_colors);
    let max_mse_per_color = max_mse_per_color.max(quality_to_mse(20));

    while boxes.len() < target_colors {
        let fraction_done = boxes.len() as f64 / target_colors as f64;
        let current_max_mse = (fraction_done * 16.0).mul_add(max_mse_per_color, max_mse_per_color);
        let Some(box_index) = take_best_splittable_box(&boxes, current_max_mse) else {
            break;
        };

        let selected = boxes.swap_remove(box_index);
        let Some((left, right)) = selected.split(&mut histogram.items) else {
            break;
        };
        boxes.push(left);
        boxes.push(right);

        if total_box_error_below_target(
            &mut boxes,
            &histogram.items,
            histogram.total_perceptual_weight,
            target_mse,
        ) {
            break;
        }
    }

    boxes_to_palette(&mut boxes, &mut histogram.items)
}

fn initial_color_boxes(histogram: &HistogramData, target_colors: usize) -> Vec<ColorBox> {
    let used_boxes = histogram
        .clusters
        .iter()
        .filter(|cluster| cluster.begin != cluster.end)
        .count();
    if used_boxes > 0 && used_boxes <= target_colors / 3 {
        histogram
            .clusters
            .iter()
            .filter(|cluster| cluster.begin != cluster.end)
            .filter_map(|cluster| ColorBox::new(&histogram.items, cluster.begin, cluster.end))
            .collect()
    } else {
        vec![
            ColorBox::new(&histogram.items, 0, histogram.items.len()).expect("non-empty histogram"),
        ]
    }
}

fn take_best_splittable_box(boxes: &[ColorBox], max_mse: f64) -> Option<usize> {
    boxes
        .iter()
        .enumerate()
        .filter(|(_, color_box)| color_box.len() > 1)
        .map(|(idx, color_box)| {
            let mut score = color_box.adjusted_weight_sum * color_box.variance.iter().sum::<f64>();
            if f64::from(color_box.max_error) > max_mse {
                score = score * f64::from(color_box.max_error) / max_mse;
            }
            (idx, score)
        })
        .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap_or(Ordering::Equal))
        .map(|(idx, _)| idx)
}

fn total_box_error_below_target(
    boxes: &mut [ColorBox],
    items: &[HistItem],
    total_perceptual_weight: f64,
    target_mse: f64,
) -> bool {
    if !target_mse.is_finite() {
        return false;
    }

    let target_total_error = target_mse * total_perceptual_weight;
    let mut total_error = boxes
        .iter()
        .filter_map(|color_box| color_box.total_error)
        .sum::<f64>();
    if total_error > target_total_error {
        return false;
    }

    for color_box in boxes
        .iter_mut()
        .filter(|color_box| color_box.total_error.is_none())
    {
        total_error += color_box.compute_total_error(items);
        if total_error > target_total_error {
            return false;
        }
    }
    true
}

fn boxes_to_palette(boxes: &mut [ColorBox], items: &mut [HistItem]) -> Vec<InternalPixel> {
    let mut palette = Vec::with_capacity(boxes.len());
    for (palette_index, color_box) in boxes.iter_mut().enumerate() {
        for item in &mut items[color_box.start..color_box.end] {
            item.likely_palette_index = palette_index as u16;
        }

        let representative = if color_box.len() > 2 {
            items[color_box.start..color_box.end]
                .iter()
                .min_by(|a, b| {
                    color_box
                        .average
                        .diff(a.color)
                        .partial_cmp(&color_box.average.diff(b.color))
                        .unwrap_or(Ordering::Equal)
                })
                .map(|item| item.color)
                .unwrap_or(color_box.average)
        } else {
            color_box.average
        };
        palette.push(representative);
    }
    palette
}

fn find_best_palette(histogram: &HistogramData, settings: QuantizerSettings) -> Vec<InternalPixel> {
    if histogram.items.is_empty() {
        return vec![InternalPixel::default()];
    }

    let max_colors = settings.max_colors.clamp(2, 256);
    let mut hist = histogram.clone();
    let hist_items = hist.items.len();
    let total_trials = effective_feedback_trials(settings.feedback_loop_trials, hist_items);
    let final_iteration_limit =
        effective_kmeans_iteration_limit(settings.kmeans_iteration_limit, hist_items);
    let has_quality_target = settings.target_mse.is_some();
    let final_iterations = effective_kmeans_iterations(
        settings.kmeans_iterations,
        hist_items,
        false,
        has_quality_target,
    );

    if hist.items.len() <= max_colors || total_trials == 0 || !has_quality_target {
        let mut palette = if hist.items.len() <= max_colors {
            hist.items.iter().map(|item| item.color).collect::<Vec<_>>()
        } else {
            median_cut_palette(&mut hist, max_colors, f64::INFINITY, f64::INFINITY)
        };
        let _ = refine_palette(
            &mut hist.items,
            &mut palette,
            final_iterations,
            false,
            final_iteration_limit,
        );
        return palette;
    }

    let target_mse = settings.target_mse.unwrap_or(f64::INFINITY);
    let mut current_max_colors = max_colors;
    let mut trials_left = total_trials as i32;
    let mut baseline_hist = histogram.clone();
    let mut baseline_palette = if baseline_hist.items.len() <= max_colors {
        baseline_hist
            .items
            .iter()
            .map(|item| item.color)
            .collect::<Vec<_>>()
    } else {
        median_cut_palette(&mut baseline_hist, max_colors, f64::INFINITY, f64::INFINITY)
    };
    let baseline_stats = refine_palette(
        &mut baseline_hist.items,
        &mut baseline_palette,
        final_iterations,
        false,
        final_iteration_limit,
    );
    let baseline_used_colors = baseline_stats
        .used_colors
        .max(2)
        .min(baseline_palette.len());
    let mut best_palette = Some(baseline_palette);
    let mut best_error = baseline_stats.error;
    let mut best_used_colors = baseline_used_colors;
    let mut fails_in_a_row = 0i32;
    let mut target_mse_overshoot = if total_trials > 0 { 1.05 } else { 1.0 };

    if best_error < target_mse && best_error > 0.0 {
        current_max_colors = current_max_colors
            .min(best_used_colors.saturating_add(1))
            .saturating_sub(1)
            .max(2);
    }

    while trials_left > 0 && current_max_colors >= 2 {
        let max_mse_per_color = target_mse
            .max(best_error.min(quality_to_mse(1)))
            .max(quality_to_mse(51))
            * 1.2;
        let mut palette = median_cut_palette(
            &mut hist,
            current_max_colors,
            target_mse * target_mse_overshoot,
            max_mse_per_color,
        );
        let first_target_run = best_palette.is_none() && target_mse > 0.0;
        let stats = kmeans_iteration(&mut hist.items, &mut palette, !first_target_run);
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
                target_mse_overshoot = (target_mse_overshoot * 1.25)
                    .min(target_mse / stats.error.max(f64::MIN_POSITIVE));
                current_max_colors = current_max_colors
                    .min(used_colors.saturating_add(1))
                    .saturating_sub(1)
                    .max(2);
            }
        } else {
            fails_in_a_row += 1;
            target_mse_overshoot = 1.0;
            trials_left -= 1 + fails_in_a_row.min(2);
        }
    }

    let mut palette = best_palette
        .unwrap_or_else(|| median_cut_palette(&mut hist, max_colors, f64::INFINITY, f64::INFINITY));
    let mut final_hist = histogram.clone();
    let palette_error_is_known = best_error.is_finite();
    let final_iterations = effective_kmeans_iterations(
        settings.kmeans_iterations,
        hist_items,
        palette_error_is_known,
        has_quality_target,
    );
    let _ = refine_palette(
        &mut final_hist.items,
        &mut palette,
        final_iterations,
        false,
        final_iteration_limit,
    );
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
    iteration_limit: f64,
) -> PaletteStats {
    if palette.is_empty() {
        return PaletteStats {
            error: 0.0,
            used_colors: 0,
        };
    }

    let mut stats = kmeans_iteration(histogram, palette, adjust_weights);
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
        .map(|item| f64::from(item.perceptual_weight))
        .sum::<f64>()
        .max(1e-9);
    let mut total_error = 0.0f64;

    for item in histogram.iter_mut() {
        let hint = usize::from(item.likely_palette_index).min(palette.len().saturating_sub(1));
        let (nearest, diff_sq) = tree.search(item.color, hint);
        let diff = f64::from(diff_sq);
        item.likely_palette_index = nearest as u16;
        total_error += diff * f64::from(item.perceptual_weight);

        let weight = if adjust_weights {
            let reflected = reflected_color(item.color, palette[nearest]);
            let reflected_diff = f64::from(tree.search(reflected, nearest).1);
            let adjusted = (2.0 * f64::from(item.adjusted_weight)
                + f64::from(item.perceptual_weight))
                * (0.5 + reflected_diff.sqrt());
            item.adjusted_weight = adjusted.min(f64::from(item.perceptual_weight) * 32.0) as f32;
            item.adjusted_weight
        } else {
            item.adjusted_weight = item.perceptual_weight;
            item.perceptual_weight
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
    let mut trials = base_trials;
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
    trials
}

fn effective_kmeans_iterations(
    base_iterations: u16,
    hist_items: usize,
    palette_error_is_known: bool,
    has_quality_target: bool,
) -> u16 {
    let mut iterations = base_iterations;
    if hist_items > 5_000 {
        iterations = (iterations * 3 + 3) / 4;
    }
    if hist_items > 25_000 {
        iterations = (iterations * 3 + 3) / 4;
    }
    if hist_items > 50_000 {
        iterations = (iterations * 3 + 3) / 4;
    }
    if hist_items > 100_000 {
        iterations = (iterations * 3 + 3) / 4;
    }
    if iterations == 0 && !palette_error_is_known && has_quality_target {
        iterations = 1;
    }
    iterations
}

fn effective_kmeans_iteration_limit(base_limit: f64, hist_items: usize) -> f64 {
    if hist_items > 100_000 {
        base_limit * 2.0
    } else {
        base_limit
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
    importance_map: Option<&[u8]>,
    contrast_pixels: Option<&[InternalPixel]>,
) -> IndexedImage {
    let (palette, mut indices, counts) = if settings.dither {
        remap_image_dithered(
            rgba,
            width,
            height,
            palette,
            settings,
            importance_map,
            contrast_pixels,
        )
    } else {
        remap_image_plain(rgba, palette, importance_map)
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
    importance_map: Option<&[u8]>,
) -> (Vec<(InternalPixel, [u8; 4])>, Vec<u8>, Vec<usize>) {
    let mut palette_points = palette.iter().map(|entry| entry.0).collect::<Vec<_>>();
    if palette_points.len() > 1 {
        let feedback = remap_image_plain_pass(rgba, &palette_points, importance_map);
        apply_remap_feedback(&mut palette_points, &feedback);
    }

    let final_pass = remap_image_plain_pass(rgba, &palette_points, importance_map);

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

fn remap_image_plain_pass(
    rgba: &[u8],
    palette_points: &[InternalPixel],
    importance_map: Option<&[u8]>,
) -> PlainRemapPass {
    let mut indices = Vec::with_capacity(rgba.len() / 4);
    let gamma = gamma_lut(SRGB_OUTPUT_GAMMA);
    let mut counts = vec![0usize; palette_points.len()];
    let mut sums = vec![[0.0f64; 4]; palette_points.len()];
    let mut weights = vec![0.0f64; palette_points.len()];
    let mut worst_color = None;
    let mut worst_diff = 0.0f32;
    let tree = NearestTree::new(palette_points);
    let mut last_idx = 0usize;

    for (pixel_idx, px) in rgba.chunks_exact(4).enumerate() {
        let color = InternalPixel::from_rgba(&gamma, px);
        let (idx, diff) = tree.search(color, last_idx);
        last_idx = idx;
        counts[idx] += 1;
        let importance = importance_map
            .and_then(|map| map.get(pixel_idx))
            .copied()
            .map(f64::from)
            .unwrap_or(1.0);
        weights[idx] += importance;
        sums[idx][0] += f64::from(color.a) * importance;
        sums[idx][1] += f64::from(color.r) * importance;
        sums[idx][2] += f64::from(color.g) * importance;
        sums[idx][3] += f64::from(color.b) * importance;
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
    importance_map: Option<&[u8]>,
    contrast_pixels: Option<&[InternalPixel]>,
) -> (Vec<(InternalPixel, [u8; 4])>, Vec<u8>, Vec<usize>) {
    let mut palette_points = palette.iter().map(|entry| entry.0).collect::<Vec<_>>();
    if palette_points.len() > 1 {
        let feedback = remap_image_plain_pass(rgba, &palette_points, importance_map);
        apply_remap_feedback(&mut palette_points, &feedback);
    }

    let tree = NearestTree::new(&palette_points);
    let gamma = gamma_lut(SRGB_OUTPUT_GAMMA);
    let owned_pixels;
    let pixels = if let Some(pixels) = contrast_pixels {
        pixels
    } else {
        owned_pixels = rgba
            .chunks_exact(4)
            .map(|px| InternalPixel::from_rgba(&gamma, px))
            .collect::<Vec<_>>();
        &owned_pixels
    };
    let plain_pass = remap_image_plain_pass(rgba, &palette_points, importance_map);
    let dither_map = if settings.use_dither_map != DitherMapMode::None {
        build_dither_map(pixels, width, height, &plain_pass.indices, &palette_points)
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

            let plain_idx = if !dither_map.is_empty() {
                plain_pass.indices[idx] as usize
            } else {
                last_match
            };
            let (mut pal_idx, _) = tree.search(color, plain_idx);
            if should_prefer_plain_match(pixels[idx], plain_idx, pal_idx, &palette_points) {
                pal_idx = plain_idx;
            }
            last_match = pal_idx;
            indices[idx] = pal_idx as u8;
            counts[pal_idx] += 1;

            let out = palette_points[pal_idx];
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

    let remapped_palette = palette_points
        .iter()
        .map(|&color| (color, color.to_rgba(SRGB_OUTPUT_GAMMA)))
        .collect::<Vec<_>>();

    (remapped_palette, indices, counts)
}

fn should_prefer_plain_match(
    input: InternalPixel,
    plain_idx: usize,
    dithered_idx: usize,
    palette_points: &[InternalPixel],
) -> bool {
    if plain_idx == dithered_idx || palette_points.is_empty() {
        return false;
    }

    let Some(&plain) = palette_points.get(plain_idx) else {
        return false;
    };
    let Some(&dithered) = palette_points.get(dithered_idx) else {
        return false;
    };

    if !is_transparentish(input) && !is_transparentish(plain) && !is_transparentish(dithered) {
        return false;
    }

    input.diff(plain) <= input.diff(dithered) * 1.02
}

fn is_transparentish(px: InternalPixel) -> bool {
    px.a <= 8.0 / 255.0 * 0.625
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

fn build_importance_map(pixels: &[InternalPixel], width: usize, height: usize) -> Option<Vec<u8>> {
    if width < 4 || height < 4 || pixels.len() != width.saturating_mul(height) {
        return None;
    }
    Some(compute_contrast_maps(pixels, width, height).0)
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

fn color_components(color: InternalPixel) -> [f32; 4] {
    [color.a, color.r, color.g, color.b]
}

fn cluster_index(rgba: [u8; 4]) -> u8 {
    ((rgba[0] >> 7) << 3) | ((rgba[1] >> 7) << 2) | ((rgba[2] >> 7) << 1) | (rgba[3] >> 7)
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

    use super::{
        InternalPixel, QuantizerSettings, apply_remap_feedback, gamma_lut, quantize_indexed,
        quantizer_settings, remap_image_dithered, remap_image_plain_pass,
        should_prefer_plain_match,
    };
    use crate::quality::{DitherMapMode, SRGB_OUTPUT_GAMMA};

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

    #[test]
    fn plain_remap_feedback_uses_importance_weights() {
        let rgba = vec![255u8, 0, 0, 255, 0, 0, 255, 255];
        let gamma = gamma_lut(SRGB_OUTPUT_GAMMA);
        let mut palette_points = vec![InternalPixel::from_rgba(&gamma, &[0, 0, 0, 255])];
        let pass = remap_image_plain_pass(&rgba, &palette_points, Some(&[255, 1]));
        apply_remap_feedback(&mut palette_points, &pass);

        assert!(palette_points[0].r > palette_points[0].b);
    }

    #[test]
    fn dithered_remap_reuses_plain_feedback_palette() {
        let rgba = vec![255u8, 0, 0, 255, 0, 0, 255, 255];
        let gamma = gamma_lut(SRGB_OUTPUT_GAMMA);
        let palette = vec![(
            InternalPixel::from_rgba(&gamma, &[0, 0, 0, 255]),
            [0u8, 0, 0, 255],
        )];
        let settings = QuantizerSettings {
            max_colors: 1,
            input_posterize_bits: 0,
            max_histogram_entries: 256,
            kmeans_iterations: 1,
            kmeans_iteration_limit: 1e-7,
            feedback_loop_trials: 1,
            target_mse: None,
            dither: true,
            use_dither_map: DitherMapMode::None,
            use_contrast_maps: false,
        };

        let (plain_palette, _, _) = super::remap_image_plain(&rgba, &palette, Some(&[255, 1]));
        let (palette, indices, counts) =
            remap_image_dithered(&rgba, 2, 1, &palette, settings, Some(&[255, 1]), None);

        assert_eq!(indices.len(), 2);
        assert_eq!(counts[0], 2);
        assert_eq!(palette[0].1, plain_palette[0].1);
    }

    #[test]
    fn transparent_pixels_prefer_plain_match_when_dither_is_worse() {
        let gamma = gamma_lut(SRGB_OUTPUT_GAMMA);
        let input = InternalPixel::from_rgba(&gamma, &[32, 32, 32, 6]);
        let palette = vec![
            InternalPixel::from_rgba(&gamma, &[0, 0, 0, 0]),
            InternalPixel::from_rgba(&gamma, &[255, 0, 0, 255]),
        ];

        assert!(should_prefer_plain_match(input, 0, 1, &palette));
        assert!(!should_prefer_plain_match(
            InternalPixel::from_rgba(&gamma, &[255, 0, 0, 255]),
            0,
            1,
            &palette,
        ));
    }
}
