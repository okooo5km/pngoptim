use clap::Parser;
use std::path::{Path, PathBuf};

use crate::error::AppError;

#[derive(Debug, Clone, Copy)]
pub struct QualityRange {
    pub min: u8,
    pub max: u8,
}

impl QualityRange {
    pub fn target(self) -> u8 {
        ((u16::from(self.min) + u16::from(self.max)) / 2) as u8
    }
}

#[derive(Parser, Debug)]
#[command(
    name = "pngoptim",
    version,
    about = "PNG quantization CLI (Phase C compatibility work)"
)]
pub struct Cli {
    #[arg(value_name = "INPUT", required = true)]
    pub inputs: Vec<String>,

    #[arg(short = 'o', long = "output", value_name = "FILE")]
    pub output: Option<String>,

    #[arg(long = "ext", default_value = "-mvp.png", value_name = "SUFFIX")]
    pub ext: String,

    #[arg(long = "quality", value_parser = parse_quality_range)]
    pub quality: Option<QualityRange>,

    #[arg(long = "speed", default_value_t = 4, value_parser = clap::value_parser!(u8).range(1..=11))]
    pub speed: u8,

    #[arg(long = "floyd", default_value_t = false, conflicts_with = "nofs")]
    pub floyd: bool,

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

    pub fn dither_enabled(&self) -> bool {
        !self.nofs || self.floyd
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
    let (min_raw, max_raw) = raw
        .split_once('-')
        .ok_or_else(|| "quality must be in min-max format, e.g. 60-85".to_string())?;

    let min = min_raw
        .parse::<u8>()
        .map_err(|_| "quality min must be 0..100".to_string())?;
    let max = max_raw
        .parse::<u8>()
        .map_err(|_| "quality max must be 0..100".to_string())?;

    if min > max {
        return Err("quality min must be <= max".to_string());
    }
    if max > 100 {
        return Err("quality max must be 0..100".to_string());
    }

    Ok(QualityRange { min, max })
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
}
