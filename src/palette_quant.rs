const BUCKET_BITS: usize = 19;
const BUCKET_COUNT: usize = 1 << BUCKET_BITS;
const INVALID_SLOT: u16 = u16::MAX;

#[derive(Debug, Clone)]
pub struct IndexedImage {
    pub palette: Vec<[u8; 4]>,
    pub indices: Vec<u8>,
    pub mean_abs_diff: f64,
}

pub fn quantize_indexed(
    rgba: &[u8],
    width: usize,
    height: usize,
    max_colors: usize,
) -> IndexedImage {
    let pixel_count = width.saturating_mul(height);
    if pixel_count == 0 {
        return IndexedImage {
            palette: vec![[0, 0, 0, 0]],
            indices: Vec::new(),
            mean_abs_diff: 0.0,
        };
    }

    let max_colors = max_colors.clamp(2, 256);

    let mut hist = vec![0u32; BUCKET_COUNT];
    for px in rgba.chunks_exact(4) {
        let key = bucket_color_key(px[0], px[1], px[2], px[3]) as usize;
        hist[key] = hist[key].saturating_add(1);
    }

    let mut freq: Vec<(u32, u32)> = hist
        .iter()
        .enumerate()
        .filter_map(|(key, count)| (*count > 0).then_some((key as u32, *count)))
        .collect();
    freq.sort_by(|a, b| b.1.cmp(&a.1));
    freq.truncate(max_colors);

    let mut palette: Vec<[u8; 4]> = freq
        .iter()
        .map(|(key, _)| decode_bucket_color(*key))
        .collect();
    if palette.is_empty() {
        palette.push([0, 0, 0, 0]);
    }

    let mut bucket_to_index = vec![INVALID_SLOT; BUCKET_COUNT];
    for (idx, color) in palette.iter().enumerate() {
        let key = bucket_color_key(color[0], color[1], color[2], color[3]) as usize;
        bucket_to_index[key] = idx as u16;
    }

    let mut nearest_cache = vec![INVALID_SLOT; BUCKET_COUNT];
    let mut indices = Vec::with_capacity(pixel_count);
    let mut total_abs_diff: u64 = 0;

    for px in rgba.chunks_exact(4) {
        let key = bucket_color_key(px[0], px[1], px[2], px[3]) as usize;
        let idx = if bucket_to_index[key] != INVALID_SLOT {
            bucket_to_index[key] as u8
        } else if nearest_cache[key] != INVALID_SLOT {
            nearest_cache[key] as u8
        } else {
            let idx = nearest_palette_index(px[0], px[1], px[2], px[3], &palette);
            nearest_cache[key] = idx as u16;
            idx
        };

        let q = palette[idx as usize];
        total_abs_diff += px[0].abs_diff(q[0]) as u64;
        total_abs_diff += px[1].abs_diff(q[1]) as u64;
        total_abs_diff += px[2].abs_diff(q[2]) as u64;
        total_abs_diff += px[3].abs_diff(q[3]) as u64;
        indices.push(idx);
    }

    let mean_abs_diff = total_abs_diff as f64 / (pixel_count as f64 * 4.0);

    IndexedImage {
        palette,
        indices,
        mean_abs_diff,
    }
}

pub fn max_colors_from_quality_speed(quality_target: u8, speed: u8) -> usize {
    let quality_component = 16 + (usize::from(quality_target) * 180 / 100); // 16..196
    let speed_penalty = usize::from(speed.saturating_sub(1)) * 14; // 0..140
    quality_component
        .saturating_sub(speed_penalty)
        .clamp(16, 256)
}

fn bucket_color_key(r: u8, g: u8, b: u8, a: u8) -> u32 {
    let rb = r >> 3; // 5 bits
    let gb = g >> 3; // 5 bits
    let bb = b >> 3; // 5 bits
    let ab = a >> 4; // 4 bits
    ((rb as u32) << 14) | ((gb as u32) << 9) | ((bb as u32) << 4) | (ab as u32)
}

fn decode_bucket_color(key: u32) -> [u8; 4] {
    let rb = ((key >> 14) & 0x1f) as u8;
    let gb = ((key >> 9) & 0x1f) as u8;
    let bb = ((key >> 4) & 0x1f) as u8;
    let ab = (key & 0x0f) as u8;

    let r = (rb << 3) | 0x04;
    let g = (gb << 3) | 0x04;
    let b = (bb << 3) | 0x04;
    let a = (ab << 4) | 0x08;
    [r, g, b, a]
}

fn nearest_palette_index(r: u8, g: u8, b: u8, a: u8, palette: &[[u8; 4]]) -> u8 {
    let mut best_idx = 0usize;
    let mut best_dist = u32::MAX;

    for (idx, c) in palette.iter().enumerate() {
        let dr = r as i32 - c[0] as i32;
        let dg = g as i32 - c[1] as i32;
        let db = b as i32 - c[2] as i32;
        let da = a as i32 - c[3] as i32;
        let dist =
            (dr * dr * 3 + dg * dg * 4 + db * db * 2 + da * da * 2).clamp(0, i32::MAX) as u32;
        if dist < best_dist {
            best_dist = dist;
            best_idx = idx;
        }
    }

    best_idx as u8
}

#[cfg(test)]
mod tests {
    use super::{max_colors_from_quality_speed, quantize_indexed};

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
        let out = quantize_indexed(&rgba, 3, 2, 16);
        assert_eq!(out.indices.len(), 6);
        assert!(!out.palette.is_empty());
    }
}
