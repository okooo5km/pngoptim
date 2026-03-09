use clap::{Parser, ValueEnum};
use std::path::{Path, PathBuf};

use crate::error::AppError;
use crate::quality::SpeedSettings;

/// APNG optimization mode.
#[derive(Clone, Debug, Default, PartialEq, Eq, ValueEnum)]
pub enum ApngMode {
    /// Only fold duplicate frames (safe, no visual risk)
    #[default]
    Safe,
    /// Also minimize frame rectangles (may alter dispose/blend semantics)
    Aggressive,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QualityRange {
    pub raw: String,
    pub min: u8,
    pub max: u8,
}

impl QualityRange {
    pub fn requested(&self) -> &str {
        &self.raw
    }

    pub fn effective(&self) -> String {
        format!("{}-{}", self.min, self.max)
    }
}

#[derive(Parser, Debug)]
#[command(
    name = "pngoptim",
    version,
    about = "Fast PNG quantization CLI — lossy PNG compression like pngquant"
)]
pub struct Cli {
    #[arg(value_name = "INPUT", required = true)]
    pub inputs: Vec<String>,

    #[arg(short = 'o', long = "output", value_name = "FILE")]
    pub output: Option<String>,

    #[arg(long = "ext", default_value = "-mvp.png", value_name = "SUFFIX")]
    pub ext: String,

    #[arg(
        long = "quality",
        value_parser = parse_quality_range,
        value_name = "N | -N | N- | MIN-MAX"
    )]
    pub quality: Option<QualityRange>,

    #[arg(long = "speed", default_value_t = 4, value_parser = clap::value_parser!(u8).range(1..=11))]
    pub speed: u8,

    #[arg(
        long = "floyd",
        num_args = 0..=1,
        default_missing_value = "1",
        require_equals = true,
        value_parser = parse_floyd_value,
        conflicts_with = "nofs",
        value_name = "N"
    )]
    pub floyd: Option<f32>,

    #[arg(long = "nofs", default_value_t = false)]
    pub nofs: bool,

    #[arg(long = "strip", default_value_t = false)]
    pub strip: bool,

    #[arg(long = "posterize", value_parser = clap::value_parser!(u8).range(0..=8))]
    pub posterize: Option<u8>,

    #[arg(long = "force", default_value_t = false)]
    pub force: bool,

    #[arg(long = "skip-if-larger", default_value_t = false)]
    pub skip_if_larger: bool,

    #[arg(short = 'q', long = "quiet", default_value_t = false)]
    pub quiet: bool,

    #[arg(long = "no-icc", default_value_t = false)]
    pub no_icc: bool,

    #[arg(long = "apng-mode", default_value = "safe", value_enum)]
    pub apng_mode: ApngMode,
}

impl Cli {
    pub fn validate(&self) -> Result<(), AppError> {
        if self.output.is_some() && self.inputs.len() > 1 {
            return Err(AppError::Arg(
                "--output can only be used with a single input".to_string(),
            ));
        }

        if self.inputs.len() > 1 && self.inputs.iter().any(|v| v == "-") {
            return Err(AppError::Arg(
                "stdin input ('-') cannot be mixed with multiple inputs".to_string(),
            ));
        }

        if self.output.as_deref() == Some("-") && self.inputs.len() != 1 {
            return Err(AppError::Arg(
                "stdout output ('--output -') requires a single input".to_string(),
            ));
        }

        if self.ext.contains(std::path::MAIN_SEPARATOR) {
            return Err(AppError::Arg(
                "--ext must be a filename suffix, not a path".to_string(),
            ));
        }

        for input in &self.inputs {
            if input == "-" {
                continue;
            }
            let path = Path::new(input);
            if !path.exists() {
                return Err(AppError::Arg(format!(
                    "input file does not exist: {}",
                    path.display()
                )));
            }
            if !path.is_file() {
                return Err(AppError::Arg(format!(
                    "input is not a file: {}",
                    path.display()
                )));
            }
        }

        Ok(())
    }

    pub fn dither_level(&self) -> f32 {
        if SpeedSettings::from_speed(self.speed).force_disable_dither {
            return 0.0;
        }
        if self.nofs {
            return 0.0;
        }
        self.floyd.unwrap_or(1.0)
    }

    pub fn effective_speed(&self) -> u8 {
        SpeedSettings::from_speed(self.speed).effective_speed
    }

    pub fn output_for_input(&self, input: &str) -> Result<OutputTarget, AppError> {
        if let Some(output) = &self.output {
            if output == "-" {
                return Ok(OutputTarget::Stdout);
            }
            return Ok(OutputTarget::File(PathBuf::from(output)));
        }

        if input == "-" {
            return Ok(OutputTarget::Stdout);
        }

        Ok(OutputTarget::File(default_output_path(
            Path::new(input),
            &self.ext,
        )))
    }
}

#[derive(Debug, Clone)]
pub enum OutputTarget {
    File(PathBuf),
    Stdout,
}

fn default_output_path(input: &Path, ext: &str) -> PathBuf {
    let parent = input.parent().unwrap_or_else(|| Path::new("."));
    let stem = input
        .file_stem()
        .and_then(|v| v.to_str())
        .unwrap_or("output");
    parent.join(format!("{stem}{ext}"))
}

pub fn parse_quality_range(raw: &str) -> Result<QualityRange, String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err("quality must not be empty".to_string());
    }

    let (min, max) = if let Some(target_raw) = raw.strip_prefix('-') {
        (0, parse_quality_value(target_raw, "quality target")?)
    } else if let Some(min_raw) = raw.strip_suffix('-') {
        (parse_quality_value(min_raw, "quality minimum")?, 100)
    } else if let Some((min_raw, max_raw)) = raw.split_once('-') {
        let min = parse_quality_value(min_raw, "quality minimum")?;
        let max = parse_quality_value(max_raw, "quality target")?;
        if min > max {
            return Err("quality minimum must be <= quality target".to_string());
        }
        (min, max)
    } else {
        let target = parse_quality_value(raw, "quality target")?;
        (((u16::from(target) * 9) / 10) as u8, target)
    };

    Ok(QualityRange {
        raw: raw.to_string(),
        min,
        max,
    })
}

fn parse_quality_value(raw: &str, label: &str) -> Result<u8, String> {
    raw.parse::<u8>()
        .map_err(|_| format!("{label} must be 0..100"))
        .and_then(|value| {
            if value <= 100 {
                Ok(value)
            } else {
                Err(format!("{label} must be 0..100"))
            }
        })
}

fn parse_floyd_value(raw: &str) -> Result<f32, String> {
    let value = raw
        .parse::<f32>()
        .map_err(|_| "--floyd argument must be in 0..1 range".to_string())?;
    if (0.0..=1.0).contains(&value) {
        Ok(value)
    } else {
        Err("--floyd argument must be in 0..1 range".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::{OutputTarget, parse_quality_range};
    use clap::Parser;

    use crate::cli::Cli;

    #[test]
    fn parse_quality_ok() {
        let range = parse_quality_range("55-80").expect("parse quality");
        assert_eq!(range.min, 55);
        assert_eq!(range.max, 80);
        assert_eq!(range.requested(), "55-80");
        assert_eq!(range.effective(), "55-80");
    }

    #[test]
    fn parse_quality_single_value_uses_pngquant_defaults() {
        let range = parse_quality_range("70").expect("parse quality");
        assert_eq!(range.min, 63);
        assert_eq!(range.max, 70);
        assert_eq!(range.requested(), "70");
        assert_eq!(range.effective(), "63-70");
    }

    #[test]
    fn parse_quality_supports_open_ranges() {
        let upper_only = parse_quality_range("-80").expect("parse quality");
        assert_eq!(upper_only.min, 0);
        assert_eq!(upper_only.max, 80);

        let lower_only = parse_quality_range("65-").expect("parse quality");
        assert_eq!(lower_only.min, 65);
        assert_eq!(lower_only.max, 100);
    }

    #[test]
    fn parse_quality_invalid() {
        assert!(parse_quality_range("80-55").is_err());
        assert!(parse_quality_range("hello").is_err());
        assert!(parse_quality_range("30-300").is_err());
    }

    #[test]
    fn output_resolution_stdout() {
        let cli = Cli::parse_from(["pngoptim", "-", "--output", "-"]);
        assert!(matches!(
            cli.output_for_input("-").expect("resolve output"),
            OutputTarget::Stdout
        ));
    }

    #[test]
    fn floyd_defaults_to_one_when_present_without_value() {
        let cli = Cli::parse_from(["pngoptim", "in.png", "--floyd"]);
        assert_eq!(cli.floyd, Some(1.0));
        assert_eq!(cli.dither_level(), 1.0);
    }

    #[test]
    fn floyd_accepts_fractional_strength() {
        let cli = Cli::parse_from(["pngoptim", "in.png", "--floyd=0.5"]);
        assert_eq!(cli.floyd, Some(0.5));
        assert_eq!(cli.dither_level(), 0.5);
    }
}
