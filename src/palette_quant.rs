use std::cmp::Ordering;
use std::collections::HashMap;
use std::hash::Hasher;

use rayon::prelude::*;

use crate::quality::{
    DitherMapMode, InternalPixel, SRGB_OUTPUT_GAMMA, SpeedSettings, gamma_lut, quality_to_mse,
};

#[derive(Debug, Clone)]
pub struct IndexedImage {
    pub palette: Vec<[u8; 4]>,
    pub indices: Vec<u8>,
}

#[derive(Debug, Clone)]
struct ContrastMaps {
    importance_map: Vec<u8>,
    edges: Vec<u8>,
}

#[derive(Debug, Clone, Copy)]
struct PaletteEntry {
    color: InternalPixel,
    popularity: f32,
}

#[derive(Debug, Clone, Copy)]
pub struct QuantizerSettings {
    pub max_colors: usize,
    pub input_posterize_bits: u8,
    pub output_posterize_bits: u8,
    pub max_histogram_entries: u32,
    pub kmeans_iterations: u16,
    pub kmeans_iteration_limit: f64,
    pub feedback_loop_trials: u16,
    pub target_mse: f64,
    pub max_mse: Option<f64>,
    pub target_mse_is_zero: bool,
    pub dither: bool,
    pub dither_level: f32,
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
    let contrast_maps = contrast_pixels
        .as_deref()
        .and_then(|pixels| build_contrast_maps(pixels, width, height));
    let importance_map = contrast_maps
        .as_ref()
        .map(|maps| maps.importance_map.as_slice());
    let edges_map = contrast_maps.as_ref().map(|maps| maps.edges.as_slice());
    let histogram = build_histogram(
        rgba,
        width,
        settings.input_posterize_bits,
        settings.max_histogram_entries,
        &gamma,
        importance_map,
    );

    let (mut palette, palette_error) = find_best_palette(&histogram, settings);
    if palette.is_empty() {
        palette = vec![PaletteEntry {
            color: InternalPixel::default(),
            popularity: 0.0,
        }];
    }
    sort_palette_entries(&mut palette);

    let final_palette = palette
        .iter()
        .map(|entry| (entry.color, entry.color.to_rgba(SRGB_OUTPUT_GAMMA)))
        .collect::<Vec<_>>();
    remap_image(
        rgba,
        width,
        height,
        &final_palette,
        palette_error,
        settings,
        importance_map,
        edges_map,
        contrast_pixels.as_deref(),
    )
}

pub fn quantizer_settings(
    max_colors: usize,
    speed: SpeedSettings,
    target_mse: f64,
    max_mse: Option<f64>,
    target_mse_is_zero: bool,
    output_posterize_bits: u8,
    dither_level: f32,
) -> QuantizerSettings {
    QuantizerSettings {
        max_colors,
        input_posterize_bits: speed.input_posterize_bits,
        output_posterize_bits,
        max_histogram_entries: speed.max_histogram_entries,
        kmeans_iterations: speed.kmeans_iterations,
        kmeans_iteration_limit: speed.kmeans_iteration_limit,
        feedback_loop_trials: speed.feedback_loop_trials,
        target_mse,
        max_mse,
        target_mse_is_zero,
        dither: dither_level > 0.0 && !speed.force_disable_dither,
        dither_level,
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
    importance_sum: u32,
    representative: [u8; 4],
    cluster_index: u8,
    initialized: bool,
}

#[derive(Default)]
struct U32HashBuilder(u32);

impl std::hash::BuildHasher for U32HashBuilder {
    type Hasher = Self;

    fn build_hasher(&self) -> Self {
        Self(0)
    }
}

impl Hasher for U32HashBuilder {
    fn finish(&self) -> u64 {
        u64::from(self.0).wrapping_mul(0x517c_c1b7_2722_0a95)
    }

    fn write(&mut self, bytes: &[u8]) {
        debug_assert!(bytes.len() <= 4);
        let mut value = [0u8; 4];
        value[..bytes.len()].copy_from_slice(bytes);
        self.0 = u32::from_ne_bytes(value);
    }

    fn write_u32(&mut self, value: u32) {
        self.0 = value;
    }
}

type HistMap = HashMap<u32, HistAccumulator, U32HashBuilder>;

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
    variance: [f32; 4],
    total_error: Option<f64>,
    max_error: f32,
}

impl ColorBox {
    fn new(items: &[HistItem], start: usize, end: usize) -> Option<Self> {
        if start >= end || end > items.len() {
            return None;
        }

        let mut adjusted_weight_sum = 0.0f64;
        let mut average = InternalPixel::default();
        let mut average_weight_sum = 0.0f32;

        for item in &items[start..end] {
            let weight = f64::from(item.adjusted_weight);
            adjusted_weight_sum += weight;
            average_weight_sum += item.adjusted_weight;
            average.a += item.color.a * item.adjusted_weight;
            average.r += item.color.r * item.adjusted_weight;
            average.g += item.color.g * item.adjusted_weight;
            average.b += item.color.b * item.adjusted_weight;
        }

        if adjusted_weight_sum == 0.0 {
            return None;
        }

        if average_weight_sum != 0.0 {
            average.a /= average_weight_sum;
            average.r /= average_weight_sum;
            average.g /= average_weight_sum;
            average.b /= average_weight_sum;
        }

        let mut variance = [0.0f32; 4];
        let mut max_error = 0.0f32;
        for item in &items[start..end] {
            let delta_a = item.color.a - average.a;
            let delta_r = item.color.r - average.r;
            let delta_g = item.color.g - average.g;
            let delta_b = item.color.b - average.b;
            variance[0] += delta_a * delta_a * item.adjusted_weight;
            variance[1] += delta_r * delta_r * item.adjusted_weight;
            variance[2] += delta_g * delta_g * item.adjusted_weight;
            variance[3] += delta_b * delta_b * item.adjusted_weight;
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
            hist_item_sort_half(slice, half_weight)
                .max(1)
                .min(slice.len().saturating_sub(1))
        };

        let split = self.start + local_split;
        let left = Self::new(items, self.start, split)?;
        let right = Self::new(items, split, self.end)?;
        Some((left, right))
    }
}

#[inline(always)]
fn mc_sort_value(base: &[HistItem], idx: usize) -> Option<u32> {
    base.get(idx).map(|item| item.sort_value)
}

#[inline]
fn qsort_pivot(base: &[HistItem]) -> Option<usize> {
    let len = base.len();
    if len < 32 {
        return Some(len / 2);
    }
    let mut pivots = [8, len / 2, len - 1];
    pivots.sort_unstable_by_key(|&idx| mc_sort_value(base, idx).unwrap_or_default());
    (pivots[1] < base.len()).then_some(pivots[1])
}

fn qsort_partition(base: &mut [HistItem]) -> Option<usize> {
    let mut right = base.len();
    base.swap(qsort_pivot(base)?, 0);
    let pivot_value = mc_sort_value(base, 0)?;
    let mut left = 1usize;
    while left < right {
        if mc_sort_value(base, left)? >= pivot_value {
            left += 1;
        } else {
            right -= 1;
            while left < right && mc_sort_value(base, right)? <= pivot_value {
                right -= 1;
            }
            if right >= base.len() {
                return None;
            }
            base.swap(left, right);
        }
    }
    left = left.saturating_sub(1);
    if left >= base.len() {
        return None;
    }
    base.swap(left, 0);
    Some(left)
}

#[inline(never)]
fn hist_item_sort_half(mut base: &mut [HistItem], mut weight_half_sum: f64) -> usize {
    let mut base_index = 0usize;
    if base.is_empty() {
        return 0;
    }
    loop {
        let Some(partition) = qsort_partition(base) else {
            return base_index;
        };
        let split_at = partition + 1;
        let (left, right) = base.split_at_mut(split_at);
        let left_sum = left
            .iter()
            .map(|item| f64::from(item.mc_color_weight))
            .sum::<f64>();
        if left_sum >= weight_half_sum {
            if partition > 0 {
                base = &mut left[..partition];
                continue;
            }
            return base_index;
        }
        weight_half_sum -= left_sum;
        base_index += left.len();
        if right.is_empty() {
            return base_index;
        }
        base = right;
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
    let mut map = build_histogram_map(rgba, importance_map);
    let mut bits = 0u8;
    let requested_bits = initial_posterize_bits.min(3);
    if requested_bits > 0 {
        reposterize_histogram_map(&mut map, requested_bits);
        bits = requested_bits;
    }
    if map.len() > max_histogram_entries as usize && bits < 3 {
        reposterize_histogram_map(&mut map, bits + 1);
    }
    finalize_histogram(map, gamma)
}

fn build_histogram_map(rgba: &[u8], importance_map: Option<&[u8]>) -> HistMap {
    let mut map: HistMap = HashMap::with_hasher(U32HashBuilder::default());
    for (pixel_idx, px) in rgba.chunks_exact(4).enumerate() {
        let representative = [px[0], px[1], px[2], px[3]];
        let key = pack_rgba_key(&representative, 0);
        let entry = map.entry(key).or_default();
        let importance = u32::from(
            importance_map
                .and_then(|map| map.get(pixel_idx))
                .copied()
                .unwrap_or(255),
        );
        entry.importance_sum = entry.importance_sum.saturating_add(importance);
        if !entry.initialized {
            entry.representative = representative;
            entry.cluster_index = cluster_index(representative);
            entry.initialized = true;
        }
    }
    map
}

fn reposterize_histogram_map(map: &mut HistMap, posterize_bits: u8) {
    if posterize_bits == 0 || map.is_empty() {
        return;
    }

    let channel_mask = 255u8 << posterize_bits;
    let mask = u32::from_ne_bytes([channel_mask, channel_mask, channel_mask, channel_mask]);
    let old_size = map.len();
    let new_capacity = (old_size / 3).max(map.capacity() / 5);
    let old_map = std::mem::replace(
        map,
        HashMap::with_capacity_and_hasher(new_capacity, U32HashBuilder::default()),
    );
    map.extend(old_map.into_iter().map(|(key, value)| (key & mask, value)));
}

fn finalize_histogram(map: HistMap, gamma: &[f32; 256]) -> HistogramData {
    if map.is_empty() {
        return HistogramData::default();
    }

    let temp = map
        .into_values()
        .filter(|acc| acc.initialized && acc.importance_sum > 0)
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

    let max_perceptual_weight = ((0.1 / 255.0)
        * temp
            .iter()
            .map(|item| f64::from(item.importance_sum))
            .sum::<f64>()) as f32;
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
) -> Vec<PaletteEntry> {
    if histogram.items.is_empty() {
        return vec![PaletteEntry {
            color: InternalPixel::default(),
            popularity: 0.0,
        }];
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
            let mut score = color_box.adjusted_weight_sum
                * color_box
                    .variance
                    .iter()
                    .map(|variance| f64::from(*variance))
                    .sum::<f64>();
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

fn boxes_to_palette(boxes: &mut [ColorBox], items: &mut [HistItem]) -> Vec<PaletteEntry> {
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
        let popularity = items[color_box.start..color_box.end]
            .iter()
            .map(|item| item.perceptual_weight)
            .sum::<f32>();
        palette.push(PaletteEntry {
            color: representative,
            popularity,
        });
    }
    palette
}

fn find_best_palette(
    histogram: &HistogramData,
    settings: QuantizerSettings,
) -> (Vec<PaletteEntry>, Option<f64>) {
    if histogram.items.is_empty() {
        return (
            vec![PaletteEntry {
                color: InternalPixel::default(),
                popularity: 0.0,
            }],
            Some(0.0),
        );
    }

    let max_colors = settings.max_colors.clamp(2, 256);
    let mut hist = histogram.clone();
    let hist_items = hist.items.len();
    let total_trials = effective_feedback_trials(settings.feedback_loop_trials, hist_items) as i32;
    let final_iteration_limit =
        effective_kmeans_iteration_limit(settings.kmeans_iteration_limit, hist_items);
    let (max_mse, target_mse, target_mse_is_zero) =
        effective_target_mse(&settings, hist_items, max_colors);
    let has_quality_target = max_mse.is_some();
    let few_input_colors = hist.items.len() <= max_colors;
    if few_input_colors && target_mse_is_zero {
        return (
            hist.items
                .iter()
                .map(|item| PaletteEntry {
                    color: item.color,
                    popularity: item.perceptual_weight,
                })
                .collect::<Vec<_>>(),
            Some(0.0),
        );
    }

    let mut current_max_colors = max_colors;
    let mut trials_left = total_trials;
    let mut best_palette: Option<Vec<PaletteEntry>> = None;
    let mut target_mse_overshoot = if total_trials > 0 { 1.05 } else { 1.0 };
    let mut fails_in_a_row = 0i32;
    let mut palette_error = None::<f64>;

    let mut palette = loop {
        let max_mse_per_color = target_mse
            .max(palette_error.unwrap_or(quality_to_mse(1)))
            .max(quality_to_mse(51))
            * 1.2;
        let mut new_palette = if few_input_colors {
            hist.items
                .iter()
                .map(|item| PaletteEntry {
                    color: item.color,
                    popularity: item.perceptual_weight,
                })
                .collect::<Vec<_>>()
        } else {
            median_cut_palette(
                &mut hist,
                current_max_colors,
                target_mse * target_mse_overshoot,
                max_mse_per_color,
            )
        };

        if trials_left <= 0 {
            break Some(new_palette);
        }

        let first_target_run = best_palette.is_none() && target_mse > 0.0;
        let stats = kmeans_iteration(&mut hist.items, &mut new_palette, !first_target_run);
        if best_palette.is_none()
            || stats.error < palette_error.unwrap_or(f64::MAX)
            || (stats.error <= target_mse && new_palette.len() < current_max_colors)
        {
            if stats.error < target_mse && stats.error > 0.0 {
                target_mse_overshoot = (target_mse_overshoot * 1.25)
                    .min(target_mse / stats.error.max(f64::MIN_POSITIVE));
            }
            palette_error = Some(stats.error);
            current_max_colors = current_max_colors.min(new_palette.len().saturating_add(1));
            trials_left -= 1;
            fails_in_a_row = 0;
            best_palette = Some(new_palette);
        } else {
            fails_in_a_row += 1;
            target_mse_overshoot = 1.0;
            trials_left -= 5 + fails_in_a_row;
        }

        if trials_left <= 0 {
            break best_palette;
        }
    }
    .unwrap_or_else(|| {
        if few_input_colors {
            hist.items
                .iter()
                .map(|item| PaletteEntry {
                    color: item.color,
                    popularity: item.perceptual_weight,
                })
                .collect::<Vec<_>>()
        } else {
            median_cut_palette(&mut hist, max_colors, f64::INFINITY, f64::INFINITY)
        }
    });

    let final_iterations = effective_kmeans_iterations(
        settings.kmeans_iterations,
        hist_items,
        palette_error.is_some(),
        has_quality_target,
    );
    refine_palette(
        &mut hist.items,
        &mut palette,
        final_iterations,
        final_iteration_limit,
        max_mse,
        &mut palette_error,
    );
    (palette, palette_error)
}

fn effective_target_mse(
    settings: &QuantizerSettings,
    hist_items: usize,
    max_colors: usize,
) -> (Option<f64>, f64, bool) {
    let max_mse = settings
        .max_mse
        .map(|mse| mse * if hist_items <= max_colors { 0.33 } else { 1.0 });
    let mut target_mse = settings
        .target_mse
        .max((f64::from(1u16 << settings.output_posterize_bits) / 1024.0).powi(2));
    if let Some(limit) = max_mse {
        target_mse = target_mse.min(limit);
    }
    (max_mse, target_mse, settings.target_mse_is_zero)
}

#[derive(Debug, Clone, Copy)]
struct PaletteStats {
    error: f64,
}
#[derive(Debug, Clone)]
struct KmeansAccumulator {
    sums: Vec<[f64; 4]>,
    weights: Vec<f64>,
    total_error: f64,
}

impl KmeansAccumulator {
    fn new(palette_len: usize) -> Self {
        Self {
            sums: vec![[0.0f64; 4]; palette_len],
            weights: vec![0.0f64; palette_len],
            total_error: 0.0,
        }
    }

    fn merge(mut self, other: Self) -> Self {
        self.total_error += other.total_error;
        for (lhs, rhs) in self.sums.iter_mut().zip(other.sums) {
            lhs[0] += rhs[0];
            lhs[1] += rhs[1];
            lhs[2] += rhs[2];
            lhs[3] += rhs[3];
        }
        for (lhs, rhs) in self.weights.iter_mut().zip(other.weights) {
            *lhs += rhs;
        }
        self
    }
}

fn refine_palette(
    histogram: &mut [HistItem],
    palette: &mut [PaletteEntry],
    iterations: u16,
    iteration_limit: f64,
    max_mse: Option<f64>,
    palette_error: &mut Option<f64>,
) {
    if palette.is_empty() {
        *palette_error = Some(0.0);
        return;
    }

    if iterations == 0 {
        return;
    }

    let mut iteration = 0u16;
    while iteration < iterations {
        let stats = kmeans_iteration(histogram, palette, false);
        let previous_error = *palette_error;
        *palette_error = Some(stats.error);
        if let Some(previous_error) = previous_error {
            if (previous_error - stats.error).abs() < iteration_limit {
                break;
            }
        }
        iteration += if stats.error > max_mse.unwrap_or(1e20) * 1.5 {
            2
        } else {
            1
        };
    }
}

fn kmeans_iteration(
    histogram: &mut [HistItem],
    palette: &mut [PaletteEntry],
    adjust_weights: bool,
) -> PaletteStats {
    let palette_points = palette.iter().map(|entry| entry.color).collect::<Vec<_>>();
    let palette_popularities = palette
        .iter()
        .map(|entry| entry.popularity)
        .collect::<Vec<_>>();
    let tree = NearestTree::new_with_popularity(&palette_points, Some(&palette_popularities));
    let total_weight = histogram
        .iter()
        .map(|item| f64::from(item.perceptual_weight))
        .sum::<f64>()
        .max(1e-9);

    let acc = histogram
        .par_chunks_mut(256)
        .fold(
            || KmeansAccumulator::new(palette.len()),
            |mut acc, batch| {
                kmeans_iteration_batch(batch, &tree, palette, adjust_weights, &mut acc);
                acc
            },
        )
        .reduce(
            || KmeansAccumulator::new(palette.len()),
            KmeansAccumulator::merge,
        );

    for idx in 0..palette.len() {
        palette[idx].popularity = acc.weights[idx] as f32;
        if acc.weights[idx] == 0.0 {
            continue;
        }
        if palette[idx].color.a == 0.0 {
            continue;
        }
        palette[idx].color = InternalPixel {
            a: (acc.sums[idx][0] / acc.weights[idx]) as f32,
            r: (acc.sums[idx][1] / acc.weights[idx]) as f32,
            g: (acc.sums[idx][2] / acc.weights[idx]) as f32,
            b: (acc.sums[idx][3] / acc.weights[idx]) as f32,
        };
    }

    replace_unused_palette_entries(histogram, palette);

    PaletteStats {
        error: acc.total_error / total_weight,
    }
}

fn kmeans_iteration_batch(
    batch: &mut [HistItem],
    tree: &NearestTree<'_>,
    palette: &[PaletteEntry],
    adjust_weights: bool,
    acc: &mut KmeansAccumulator,
) {
    for item in batch.iter_mut() {
        let hint = usize::from(item.likely_palette_index).min(palette.len().saturating_sub(1));
        let (nearest, diff_sq) = tree.search(item.color, hint);
        let mut diff = f64::from(diff_sq);
        item.likely_palette_index = nearest as u16;

        if adjust_weights {
            let reflected = reflected_color(item.color, palette[nearest].color);
            let reflected_diff = f64::from(tree.search(reflected, nearest).1);
            diff = reflected_diff;
            item.adjusted_weight = ((2.0 * item.adjusted_weight) + item.perceptual_weight)
                * (0.5 + reflected_diff as f32);
        }

        acc.total_error += diff * f64::from(item.perceptual_weight);
        let weight = f64::from(item.adjusted_weight);
        acc.weights[nearest] += weight;
        acc.sums[nearest][0] += f64::from(item.color.a) * weight;
        acc.sums[nearest][1] += f64::from(item.color.r) * weight;
        acc.sums[nearest][2] += f64::from(item.color.g) * weight;
        acc.sums[nearest][3] += f64::from(item.color.b) * weight;
    }
}

fn replace_unused_palette_entries(histogram: &[HistItem], palette: &mut [PaletteEntry]) {
    for pal_idx in 0..palette.len() {
        if palette[pal_idx].popularity > 0.0 {
            continue;
        }

        let worst_idx = {
            let palette_points = palette.iter().map(|entry| entry.color).collect::<Vec<_>>();
            let palette_popularities = palette
                .iter()
                .map(|entry| entry.popularity)
                .collect::<Vec<_>>();
            let tree =
                NearestTree::new_with_popularity(&palette_points, Some(&palette_popularities));
            let mut worst_idx = None;
            let mut worst_diff = 0.0f32;
            for (item_idx, item) in histogram.iter().enumerate() {
                let hint =
                    usize::from(item.likely_palette_index).min(palette.len().saturating_sub(1));
                let may_be_worst = palette
                    .get(hint)
                    .map_or(true, |pal| pal.color.diff(item.color) > worst_diff);
                if !may_be_worst {
                    continue;
                }

                let diff = tree.search(item.color, hint).1;
                if diff > worst_diff {
                    worst_diff = diff;
                    worst_idx = Some(item_idx);
                }
            }
            worst_idx
        };

        if let Some(worst_idx) = worst_idx {
            palette[pal_idx] = PaletteEntry {
                color: histogram[worst_idx].color,
                popularity: histogram[worst_idx].adjusted_weight,
            };
        }
    }
}

fn sort_palette_entries(palette: &mut [PaletteEntry]) {
    palette.sort_by(|left, right| {
        let left_transparent = left.color.to_rgba(SRGB_OUTPUT_GAMMA)[3] < 255;
        let right_transparent = right.color.to_rgba(SRGB_OUTPUT_GAMMA)[3] < 255;
        right_transparent.cmp(&left_transparent).then_with(|| {
            right
                .popularity
                .partial_cmp(&left.popularity)
                .unwrap_or(Ordering::Equal)
        })
    });
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
        a: color.a + (color.a - mapped.a),
        r: color.r + (color.r - mapped.r),
        g: color.g + (color.g - mapped.g),
        b: color.b + (color.b - mapped.b),
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
        Self::new_with_popularity(points, None)
    }

    fn new_with_popularity(points: &'a [InternalPixel], popularities: Option<&[f32]>) -> Self {
        debug_assert!(!points.is_empty());
        let mut indexes = (0..points.len()).collect::<Vec<_>>();
        let root = build_search_node(points, &mut indexes, popularities);
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

fn build_search_node(
    points: &[InternalPixel],
    indexes: &mut [usize],
    popularities: Option<&[f32]>,
) -> SearchNode {
    debug_assert!(!indexes.is_empty());
    if indexes.len() == 1 {
        let idx = indexes[0];
        return SearchNode {
            idx,
            vantage_point: points[idx],
            inner: SearchNodeInner::Leaf { idxs: Box::new([]) },
        };
    }

    if let Some(popularities) = popularities {
        if let Some((most_popular, _)) =
            indexes
                .iter()
                .enumerate()
                .max_by(|(_, left_idx), (_, right_idx)| {
                    let left = popularities.get(**left_idx).copied().unwrap_or_default();
                    let right = popularities.get(**right_idx).copied().unwrap_or_default();
                    left.partial_cmp(&right).unwrap_or(Ordering::Equal)
                })
        {
            indexes.swap(0, most_popular);
        }
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
            near: Box::new(build_search_node(points, near_idx, popularities)),
            far: Box::new(build_search_node(points, far_idx, popularities)),
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

fn remap_image(
    rgba: &[u8],
    width: usize,
    height: usize,
    palette: &[(InternalPixel, [u8; 4])],
    palette_error: Option<f64>,
    settings: QuantizerSettings,
    importance_map: Option<&[u8]>,
    edges_map: Option<&[u8]>,
    contrast_pixels: Option<&[InternalPixel]>,
) -> IndexedImage {
    let (palette, mut indices, counts) = if settings.dither {
        remap_image_dithered(
            rgba,
            width,
            height,
            palette,
            palette_error,
            settings,
            importance_map,
            edges_map,
            contrast_pixels,
        )
    } else {
        remap_image_plain(
            rgba,
            width,
            palette,
            settings.output_posterize_bits,
            importance_map,
            contrast_pixels,
        )
    };

    let order = (0..palette.len())
        .filter(|&idx| counts[idx] > 0)
        .collect::<Vec<_>>();

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
    width: usize,
    palette: &[(InternalPixel, [u8; 4])],
    output_posterize_bits: u8,
    importance_map: Option<&[u8]>,
    contrast_pixels: Option<&[InternalPixel]>,
) -> (Vec<(InternalPixel, [u8; 4])>, Vec<u8>, Vec<usize>) {
    let mut palette_points = palette.iter().map(|entry| entry.0).collect::<Vec<_>>();
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

    // Reference dither_level=0 path: init_int_palette() first, then remap_to_palette().
    // init_int_palette rounds palette and overwrites f_pixel values.
    // remap_to_palette uses rounded palette for nearest search + k-means finalize.
    // The int_palette (output RGBA) is set before finalize, so finalize doesn't affect output.
    let output_palette =
        round_palette_for_output_in_place(&mut palette_points, output_posterize_bits);
    let final_pass = finalize_plain_remap(width, pixels, &mut palette_points, importance_map);

    let remapped_palette = palette_points
        .into_iter()
        .zip(output_palette)
        .map(|(color, rgba)| (color, rgba))
        .collect::<Vec<_>>();

    (remapped_palette, final_pass.indices, final_pass.counts)
}

struct PlainRemapPass {
    indices: Vec<u8>,
    counts: Vec<usize>,
    sums: Vec<[f64; 4]>,
    weights: Vec<f64>,
    palette_error: f64,
}

fn remap_image_plain_pass(
    width: usize,
    pixels: &[InternalPixel],
    palette_points: &[InternalPixel],
    importance_map: Option<&[u8]>,
) -> PlainRemapPass {
    let mut indices = Vec::with_capacity(pixels.len());
    let mut counts = vec![0usize; palette_points.len()];
    let mut sums = vec![[0.0f64; 4]; palette_points.len()];
    let mut weights = vec![0.0f64; palette_points.len()];
    let mut total_error = 0.0f64;
    let tree = NearestTree::new(palette_points);
    for (pixel_idx, color) in pixels.iter().copied().enumerate() {
        let row_offset = pixel_idx % width;
        let last_idx = if row_offset == 0 {
            0usize
        } else {
            indices[pixel_idx - 1] as usize
        };
        let (idx, diff) = tree.search(color, last_idx);
        counts[idx] += 1;
        total_error += f64::from(diff);
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
        indices.push(idx as u8);
    }

    PlainRemapPass {
        indices,
        counts,
        sums,
        weights,
        palette_error: total_error / pixels.len().max(1) as f64,
    }
}

fn finalize_plain_remap(
    width: usize,
    pixels: &[InternalPixel],
    palette_points: &mut [InternalPixel],
    importance_map: Option<&[u8]>,
) -> PlainRemapPass {
    let feedback = remap_image_plain_pass(width, pixels, palette_points, importance_map);
    if palette_points.len() <= 1 {
        return feedback;
    }

    apply_remap_feedback(palette_points, &feedback);
    feedback
}

fn apply_remap_feedback(palette_points: &mut [InternalPixel], pass: &PlainRemapPass) {
    for idx in 0..palette_points.len() {
        if pass.weights[idx] == 0.0 {
            continue;
        }
        if palette_points[idx].a == 0.0 {
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
    palette_error: Option<f64>,
    settings: QuantizerSettings,
    importance_map: Option<&[u8]>,
    edges_map: Option<&[u8]>,
    contrast_pixels: Option<&[InternalPixel]>,
) -> (Vec<(InternalPixel, [u8; 4])>, Vec<u8>, Vec<usize>) {
    let mut palette_points = palette.iter().map(|entry| entry.0).collect::<Vec<_>>();
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
    let is_image_huge = width.saturating_mul(height) > 2000 * 2000;
    let generate_dither_map = settings.use_dither_map == DitherMapMode::Always
        || (!is_image_huge && settings.use_dither_map != DitherMapMode::None);

    // Always run plain remap + k-means finalize before dithering.
    // Reference remap_to_palette() always calls kmeans.finalize(palette).
    // When generate_dither_map is true, finalize_plain_remap does remap+feedback+re-remap.
    // When false, we still need the k-means feedback to refine palette colors.
    let plain_pass = if generate_dither_map {
        Some(finalize_plain_remap(
            width,
            pixels,
            &mut palette_points,
            importance_map,
        ))
    } else if palette_points.len() > 1 {
        // K-Means finalize even without dither map generation
        let feedback = remap_image_plain_pass(width, pixels, &palette_points, importance_map);
        apply_remap_feedback(&mut palette_points, &feedback);
        Some(remap_image_plain_pass(
            width,
            pixels,
            &palette_points,
            importance_map,
        ))
    } else {
        None
    };
    let output_image_is_remapped = plain_pass.is_some();

    // Output RGBA generated AFTER k-means finalize, matching reference init_int_palette() timing
    let output_palette =
        round_palette_for_output_in_place(&mut palette_points, settings.output_posterize_bits);

    let dither_map = select_dither_map(
        pixels,
        width,
        height,
        plain_pass.as_ref().filter(|_| generate_dither_map),
        &palette_points,
        settings.use_dither_map,
        generate_dither_map,
        edges_map,
    );
    let tree = NearestTree::new(&palette_points);
    let mut indices = plain_pass
        .as_ref()
        .filter(|_| output_image_is_remapped)
        .map(|pass| pass.indices.clone())
        .unwrap_or_else(|| vec![0u8; pixels.len()]);
    let mut base_dithering_level = (1.0 - settings.dither_level)
        .mul_add(-(1.0 - settings.dither_level), 1.0)
        * (15.0f32 / 16.0f32);
    if !dither_map.is_empty() {
        base_dithering_level *= 1.0 / 255.0;
    }
    let max_dither_error = plain_pass
        .as_ref()
        .map(|pass| pass.palette_error)
        .or(palette_error)
        .unwrap_or(quality_to_mse(80))
        .mul_add(2.4, 0.0)
        .max(quality_to_mse(35)) as f32;
    remap_image_dithered_rows(
        pixels,
        width,
        height,
        &tree,
        &palette_points,
        &dither_map,
        base_dithering_level,
        max_dither_error,
        output_image_is_remapped,
        plain_pass.as_ref().map(|pass| pass.indices.as_slice()),
        &mut indices,
    );
    let mut counts = vec![0usize; palette.len()];
    for &idx in &indices {
        counts[idx as usize] += 1;
    }

    let remapped_palette = palette_points
        .into_iter()
        .zip(output_palette)
        .map(|(color, rgba)| (color, rgba))
        .collect::<Vec<_>>();

    (remapped_palette, indices, counts)
}

fn remap_image_dithered_rows(
    pixels: &[InternalPixel],
    width: usize,
    height: usize,
    tree: &NearestTree<'_>,
    palette_points: &[InternalPixel],
    dither_map: &[u8],
    base_dithering_level: f32,
    max_dither_error: f32,
    output_image_is_remapped: bool,
    plain_indices: Option<&[u8]>,
    indices: &mut [u8],
) {
    let num_chunks = effective_dither_chunks(width, height);
    let chunk_height = (height + num_chunks - 1) / num_chunks;
    indices
        .par_chunks_mut(chunk_height.saturating_mul(width).max(width))
        .enumerate()
        .for_each(|(chunk_idx, chunk)| {
            let start_row = chunk_idx * chunk_height;
            let rows_in_chunk = chunk.len() / width;
            if rows_in_chunk == 0 {
                return;
            }
            let mut next_errors = vec![InternalPixel::default(); width + 2];
            let mut curr_errors = vec![InternalPixel::default(); width + 2];
            let mut discard = vec![0u8; width];

            if start_row > 2 {
                for row in (start_row - 2)..start_row {
                    let row_pixels = &pixels[row * width..][..width];
                    let row_map = dither_map
                        .get(row * width..row * width + width)
                        .unwrap_or(&[]);
                    if output_image_is_remapped {
                        if let Some(plain_indices) = plain_indices {
                            discard.copy_from_slice(&plain_indices[row * width..][..width]);
                        }
                    } else {
                        discard.fill(0);
                    }
                    dither_row(
                        row_pixels,
                        &mut discard,
                        row_map,
                        base_dithering_level,
                        max_dither_error,
                        tree,
                        palette_points,
                        output_image_is_remapped,
                        &mut curr_errors,
                        &mut next_errors,
                        row % 2 == 0,
                    );
                }
            }

            for local_row in 0..rows_in_chunk {
                let row = start_row + local_row;
                let row_pixels = &pixels[row * width..][..width];
                let row_map = dither_map
                    .get(row * width..row * width + width)
                    .unwrap_or(&[]);
                let row_indices = &mut chunk[local_row * width..][..width];
                if output_image_is_remapped {
                    if let Some(plain_indices) = plain_indices {
                        row_indices.copy_from_slice(&plain_indices[row * width..][..width]);
                    }
                }
                dither_row(
                    row_pixels,
                    row_indices,
                    row_map,
                    base_dithering_level,
                    max_dither_error,
                    tree,
                    palette_points,
                    output_image_is_remapped,
                    &mut curr_errors,
                    &mut next_errors,
                    row % 2 == 0,
                );
            }
        });
}

fn effective_dither_chunks(width: usize, height: usize) -> usize {
    if height <= 128 {
        return 1;
    }
    let suggested = (width.saturating_mul(height) / 524_288)
        .min(height / 128)
        .max(2)
        .min(rayon::current_num_threads());
    suggested.max(1)
}

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
) {
    std::mem::swap(curr_errors, next_errors);
    next_errors.fill(InternalPixel::default());

    let width = row_pixels.len();
    let mut last_match = 0usize;
    for offset in 0..width {
        let x = if even_row { offset } else { width - 1 - offset };
        let mut dither_level = base_dithering_level;
        if let Some(&level) = dither_map.get(x) {
            dither_level *= f32::from(level);
        }

        let color = get_dithered_pixel(
            dither_level,
            max_dither_error,
            curr_errors[x + 1],
            row_pixels[x],
        );

        let guessed_match = if output_image_is_remapped {
            output_row[x] as usize
        } else {
            last_match
        };
        let (pal_idx, _) = tree.search(color, guessed_match);
        last_match = pal_idx;
        output_row[x] = pal_idx as u8;

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

        if even_row {
            add_scaled_error(&mut curr_errors[x + 2], diff, 7.0 / 16.0);
            add_scaled_error(&mut next_errors[x], diff, 3.0 / 16.0);
            add_scaled_error(&mut next_errors[x + 1], diff, 5.0 / 16.0);
            next_errors[x + 2] = scaled_error(diff, 1.0 / 16.0);
        } else {
            add_scaled_error(&mut curr_errors[x], diff, 7.0 / 16.0);
            next_errors[x] = scaled_error(diff, 1.0 / 16.0);
            add_scaled_error(&mut next_errors[x + 1], diff, 5.0 / 16.0);
            add_scaled_error(&mut next_errors[x + 2], diff, 3.0 / 16.0);
        }
    }
}

fn select_dither_map(
    pixels: &[InternalPixel],
    width: usize,
    height: usize,
    plain_pass: Option<&PlainRemapPass>,
    palette_points: &[InternalPixel],
    use_dither_map: DitherMapMode,
    generate_dither_map: bool,
    edges_map: Option<&[u8]>,
) -> Vec<u8> {
    if generate_dither_map {
        let generated = plain_pass
            .map(|plain_pass| {
                build_dither_map(pixels, width, height, &plain_pass.indices, palette_points)
            })
            .unwrap_or_default();
        if !generated.is_empty() {
            return generated;
        }
    }

    if use_dither_map != DitherMapMode::None {
        return edges_map.map_or_else(Vec::new, |edges| edges.to_vec());
    }

    Vec::new()
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

fn build_contrast_maps(
    pixels: &[InternalPixel],
    width: usize,
    height: usize,
) -> Option<ContrastMaps> {
    if width < 4 || height < 4 || pixels.len() != width.saturating_mul(height) {
        return None;
    }
    let (importance_map, edges) = compute_contrast_maps(pixels, width, height);
    Some(ContrastMaps {
        importance_map,
        edges,
    })
}

fn build_dither_map(
    pixels: &[InternalPixel],
    width: usize,
    height: usize,
    remapped_indices: &[u8],
    _palette: &[InternalPixel],
) -> Vec<u8> {
    if width < 4 || height < 4 || pixels.len() != width.saturating_mul(height) {
        return Vec::new();
    }

    let (_, mut edges) = compute_contrast_maps(pixels, width, height);
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
                    edges[row_start + i] = (f32::from(u16::from(edges[row_start + i]) + 128)
                        * (255.0 / (255.0 + 128.0))
                        * (1.0 - 20.0 / (20.0 + neighbor_count as f32)))
                        as u8;
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

fn scaled_error(diff: InternalPixel, scale: f32) -> InternalPixel {
    InternalPixel {
        a: diff.a * scale,
        r: diff.r * scale,
        g: diff.g * scale,
        b: diff.b * scale,
    }
}

fn round_palette_for_output_in_place(
    palette_points: &mut [InternalPixel],
    output_posterize_bits: u8,
) -> Vec<[u8; 4]> {
    let gamma = gamma_lut(SRGB_OUTPUT_GAMMA);
    palette_points
        .iter_mut()
        .map(|point| {
            let mut rgba = point
                .to_rgba(SRGB_OUTPUT_GAMMA)
                .map(|channel| posterize_output_channel(channel, output_posterize_bits));
            *point = InternalPixel::from_rgba(&gamma, &rgba);
            if rgba[3] == 0 {
                rgba[0] = 71;
                rgba[1] = 112;
                rgba[2] = 76;
            }
            rgba
        })
        .collect()
}

fn color_components(color: InternalPixel) -> [f32; 4] {
    [color.a, color.r, color.g, color.b]
}

fn cluster_index(rgba: [u8; 4]) -> u8 {
    ((rgba[0] >> 7) << 3) | ((rgba[1] >> 7) << 2) | ((rgba[2] >> 7) << 1) | (rgba[3] >> 7)
}

fn pack_rgba_key(rgba: &[u8], posterize_bits: u8) -> u32 {
    let mut px = posterized_rgba(rgba, posterize_bits);
    if px[3] == 0 {
        px = [0, 0, 0, 0];
    }
    u32::from_ne_bytes(px)
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

fn posterize_output_channel(channel: u8, bits: u8) -> u8 {
    if bits == 0 {
        channel
    } else {
        (channel & !((1u8 << bits) - 1)) | (channel >> (8 - bits))
    }
}

#[cfg(test)]
mod tests {
    use crate::quality::SpeedSettings;

    use super::{
        InternalPixel, QuantizerSettings, apply_remap_feedback, gamma_lut, quantize_indexed,
        quantizer_settings, remap_image_dithered, remap_image_plain, remap_image_plain_pass,
        select_dither_map,
    };
    use crate::quality::{DitherMapMode, SRGB_OUTPUT_GAMMA};

    #[test]
    fn quantize_indexed_runs() {
        let rgba = vec![
            255u8, 0, 0, 255, 250, 0, 0, 255, 0, 255, 0, 255, 0, 250, 0, 255, 0, 0, 255, 255, 0, 0,
            250, 255,
        ];
        let settings =
            quantizer_settings(16, SpeedSettings::from_speed(4), 0.0, None, true, 0, 0.0);
        let out = quantize_indexed(&rgba, 3, 2, settings);
        assert_eq!(out.indices.len(), 6);
        assert!(!out.palette.is_empty());
    }

    #[test]
    fn input_posterize_reduces_palette_variety() {
        let rgba = vec![
            255u8, 0, 0, 255, 254, 1, 0, 255, 253, 2, 0, 255, 252, 3, 0, 255,
        ];
        let mut direct_settings =
            quantizer_settings(16, SpeedSettings::from_speed(4), 0.0, None, true, 0, 0.0);
        direct_settings.input_posterize_bits = 0;
        let direct = quantize_indexed(&rgba, 2, 2, direct_settings);

        let mut posterized_settings =
            quantizer_settings(16, SpeedSettings::from_speed(4), 0.0, None, true, 0, 0.0);
        posterized_settings.input_posterize_bits = 2;
        let posterized = quantize_indexed(&rgba, 2, 2, posterized_settings);

        assert!(posterized.palette.len() <= direct.palette.len());
    }

    #[test]
    fn histogram_matches_reference_single_step_posterize_bump() {
        let rgba = [240u8, 242, 244]
            .into_iter()
            .flat_map(|r| [128u8, 130].into_iter().flat_map(move |g| [r, g, 64, 255]))
            .collect::<Vec<_>>();
        let gamma = gamma_lut(SRGB_OUTPUT_GAMMA);
        let mut expected_map = super::build_histogram_map(&rgba, None);
        super::reposterize_histogram_map(&mut expected_map, 1);
        let expected = super::finalize_histogram(expected_map, &gamma);
        let histogram = super::build_histogram(&rgba, 8, 0, 4, &gamma, None);

        assert_eq!(histogram.items.len(), expected.items.len());
        assert!(histogram.items.len() > 4);
    }

    #[test]
    fn palette_respects_max_colors() {
        let rgba = (0..64u8)
            .flat_map(|v| [v, 255 - v, v / 2, 255])
            .collect::<Vec<_>>();
        let settings = quantizer_settings(4, SpeedSettings::from_speed(4), 0.0, None, true, 0, 0.0);
        let out = quantize_indexed(&rgba, 8, 8, settings);
        assert!(out.palette.len() <= 4);
    }

    #[test]
    fn plain_remap_feedback_uses_importance_weights() {
        let rgba = vec![255u8, 0, 0, 255, 0, 0, 255, 255];
        let gamma = gamma_lut(SRGB_OUTPUT_GAMMA);
        let pixels = rgba
            .chunks_exact(4)
            .map(|px| InternalPixel::from_rgba(&gamma, px))
            .collect::<Vec<_>>();
        let mut palette_points = vec![InternalPixel::from_rgba(&gamma, &[0, 0, 0, 255])];
        let pass = remap_image_plain_pass(2, &pixels, &palette_points, Some(&[255, 1]));
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
            output_posterize_bits: 0,
            max_histogram_entries: 256,
            kmeans_iterations: 1,
            kmeans_iteration_limit: 1e-7,
            feedback_loop_trials: 1,
            target_mse: 0.0,
            max_mse: None,
            target_mse_is_zero: true,
            dither: true,
            dither_level: 1.0,
            use_dither_map: DitherMapMode::Enabled,
            use_contrast_maps: false,
        };

        let pixels = rgba
            .chunks_exact(4)
            .map(|px| InternalPixel::from_rgba(&gamma, px))
            .collect::<Vec<_>>();
        let (plain_palette, _, _) =
            super::remap_image_plain(&rgba, 2, &palette, 0, Some(&[255, 1]), Some(&pixels));
        let (palette, indices, counts) = remap_image_dithered(
            &rgba,
            2,
            1,
            &palette,
            None,
            settings,
            Some(&[255, 1]),
            None,
            None,
        );

        assert_eq!(indices.len(), 2);
        assert_eq!(counts[0], 2);
        assert_eq!(palette[0].1, plain_palette[0].1);
    }

    #[test]
    fn huge_image_dithering_falls_back_to_edges_map() {
        let pixels = vec![InternalPixel::default(); 4];
        let edges = vec![9u8, 8, 7, 6];
        let dither_map = select_dither_map(
            &pixels,
            2,
            2,
            None,
            &[],
            DitherMapMode::Enabled,
            false,
            Some(&edges),
        );

        assert_eq!(dither_map, edges);
    }

    #[test]
    fn empty_generated_dither_map_falls_back_to_edges_map() {
        let edges = vec![5u8, 4, 3, 2];
        let dither_map = select_dither_map(
            &[],
            0,
            0,
            None,
            &[],
            DitherMapMode::Enabled,
            true,
            Some(&edges),
        );

        assert_eq!(dither_map, edges);
    }

    #[test]
    fn plain_remap_uses_reference_output_posterization() {
        let rgba = vec![131u8, 131, 131, 255];
        let gamma = gamma_lut(SRGB_OUTPUT_GAMMA);
        let seed = vec![(
            InternalPixel::from_rgba(&gamma, &[131, 131, 131, 255]),
            [131u8, 131, 131, 255],
        )];

        let (final_palette, _, _) = remap_image_plain(&rgba, 1, &seed, 2, None, None);
        assert_eq!(final_palette[0].1, [130, 130, 130, 255]);
    }

    #[test]
    fn kmeans_without_adjust_weights_preserves_existing_adjusted_weight() {
        let mut histogram = vec![super::HistItem {
            color: InternalPixel {
                a: 0.625,
                r: 0.1,
                g: 0.2,
                b: 0.3,
            },
            adjusted_weight: 7.5,
            perceptual_weight: 1.0,
            mc_color_weight: 0.0,
            sort_value: 0,
            likely_palette_index: 0,
        }];
        let mut palette = vec![super::PaletteEntry {
            color: InternalPixel {
                a: 0.625,
                r: 0.1,
                g: 0.2,
                b: 0.3,
            },
            popularity: 1.0,
        }];

        let _ = super::kmeans_iteration(&mut histogram, &mut palette, false);

        assert_eq!(histogram[0].adjusted_weight, 7.5);
    }

    fn quality_target_test_settings(
        target_mse: f64,
        max_mse: Option<f64>,
        target_mse_is_zero: bool,
        output_posterize_bits: u8,
    ) -> QuantizerSettings {
        QuantizerSettings {
            max_colors: 256,
            input_posterize_bits: 0,
            output_posterize_bits,
            max_histogram_entries: 1 << 20,
            kmeans_iterations: 0,
            kmeans_iteration_limit: 0.0,
            feedback_loop_trials: 0,
            target_mse,
            max_mse,
            target_mse_is_zero,
            dither: false,
            dither_level: 1.0,
            use_dither_map: DitherMapMode::None,
            use_contrast_maps: false,
        }
    }

    fn assert_approx_eq(actual: f64, expected: f64) {
        let delta = (actual - expected).abs();
        assert!(
            delta < 1e-12,
            "expected {expected:.12}, got {actual:.12}, delta={delta:.12}"
        );
    }

    #[test]
    fn target_mse_scales_small_histograms_like_reference() {
        let settings = quality_target_test_settings(0.004, Some(0.009), false, 0);
        let (max_mse, target_mse, aim_perfect) = super::effective_target_mse(&settings, 128, 256);
        assert!(!aim_perfect);
        assert_approx_eq(max_mse.expect("max_mse"), 0.009 * 0.33);
        assert_approx_eq(target_mse, 0.009 * 0.33);
    }

    #[test]
    fn target_mse_preserves_large_histogram_limits() {
        let settings = quality_target_test_settings(0.004, Some(0.009), false, 0);
        let (max_mse, target_mse, _) = super::effective_target_mse(&settings, 2048, 256);
        assert_approx_eq(max_mse.expect("max_mse"), 0.009);
        assert_approx_eq(target_mse, 0.004);
    }

    #[test]
    fn target_mse_respects_output_posterization_floor() {
        let settings = quality_target_test_settings(0.0, None, true, 2);
        let (_, target_mse, aim_perfect) = super::effective_target_mse(&settings, 4096, 256);
        assert!(aim_perfect);
        assert_approx_eq(target_mse, (f64::from(1u16 << 2) / 1024.0).powi(2));
    }

    #[test]
    fn refine_palette_does_not_run_when_iterations_are_zero() {
        let gamma = gamma_lut(SRGB_OUTPUT_GAMMA);
        let mut histogram = vec![super::HistItem {
            color: InternalPixel::from_rgba(&gamma, &[255, 16, 16, 16]),
            adjusted_weight: 1.0,
            perceptual_weight: 1.0,
            mc_color_weight: 1.0,
            sort_value: 0,
            likely_palette_index: 0,
        }];
        let original = InternalPixel::from_rgba(&gamma, &[255, 32, 32, 32]);
        let mut palette = vec![super::PaletteEntry {
            color: original,
            popularity: 1.0,
        }];
        let mut palette_error = Some(0.123);

        super::refine_palette(
            &mut histogram,
            &mut palette,
            0,
            0.0,
            None,
            &mut palette_error,
        );

        assert_eq!(palette.len(), 1);
        assert!((palette[0].color.a - original.a).abs() < f32::EPSILON);
        assert!((palette[0].color.r - original.r).abs() < f32::EPSILON);
        assert!((palette[0].color.g - original.g).abs() < f32::EPSILON);
        assert!((palette[0].color.b - original.b).abs() < f32::EPSILON);
        assert_eq!(palette_error, Some(0.123));
    }

    #[test]
    fn nearest_tree_matches_bruteforce_search() {
        let gamma = gamma_lut(SRGB_OUTPUT_GAMMA);
        let palette_rgba = [
            [255, 255, 255, 255],
            [245, 245, 245, 255],
            [236, 236, 236, 255],
            [205, 205, 205, 255],
            [171, 171, 171, 255],
            [136, 136, 136, 255],
            [106, 106, 106, 255],
            [70, 70, 70, 255],
            [15, 15, 15, 255],
            [0, 0, 0, 0],
            [84, 138, 250, 255],
            [207, 99, 35, 255],
            [205, 140, 74, 255],
        ];
        let palette = palette_rgba
            .iter()
            .map(|rgba| InternalPixel::from_rgba(&gamma, rgba))
            .collect::<Vec<_>>();
        let tree = super::NearestTree::new(&palette);

        let sample_rgba = [
            [250, 250, 250, 255],
            [238, 238, 238, 255],
            [220, 220, 220, 255],
            [190, 190, 190, 255],
            [150, 150, 150, 255],
            [110, 110, 110, 255],
            [80, 80, 80, 255],
            [20, 20, 20, 255],
            [0, 0, 0, 12],
            [0, 0, 0, 48],
            [88, 140, 248, 255],
            [210, 101, 34, 255],
            [210, 141, 70, 255],
        ];

        for likely in 0..palette.len() {
            for rgba in sample_rgba {
                let needle = InternalPixel::from_rgba(&gamma, &rgba);
                let (idx, dist) = tree.search(needle, likely);

                let (expected_idx, expected_dist) = palette
                    .iter()
                    .enumerate()
                    .map(|(idx, point)| (idx, needle.diff(*point)))
                    .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap())
                    .expect("expected nearest palette entry");

                assert_eq!(idx, expected_idx, "likely={likely}, rgba={rgba:?}");
                assert!(
                    (dist - expected_dist).abs() < f32::EPSILON,
                    "likely={likely}, rgba={rgba:?}, expected_dist={expected_dist}, got={dist}"
                );
            }
        }
    }
}
