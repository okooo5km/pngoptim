#[derive(Debug, Clone, Copy)]
pub struct QuantizeSettings {
    pub quality_target: u8,
    pub speed: u8,
    pub dither: bool,
}

pub fn quantize_rgba_in_place(
    pixels: &mut [u8],
    width: usize,
    height: usize,
    settings: QuantizeSettings,
) {
    let rgb_levels = levels_from_quality_speed(settings.quality_target, settings.speed);
    let alpha_levels = (rgb_levels / 2).max(4);

    if settings.dither {
        quantize_with_floyd_steinberg(pixels, width, height, rgb_levels, alpha_levels);
    } else {
        quantize_without_dither(pixels, rgb_levels, alpha_levels);
    }
}

fn levels_from_quality_speed(quality_target: u8, speed: u8) -> u8 {
    let quality_component = 2 + (i32::from(quality_target) * 30 / 100); // 2..32
    let speed_penalty = (i32::from(speed).saturating_sub(1) * 8) / 10; // 0..8
    (quality_component - speed_penalty).clamp(2, 32) as u8
}

fn quantize_without_dither(pixels: &mut [u8], rgb_levels: u8, alpha_levels: u8) {
    for px in pixels.chunks_exact_mut(4) {
        px[0] = quantize_channel(px[0] as f32, rgb_levels);
        px[1] = quantize_channel(px[1] as f32, rgb_levels);
        px[2] = quantize_channel(px[2] as f32, rgb_levels);
        px[3] = quantize_channel(px[3] as f32, alpha_levels);
    }
}

fn quantize_with_floyd_steinberg(
    pixels: &mut [u8],
    width: usize,
    height: usize,
    rgb_levels: u8,
    alpha_levels: u8,
) {
    let mut curr = vec![[0.0f32; 3]; width + 2];
    let mut next = vec![[0.0f32; 3]; width + 2];

    for y in 0..height {
        for x in 0..width {
            let idx = (y * width + x) * 4;

            for c in 0..3 {
                let old = pixels[idx + c] as f32 + curr[x + 1][c];
                let new_val = quantize_channel(old, rgb_levels);
                pixels[idx + c] = new_val;

                let error = old - new_val as f32;
                curr[x + 2][c] += error * (7.0 / 16.0);
                next[x][c] += error * (3.0 / 16.0);
                next[x + 1][c] += error * (5.0 / 16.0);
                next[x + 2][c] += error * (1.0 / 16.0);
            }

            pixels[idx + 3] = quantize_channel(pixels[idx + 3] as f32, alpha_levels);
        }

        std::mem::swap(&mut curr, &mut next);
        for cell in &mut next {
            *cell = [0.0; 3];
        }
    }
}

fn quantize_channel(value: f32, levels: u8) -> u8 {
    if levels <= 1 {
        return value.clamp(0.0, 255.0).round() as u8;
    }

    let value = value.clamp(0.0, 255.0);
    let max_index = (levels - 1) as f32;
    let level = (value / 255.0 * max_index).round();
    ((level / max_index) * 255.0).round().clamp(0.0, 255.0) as u8
}

#[cfg(test)]
mod tests {
    use super::{QuantizeSettings, levels_from_quality_speed, quantize_rgba_in_place};

    #[test]
    fn levels_stay_in_range() {
        for quality in 0..=100 {
            for speed in 1..=11 {
                let levels = levels_from_quality_speed(quality, speed);
                assert!((2..=32).contains(&levels));
            }
        }
    }

    #[test]
    fn quantize_runs() {
        let mut pixels = vec![120u8, 150, 210, 255, 90, 40, 200, 180];
        quantize_rgba_in_place(
            &mut pixels,
            2,
            1,
            QuantizeSettings {
                quality_target: 80,
                speed: 4,
                dither: true,
            },
        );
        assert_eq!(pixels.len(), 8);
    }
}
