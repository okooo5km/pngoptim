mod cli;
mod error;
mod palette_quant;
mod pipeline;

use clap::Parser;
use cli::{Cli, OutputTarget, QualityRange};
use error::AppError;
use pipeline::{
    PipelineOptions, PipelineResult, process_png_bytes, process_png_file, write_output_file,
};
use std::io::{Read, Write};
use std::path::Path;

#[derive(Debug, Clone, Copy)]
struct RunSummary {
    processed: usize,
    succeeded: usize,
    failed: usize,
}

fn run(cli: Cli) -> Result<(), AppError> {
    cli.validate()?;
    let profile_metrics = std::env::var_os("PNGOPTIM_PROFILE_METRICS").is_some();

    let mut summary = RunSummary {
        processed: 0,
        succeeded: 0,
        failed: 0,
    };
    let mut exit_code = 0;

    for input in &cli.inputs {
        summary.processed += 1;

        let output_target = cli.output_for_input(input)?;
        let options = PipelineOptions {
            quality: cli.quality,
            speed: cli.speed,
            dither: cli.dither_enabled(),
            posterize: cli.posterize,
            strip: cli.strip,
            skip_if_larger: cli.skip_if_larger,
        };

        let result = if input == "-" {
            let mut bytes = Vec::new();
            std::io::stdin()
                .read_to_end(&mut bytes)
                .map_err(|e| AppError::Io {
                    path: None,
                    source: e,
                })?;
            process_png_bytes(&bytes, options)
        } else {
            process_png_file(std::path::Path::new(input), options)
        };

        match (result, output_target) {
            (Ok(result), OutputTarget::File(path)) => {
                if let Err(err) = write_output_file(&path, &result.png_data, cli.force) {
                    summary.failed += 1;
                    if exit_code == 0 {
                        exit_code = err.exit_code();
                    }
                    if !cli.quiet {
                        eprintln!("error: {input} -> {}: {err}", path.display());
                    }
                    continue;
                }

                summary.succeeded += 1;
                if profile_metrics {
                    eprintln!(
                        "profile_metrics\tinput={}\toutput={}\tdecode_ms={:.3}\tquantize_ms={:.3}\tencode_ms={:.3}\ttotal_ms={:.3}",
                        input,
                        path.display(),
                        result.metrics.decode_ms,
                        result.metrics.quantize_ms,
                        result.metrics.encode_ms,
                        result.metrics.total_ms
                    );
                }
                if !cli.quiet {
                    println!("{}", format_success_message(&result, &path, cli.quality));
                }
            }
            (Ok(result), OutputTarget::Stdout) => {
                std::io::stdout()
                    .write_all(&result.png_data)
                    .map_err(|e| AppError::Io {
                        path: None,
                        source: e,
                    })?;
                summary.succeeded += 1;
                if profile_metrics {
                    eprintln!(
                        "profile_metrics\tinput={}\toutput=-\tdecode_ms={:.3}\tquantize_ms={:.3}\tencode_ms={:.3}\ttotal_ms={:.3}",
                        input,
                        result.metrics.decode_ms,
                        result.metrics.quantize_ms,
                        result.metrics.encode_ms,
                        result.metrics.total_ms
                    );
                }
            }
            (Err(err), _) => {
                summary.failed += 1;
                if exit_code == 0 {
                    exit_code = err.exit_code();
                }
                if !cli.quiet {
                    eprintln!("error: {input}: {err}");
                }
            }
        }
    }

    if !cli.quiet && summary.processed > 1 {
        println!(
            "summary: processed={}, success={}, failed={}",
            summary.processed, summary.succeeded, summary.failed
        );
    }

    if exit_code != 0 {
        std::process::exit(exit_code);
    }

    Ok(())
}

fn format_success_message(
    result: &PipelineResult,
    path: &Path,
    requested_quality: Option<QualityRange>,
) -> String {
    let quality_part = match requested_quality {
        Some(range) => format!(
            "requested_quality={}-{}, quality_score={}",
            range.min, range.max, result.quality_score
        ),
        None => format!("quality_score={}", result.quality_score),
    };

    format!(
        "ok: {}x{}, {}, {} -> {} bytes, wrote {}",
        result.width,
        result.height,
        quality_part,
        result.input_bytes,
        result.output_bytes,
        path.display()
    )
}

fn main() {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(err) => {
            let exit = if err.use_stderr() { 2 } else { 0 };
            let _ = err.print();
            std::process::exit(exit);
        }
    };

    if let Err(err) = run(cli) {
        eprintln!("error: {err}");
        std::process::exit(err.exit_code());
    }
}

#[cfg(test)]
mod tests {
    use super::format_success_message;
    use crate::cli::QualityRange;
    use crate::pipeline::{PipelineMetrics, PipelineResult};
    use std::path::Path;

    fn sample_result() -> PipelineResult {
        PipelineResult {
            width: 10,
            height: 20,
            input_bytes: 1000,
            output_bytes: 400,
            quality_score: 99,
            png_data: Vec::new(),
            metrics: PipelineMetrics {
                decode_ms: 0.0,
                quantize_ms: 0.0,
                encode_ms: 0.0,
                total_ms: 0.0,
            },
        }
    }

    #[test]
    fn success_message_includes_requested_quality_range() {
        let msg = format_success_message(
            &sample_result(),
            Path::new("/tmp/out.png"),
            Some(QualityRange { min: 65, max: 75 }),
        );
        assert!(msg.contains("requested_quality=65-75"));
        assert!(msg.contains("quality_score=99"));
    }

    #[test]
    fn success_message_uses_quality_score_without_request_range() {
        let msg = format_success_message(&sample_result(), Path::new("/tmp/out.png"), None);
        assert!(msg.contains("quality_score=99"));
        assert!(!msg.contains("requested_quality="));
    }
}
