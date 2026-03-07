const INTERNAL_GAMMA: f64 = 0.57;
const LIQ_WEIGHT_A: f64 = 0.625;
const LIQ_WEIGHT_R: f64 = 0.5;
const LIQ_WEIGHT_G: f64 = 1.0;
const LIQ_WEIGHT_B: f64 = 0.45;
const LIQ_WEIGHT_MSE: f64 = 0.45;
pub(crate) const SRGB_OUTPUT_GAMMA: f64 = 0.45455;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DitherMapMode {
    None,
    Enabled,
    Always,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpeedSettings {
    pub raw_speed: u8,
    pub effective_speed: u8,
    pub kmeans_iterations: u16,
    pub kmeans_iteration_limit: f64,
    pub feedback_loop_trials: u16,
    pub max_histogram_entries: u32,
    pub input_posterize_bits: u8,
    pub use_dither_map: DitherMapMode,
    pub use_contrast_maps: bool,
    pub single_threaded_dithering: bool,
    pub force_disable_dither: bool,
}

impl SpeedSettings {
    pub fn from_speed(raw_speed: u8) -> Self {
        let force_disable_dither = raw_speed >= 11;
        let effective_speed = raw_speed.min(10);

        let mut iterations = (8 - i32::from(effective_speed)).max(0) as u16;
        iterations += iterations * iterations / 2;
        let kmeans_iteration_limit = 1.0 / f64::from(1u32 << (23 - u32::from(effective_speed)));

        let feedback_loop_trials = (56 - 9 * i32::from(effective_speed)).max(0) as u16;
        let max_histogram_entries = (1 << 17) + (1 << 18) * (10 - u32::from(effective_speed));
        let input_posterize_bits = if effective_speed >= 8 { 1 } else { 0 };

        let mut use_dither_map = if effective_speed <= 6 {
            DitherMapMode::Enabled
        } else {
            DitherMapMode::None
        };
        if effective_speed < 3 && use_dither_map != DitherMapMode::None {
            use_dither_map = DitherMapMode::Always;
        }

        let use_contrast_maps = effective_speed <= 7 || use_dither_map != DitherMapMode::None;
        let single_threaded_dithering = effective_speed == 1;

        Self {
            raw_speed,
            effective_speed,
            kmeans_iterations: iterations,
            kmeans_iteration_limit,
            feedback_loop_trials,
            max_histogram_entries,
            input_posterize_bits,
            use_dither_map,
            use_contrast_maps,
            single_threaded_dithering,
            force_disable_dither,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct QualityMetrics {
    pub internal_mse: f64,
    pub standard_mse: f64,
    pub quality_score: u8,
}

pub fn quality_to_mse(quality: u8) -> f64 {
    if quality == 0 {
        return 1e20;
    }
    if quality >= 100 {
        return 0.0;
    }
    let extra_low_quality_fudge = (0.016 / (0.001 + f64::from(quality)) - 0.001).max(0.0);
    unit_mse_to_internal_mse(
        extra_low_quality_fudge
            + 2.5 / (210.0 + f64::from(quality)).powf(1.2) * (100.1 - f64::from(quality)) / 100.0,
    )
}

pub fn mse_to_quality(mse: f64) -> u8 {
    for quality in (1..=100).rev() {
        if mse <= quality_to_mse(quality) + 0.000001 {
            return quality;
        }
    }
    0
}

pub fn evaluate_quality_against_rgba(original_rgba: &[u8], remapped_rgba: &[u8]) -> QualityMetrics {
    assert_eq!(original_rgba.len(), remapped_rgba.len());
    assert_eq!(original_rgba.len() % 4, 0);

    if original_rgba.is_empty() {
        return QualityMetrics {
            internal_mse: 0.0,
            standard_mse: 0.0,
            quality_score: 100,
        };
    }

    let gamma_lut = gamma_lut(SRGB_OUTPUT_GAMMA);
    let mut total_internal_mse = 0.0f64;
    let mut pixels = 0usize;

    for (src, dst) in original_rgba
        .chunks_exact(4)
        .zip(remapped_rgba.chunks_exact(4))
    {
        let src_px = InternalPixel::from_rgba(&gamma_lut, src);
        let dst_px = InternalPixel::from_rgba(&gamma_lut, dst);
        total_internal_mse += f64::from(src_px.diff(dst_px));
        pixels += 1;
    }

    let internal_mse = total_internal_mse / pixels as f64;
    QualityMetrics {
        internal_mse,
        standard_mse: internal_mse_to_standard_mse(internal_mse),
        quality_score: mse_to_quality(internal_mse),
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct InternalPixel {
    pub(crate) a: f32,
    pub(crate) r: f32,
    pub(crate) g: f32,
    pub(crate) b: f32,
}

impl InternalPixel {
    pub(crate) fn from_rgba(gamma_lut: &[f32; 256], rgba: &[u8]) -> Self {
        let alpha = f32::from(rgba[3]) / 255.0;
        Self {
            a: alpha * LIQ_WEIGHT_A as f32,
            r: gamma_lut[rgba[0] as usize] * LIQ_WEIGHT_R as f32 * alpha,
            g: gamma_lut[rgba[1] as usize] * LIQ_WEIGHT_G as f32 * alpha,
            b: gamma_lut[rgba[2] as usize] * LIQ_WEIGHT_B as f32 * alpha,
        }
    }

    pub(crate) fn diff(self, other: Self) -> f32 {
        let alpha_diff = other.a - self.a;
        let black_r = self.r - other.r;
        let black_g = self.g - other.g;
        let black_b = self.b - other.b;

        let white_r = black_r + alpha_diff;
        let white_g = black_g + alpha_diff;
        let white_b = black_b + alpha_diff;

        // Addition order matches reference aarch64 NEON: max_r + (max_g + max_b)
        // via vpaddq_f32 pairwise add. Floating-point addition is non-associative,
        // so matching the order is important for bit-level parity.
        let max_r = (black_r * black_r).max(white_r * white_r);
        let max_g = (black_g * black_g).max(white_g * white_g);
        let max_b = (black_b * black_b).max(white_b * white_b);
        max_r + (max_g + max_b)
    }

    pub(crate) fn to_rgba(self, gamma: f64) -> [u8; 4] {
        if self.a <= (1.0 / 255.0 * LIQ_WEIGHT_A) as f32 {
            return [0, 0, 0, 0];
        }

        let r = (LIQ_WEIGHT_A / LIQ_WEIGHT_R) as f32 * self.r / self.a;
        let g = (LIQ_WEIGHT_A / LIQ_WEIGHT_G) as f32 * self.g / self.a;
        let b = (LIQ_WEIGHT_A / LIQ_WEIGHT_B) as f32 * self.b / self.a;
        let gamma = (gamma / INTERNAL_GAMMA) as f32;

        [
            float_to_byte(r.max(0.0).powf(gamma) * 256.0),
            float_to_byte(g.max(0.0).powf(gamma) * 256.0),
            float_to_byte(b.max(0.0).powf(gamma) * 256.0),
            float_to_byte(self.a * (256.0 / LIQ_WEIGHT_A as f32)),
        ]
    }
}

pub(crate) fn gamma_lut(gamma: f64) -> [f32; 256] {
    let mut lut = [0.0; 256];
    for (idx, value) in lut.iter_mut().enumerate() {
        *value = ((idx as f32) / 255.0).powf((INTERNAL_GAMMA / gamma) as f32);
    }
    lut
}

fn float_to_byte(value: f32) -> u8 {
    value.clamp(0.0, 255.0) as u8
}

fn unit_mse_to_internal_mse(mse: f64) -> f64 {
    LIQ_WEIGHT_MSE * mse
}

fn internal_mse_to_standard_mse(mse: f64) -> f64 {
    (mse * 65536.0 / 6.0) / LIQ_WEIGHT_MSE
}

#[cfg(test)]
mod tests {
    use super::{
        DitherMapMode, SpeedSettings, evaluate_quality_against_rgba, mse_to_quality, quality_to_mse,
    };

    #[test]
    fn quality_mse_roundtrip_is_monotonic() {
        let mut last = quality_to_mse(0);
        for quality in 1..=100 {
            let mse = quality_to_mse(quality);
            assert!(mse <= last);
            assert!(mse_to_quality(mse) >= quality.saturating_sub(1));
            last = mse;
        }
    }

    #[test]
    fn quality_metrics_score_identical_image_as_100() {
        let rgba = [10u8, 20, 30, 255, 80, 90, 100, 128];
        let metrics = evaluate_quality_against_rgba(&rgba, &rgba);
        assert_eq!(metrics.quality_score, 100);
        assert_eq!(metrics.internal_mse, 0.0);
        assert_eq!(metrics.standard_mse, 0.0);
    }

    #[test]
    fn speed_11_disables_dithering_and_clamps_to_10() {
        let settings = SpeedSettings::from_speed(11);
        assert_eq!(settings.effective_speed, 10);
        assert!(settings.force_disable_dither);
        assert_eq!(settings.input_posterize_bits, 1);
        assert_eq!(settings.use_dither_map, DitherMapMode::None);
    }

    #[test]
    fn speed_4_matches_reference_shape() {
        let settings = SpeedSettings::from_speed(4);
        assert_eq!(settings.effective_speed, 4);
        assert_eq!(settings.kmeans_iterations, 12);
        assert!((settings.kmeans_iteration_limit - (1.0 / 524_288.0)).abs() < f64::EPSILON);
        assert_eq!(settings.feedback_loop_trials, 20);
        assert_eq!(settings.input_posterize_bits, 0);
        assert_eq!(settings.use_dither_map, DitherMapMode::Enabled);
        assert!(settings.use_contrast_maps);
        assert!(!settings.single_threaded_dithering);
    }
}
