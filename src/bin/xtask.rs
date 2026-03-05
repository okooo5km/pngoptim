use clap::{Args, Parser, Subcommand};
use csv::Writer;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeSet, HashMap};
use std::error::Error;
use std::ffi::OsString;
use std::fs;
use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Instant;

const COMPARE_SPLITS: [&str; 3] = ["functional", "quality", "perf"];
const ALL_SPLITS: [&str; 4] = ["functional", "quality", "perf", "robustness"];
const PNG_SIG: &[u8] = b"\x89PNG\r\n\x1a\n";

type AppResult<T> = Result<T, Box<dyn Error>>;

#[derive(Parser)]
#[command(name = "xtask")]
#[command(about = "Rust-native engineering orchestration commands")]
struct XtaskCli {
    #[command(subcommand)]
    command: XtaskCommand,
}

#[derive(Subcommand)]
enum XtaskCommand {
    #[command(name = "cross-platform")]
    CrossPlatform(CrossPlatformCli),
}

#[derive(Parser)]
struct CrossPlatformCli {
    #[command(subcommand)]
    command: CrossPlatformCommand,
}

#[derive(Subcommand)]
enum CrossPlatformCommand {
    Collect(CollectArgs),
    Aggregate(AggregateArgs),
}

#[derive(Args)]
struct CollectArgs {
    #[arg(long)]
    run_id: Option<String>,
    #[arg(long)]
    platform_label: Option<String>,
    #[arg(long, default_value = "target/release/pngoptim")]
    binary: String,
    #[arg(long, default_value_t = false)]
    build: bool,
    #[arg(long, default_value_t = 12.0)]
    timeout_sec: f64,
}

#[derive(Args)]
struct AggregateArgs {
    #[arg(long)]
    run_id: Option<String>,
    #[arg(long, default_value_t = false)]
    allow_partial: bool,
}

#[derive(Debug, Deserialize)]
struct ManifestEntry {
    id: Option<String>,
    filename: String,
    expected_success: Option<bool>,
}

#[derive(Debug, Clone)]
struct Sample {
    split: String,
    sample_id: String,
    filename: String,
    expected_success: bool,
}

#[derive(Debug)]
struct CmdOutput {
    code: Option<i32>,
    stdout: Vec<u8>,
    stderr: String,
}

#[derive(Debug, Serialize)]
struct CollectRow {
    sample_id: String,
    split: String,
    input_file: String,
    exit_code: i32,
    elapsed_ms: f64,
    input_bytes: u64,
    output_bytes: Option<u64>,
    size_ratio: Option<f64>,
    output_sha256: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct PlatformSampleMetric {
    output_bytes: Option<u64>,
    size_ratio: Option<f64>,
    output_sha256: String,
    exit_code: i32,
}

#[derive(Debug, Serialize, Deserialize)]
struct PlatformMetrics {
    run_id: String,
    platform_label: String,
    system: String,
    release: String,
    machine: String,
    #[serde(default)]
    rust_version: String,
    sample_count: usize,
    success_count: usize,
    failure_count: usize,
    size_ratio_mean: f64,
    size_ratio_median: f64,
    size_ratio_p95: f64,
    elapsed_ms_mean: f64,
    elapsed_ms_median: f64,
    elapsed_ms_p95: f64,
    smoke_passed: bool,
    compat_exit_passed: bool,
    compat_io_passed: bool,
    stability_crash_like_count: i32,
    stability_failures_count: i32,
    scripts: HashMap<String, String>,
    samples: HashMap<String, PlatformSampleMetric>,
    collect_failures: Vec<FailureItem>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct FailureItem {
    stage: String,
    detail: String,
    exit_code: Option<i32>,
}

#[derive(Debug, Serialize)]
struct ConsistencyRow {
    metric: String,
    min: f64,
    max: f64,
    spread: f64,
    threshold: f64,
    passed: bool,
}

fn main() {
    let cli = XtaskCli::parse();
    let code = match run(cli) {
        Ok(code) => code,
        Err(err) => {
            eprintln!("error: {err}");
            1
        }
    };
    std::process::exit(code);
}

fn run(cli: XtaskCli) -> AppResult<i32> {
    match cli.command {
        XtaskCommand::CrossPlatform(cp) => match cp.command {
            CrossPlatformCommand::Collect(args) => collect_cross_platform(args),
            CrossPlatformCommand::Aggregate(args) => aggregate_cross_platform(args),
        },
    }
}

fn collect_cross_platform(args: CollectArgs) -> AppResult<i32> {
    let root = std::env::current_dir()?;
    let run_id = args
        .run_id
        .unwrap_or_else(|| "cross-platform-v1-local".to_string());
    let platform_label = args.platform_label.unwrap_or_else(default_platform_label);
    let reports_dir = root.join("reports").join("cross_platform").join(&run_id);
    let platform_dir = reports_dir.join("platform");
    let out_dir = reports_dir.join("out").join(&platform_label);

    fs::create_dir_all(&platform_dir)?;
    fs::create_dir_all(&out_dir)?;

    if args.build {
        let status = Command::new("cargo")
            .current_dir(&root)
            .arg("build")
            .arg("--release")
            .status()?;
        if !status.success() {
            return Ok(status.code().unwrap_or(1));
        }
    }

    let binary = resolve_binary_path(&root, &args.binary);
    if !binary.exists() {
        eprintln!("binary not found: {}", binary.display());
        return Ok(2);
    }

    let mut failures: Vec<FailureItem> = Vec::new();
    let samples = load_samples(&root, &COMPARE_SPLITS)?
        .into_iter()
        .filter(|s| s.expected_success)
        .collect::<Vec<_>>();

    let mut rows: Vec<CollectRow> = Vec::new();
    for sample in &samples {
        let input_path = root
            .join("dataset")
            .join(&sample.split)
            .join(&sample.filename);
        let stem = Path::new(&sample.filename)
            .file_stem()
            .and_then(|v| v.to_str())
            .unwrap_or("sample");
        let output_path = out_dir.join(&sample.split).join(format!("{stem}.cp.png"));
        if let Some(parent) = output_path.parent() {
            fs::create_dir_all(parent)?;
        }
        if output_path.exists() {
            let _ = fs::remove_file(&output_path);
        }

        let cmd_args = vec![
            input_path.to_string_lossy().to_string(),
            "--quality".to_string(),
            "55-75".to_string(),
            "--speed".to_string(),
            "4".to_string(),
            "--strip".to_string(),
            "--force".to_string(),
            "--quiet".to_string(),
            "--output".to_string(),
            output_path.to_string_lossy().to_string(),
        ];
        let start = Instant::now();
        let output = run_command(&root, &binary, &cmd_args, None)?;
        let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;

        let input_bytes = fs::metadata(&input_path)?.len();
        let success = output.code == Some(0) && output_path.exists();
        let output_bytes = if success {
            Some(fs::metadata(&output_path)?.len())
        } else {
            None
        };
        let size_ratio = output_bytes.map(|v| v as f64 / input_bytes as f64);
        let output_sha256 = if success {
            sha256_file(&output_path)?
        } else {
            String::new()
        };

        if !success {
            failures.push(FailureItem {
                stage: "candidate_eval".to_string(),
                detail: format!(
                    "sample_id={} exit_code={} stderr={}",
                    sample.sample_id,
                    output.code.unwrap_or(-1),
                    truncate(&output.stderr, 500)
                ),
                exit_code: output.code,
            });
        }

        rows.push(CollectRow {
            sample_id: sample.sample_id.clone(),
            split: sample.split.clone(),
            input_file: sample.filename.clone(),
            exit_code: output.code.unwrap_or(-1),
            elapsed_ms,
            input_bytes,
            output_bytes,
            size_ratio,
            output_sha256,
        });
    }

    let smoke_ok = run_smoke_check(
        &root,
        &binary,
        &reports_dir,
        &platform_label,
        args.timeout_sec,
    )?;
    if !smoke_ok {
        failures.push(FailureItem {
            stage: "smoke".to_string(),
            detail: "smoke checks failed".to_string(),
            exit_code: Some(1),
        });
    }

    let (compat_exit_ok, compat_io_ok) =
        run_compat_check(&root, &binary, &reports_dir, &platform_label)?;
    if !compat_exit_ok {
        failures.push(FailureItem {
            stage: "compat_exit".to_string(),
            detail: "compat exit-code checks failed".to_string(),
            exit_code: Some(1),
        });
    }
    if !compat_io_ok {
        failures.push(FailureItem {
            stage: "compat_io".to_string(),
            detail: "compat io checks failed".to_string(),
            exit_code: Some(1),
        });
    }

    let (stability_crash_like_count, stability_failures_count) =
        run_stability_check(&root, &binary, &reports_dir, &platform_label)?;
    if stability_failures_count > 0 {
        failures.push(FailureItem {
            stage: "stability".to_string(),
            detail: format!(
                "stability failures={} crash_like={}",
                stability_failures_count, stability_crash_like_count
            ),
            exit_code: Some(1),
        });
    }

    let success_rows = rows
        .iter()
        .filter(|r| r.output_bytes.is_some())
        .collect::<Vec<_>>();
    let size_ratios = success_rows
        .iter()
        .filter_map(|r| r.size_ratio)
        .collect::<Vec<_>>();
    let elapsed_vals = success_rows
        .iter()
        .map(|r| r.elapsed_ms)
        .collect::<Vec<_>>();

    let mut samples_map = HashMap::new();
    for row in &rows {
        samples_map.insert(
            row.sample_id.clone(),
            PlatformSampleMetric {
                output_bytes: row.output_bytes,
                size_ratio: row.size_ratio,
                output_sha256: row.output_sha256.clone(),
                exit_code: row.exit_code,
            },
        );
    }

    let rust_version = rustc_version().unwrap_or_else(|| "unknown".to_string());
    let platform_metrics = PlatformMetrics {
        run_id: run_id.clone(),
        platform_label: platform_label.clone(),
        system: std::env::consts::OS.to_string(),
        release: "unknown".to_string(),
        machine: std::env::consts::ARCH.to_string(),
        rust_version,
        sample_count: rows.len(),
        success_count: success_rows.len(),
        failure_count: rows.len().saturating_sub(success_rows.len()),
        size_ratio_mean: mean(&size_ratios),
        size_ratio_median: median(&size_ratios),
        size_ratio_p95: p95(&size_ratios),
        elapsed_ms_mean: mean(&elapsed_vals),
        elapsed_ms_median: median(&elapsed_vals),
        elapsed_ms_p95: p95(&elapsed_vals),
        smoke_passed: smoke_ok,
        compat_exit_passed: compat_exit_ok,
        compat_io_passed: compat_io_ok,
        stability_crash_like_count,
        stability_failures_count,
        scripts: {
            let mut m = HashMap::new();
            m.insert(
                "smoke_run_id".to_string(),
                format!("smoke-{run_id}-{platform_label}"),
            );
            m.insert(
                "compat_run_id".to_string(),
                format!("compat-{run_id}-{platform_label}"),
            );
            m.insert(
                "stability_run_id".to_string(),
                format!("stability-{run_id}-{platform_label}"),
            );
            m
        },
        samples: samples_map,
        collect_failures: failures.clone(),
    };

    let platform_json = serde_json::to_string_pretty(&platform_metrics)?;
    fs::write(
        platform_dir.join(format!("{platform_label}.json")),
        format!("{platform_json}\n"),
    )?;

    let mut writer = Writer::from_path(reports_dir.join(format!("collect_{platform_label}.csv")))?;
    writer.write_record([
        "sample_id",
        "split",
        "input_file",
        "exit_code",
        "elapsed_ms",
        "input_bytes",
        "output_bytes",
        "size_ratio",
        "output_sha256",
    ])?;
    for row in &rows {
        writer.write_record([
            row.sample_id.as_str(),
            row.split.as_str(),
            row.input_file.as_str(),
            &row.exit_code.to_string(),
            &format!("{:.3}", row.elapsed_ms),
            &row.input_bytes.to_string(),
            &row.output_bytes.map(|v| v.to_string()).unwrap_or_default(),
            &row.size_ratio
                .map(|v| format!("{v:.9}"))
                .unwrap_or_default(),
            row.output_sha256.as_str(),
        ])?;
    }
    writer.flush()?;

    println!(
        "Cross-platform collect complete: {}",
        platform_dir
            .join(format!("{platform_label}.json"))
            .display()
    );
    Ok(if failures.is_empty() { 0 } else { 1 })
}

fn aggregate_cross_platform(args: AggregateArgs) -> AppResult<i32> {
    let root = std::env::current_dir()?;
    let run_id = args
        .run_id
        .unwrap_or_else(|| "cross-platform-v1-local".to_string());
    let run_dir = root.join("reports").join("cross_platform").join(&run_id);
    let platform_dir = run_dir.join("platform");

    if !platform_dir.exists() {
        eprintln!("platform directory not found: {}", platform_dir.display());
        return Ok(2);
    }

    let mut files = fs::read_dir(&platform_dir)?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|v| v.to_str()) == Some("json"))
        .collect::<Vec<_>>();
    files.sort();

    if files.is_empty() {
        eprintln!("no platform json found for aggregation");
        return Ok(2);
    }

    let mut data: Vec<PlatformMetrics> = Vec::new();
    for path in files {
        let raw = fs::read_to_string(path)?;
        data.push(serde_json::from_str(&raw)?);
    }

    let labels = data
        .iter()
        .map(|d| d.platform_label.clone())
        .collect::<Vec<_>>();
    let platform_count = data.len() as f64;

    let mut checks: Vec<ConsistencyRow> = Vec::new();
    checks.push(ConsistencyRow {
        metric: "platform_count".to_string(),
        min: platform_count,
        max: platform_count,
        spread: 0.0,
        threshold: 3.0,
        passed: data.len() >= 3 || args.allow_partial,
    });

    for (metric, threshold) in [
        ("size_ratio_mean", 1e-6_f64),
        ("size_ratio_median", 1e-6_f64),
        ("size_ratio_p95", 1e-6_f64),
    ] {
        let vals = data
            .iter()
            .map(|d| match metric {
                "size_ratio_mean" => d.size_ratio_mean,
                "size_ratio_median" => d.size_ratio_median,
                _ => d.size_ratio_p95,
            })
            .collect::<Vec<_>>();
        let (min_v, max_v, spread_v) = spread(&vals);
        checks.push(ConsistencyRow {
            metric: metric.to_string(),
            min: min_v,
            max: max_v,
            spread: spread_v,
            threshold,
            passed: spread_v <= threshold,
        });
    }

    let smoke_ok = data.iter().all(|d| d.smoke_passed);
    let compat_ok = data
        .iter()
        .all(|d| d.compat_exit_passed && d.compat_io_passed);
    let stability_ok = data
        .iter()
        .all(|d| d.stability_crash_like_count == 0 && d.stability_failures_count == 0);

    checks.push(ConsistencyRow {
        metric: "smoke_passed_all_platforms".to_string(),
        min: if smoke_ok { 1.0 } else { 0.0 },
        max: if smoke_ok { 1.0 } else { 0.0 },
        spread: 0.0,
        threshold: 1.0,
        passed: smoke_ok,
    });
    checks.push(ConsistencyRow {
        metric: "compat_passed_all_platforms".to_string(),
        min: if compat_ok { 1.0 } else { 0.0 },
        max: if compat_ok { 1.0 } else { 0.0 },
        spread: 0.0,
        threshold: 1.0,
        passed: compat_ok,
    });
    checks.push(ConsistencyRow {
        metric: "stability_passed_all_platforms".to_string(),
        min: if stability_ok { 1.0 } else { 0.0 },
        max: if stability_ok { 1.0 } else { 0.0 },
        spread: 0.0,
        threshold: 1.0,
        passed: stability_ok,
    });

    let mut sample_ids = BTreeSet::new();
    for d in &data {
        for key in d.samples.keys() {
            sample_ids.insert(key.clone());
        }
    }

    let mut inconsistent_samples = Vec::<serde_json::Value>::new();
    for sample_id in sample_ids {
        let mut vals: Vec<u64> = Vec::new();
        for d in &data {
            match d.samples.get(&sample_id).and_then(|s| s.output_bytes) {
                Some(v) => vals.push(v),
                None => {
                    inconsistent_samples.push(serde_json::json!({
                        "sample_id": sample_id,
                        "reason": "missing_output"
                    }));
                    vals.clear();
                    break;
                }
            }
        }
        if !vals.is_empty() {
            let first = vals[0];
            if vals.iter().any(|v| *v != first) {
                inconsistent_samples.push(serde_json::json!({
                    "sample_id": sample_id,
                    "reason": "bytes_mismatch",
                    "values": vals
                }));
            }
        }
    }

    checks.push(ConsistencyRow {
        metric: "sample_output_bytes_consistent".to_string(),
        min: inconsistent_samples.len() as f64,
        max: inconsistent_samples.len() as f64,
        spread: 0.0,
        threshold: 0.0,
        passed: inconsistent_samples.is_empty(),
    });

    fs::create_dir_all(&run_dir)?;
    let mut writer = Writer::from_path(run_dir.join("consistency.csv"))?;
    writer.write_record(["metric", "min", "max", "spread", "threshold", "passed"])?;
    for row in &checks {
        writer.write_record([
            row.metric.as_str(),
            &row.min.to_string(),
            &row.max.to_string(),
            &row.spread.to_string(),
            &row.threshold.to_string(),
            if row.passed { "true" } else { "false" },
        ])?;
    }
    writer.flush()?;

    fs::write(
        run_dir.join("inconsistent_samples.json"),
        format!("{}\n", serde_json::to_string_pretty(&inconsistent_samples)?),
    )?;

    let failed_checks = checks.iter().filter(|c| !c.passed).collect::<Vec<_>>();
    let passed = failed_checks.is_empty();

    let mut summary = vec![
        "# Cross-platform Report v1".to_string(),
        String::new(),
        format!("- run_id: `{run_id}`"),
        format!("- platforms: {}", data.len()),
        format!("- platform_labels: `{}`", labels.join(", ")),
        format!("- allow_partial: {}", args.allow_partial),
        format!("- inconsistent_samples: {}", inconsistent_samples.len()),
        format!("- status: {}", if passed { "pass" } else { "fail" }),
        String::new(),
        "Artifacts:".to_string(),
        format!("- `reports/cross_platform/{run_id}/consistency.csv`"),
        format!("- `reports/cross_platform/{run_id}/inconsistent_samples.json`"),
    ];

    if !failed_checks.is_empty() {
        summary.push(String::new());
        summary.push("Failed Checks:".to_string());
        for c in &failed_checks {
            summary.push(format!(
                "- {}: min={}, max={}, spread={}, threshold={}",
                c.metric, c.min, c.max, c.spread, c.threshold
            ));
        }
    }
    fs::write(
        run_dir.join("summary.md"),
        format!("{}\n", summary.join("\n")),
    )?;

    println!(
        "Cross-platform aggregate complete: {}",
        run_dir.join("summary.md").display()
    );
    for c in &failed_checks {
        eprintln!(
            "FAILED_CHECK\t{}\tmin={}\tmax={}\tspread={}\tthreshold={}",
            c.metric, c.min, c.max, c.spread, c.threshold
        );
    }

    Ok(if passed { 0 } else { 1 })
}

fn run_smoke_check(
    root: &Path,
    binary: &Path,
    reports_dir: &Path,
    platform_label: &str,
    _timeout_sec: f64,
) -> AppResult<bool> {
    let samples = load_samples(root, &ALL_SPLITS)?;
    let mut all_passed = true;

    for sample in samples {
        let input_path = root
            .join("dataset")
            .join(&sample.split)
            .join(&sample.filename);
        let stem = Path::new(&sample.filename)
            .file_stem()
            .and_then(|v| v.to_str())
            .unwrap_or("sample");
        let output_path = reports_dir
            .join("out")
            .join(platform_label)
            .join("smoke")
            .join(&sample.split)
            .join(format!("{stem}.smoke.png"));
        if let Some(parent) = output_path.parent() {
            fs::create_dir_all(parent)?;
        }
        if output_path.exists() {
            let _ = fs::remove_file(&output_path);
        }

        let cmd_args = vec![
            input_path.to_string_lossy().to_string(),
            "--output".to_string(),
            output_path.to_string_lossy().to_string(),
            "--force".to_string(),
            "--quality".to_string(),
            "60-85".to_string(),
            "--speed".to_string(),
            "4".to_string(),
        ];

        let output = run_command(root, binary, &cmd_args, None)?;
        let success = output.code == Some(0) && output_path.exists();
        let passed = if sample.expected_success {
            success
        } else {
            output.code != Some(0)
        };

        if !passed {
            all_passed = false;
        }
    }

    Ok(all_passed)
}

fn run_compat_check(
    root: &Path,
    binary: &Path,
    reports_dir: &Path,
    platform_label: &str,
) -> AppResult<(bool, bool)> {
    let tmp_dir = reports_dir
        .join("tmp")
        .join(format!("compat-{platform_label}"));
    fs::create_dir_all(&tmp_dir)?;

    let sample_func = root
        .join("dataset")
        .join("functional")
        .join("pngquant_test.png");
    let sample_meta = root
        .join("dataset")
        .join("functional")
        .join("pngquant_metadata.png");

    let tiny_skip_case = tmp_dir.join("tiny_skip_case.png");
    write_tiny_png(&tiny_skip_case)?;

    let io_parent_file = tmp_dir.join("not_a_directory");
    fs::write(&io_parent_file, b"compat-io-failure-sentinel\n")?;
    let io_failure_output = io_parent_file.join("child.png");

    let success_res = run_command(
        root,
        binary,
        &vec![
            sample_func.to_string_lossy().to_string(),
            "--output".to_string(),
            tmp_dir
                .join("exit_success.png")
                .to_string_lossy()
                .to_string(),
            "--force".to_string(),
        ],
        None,
    )?;
    let param_error_res = run_command(root, binary, &vec!["no-such-input.png".to_string()], None)?;
    let quality_low_res = run_command(
        root,
        binary,
        &vec![
            sample_func.to_string_lossy().to_string(),
            "--output".to_string(),
            tmp_dir
                .join("exit_quality.png")
                .to_string_lossy()
                .to_string(),
            "--quality".to_string(),
            "99-100".to_string(),
            "--posterize".to_string(),
            "8".to_string(),
            "--force".to_string(),
        ],
        None,
    )?;
    let size_not_reduced_res = run_command(
        root,
        binary,
        &vec![
            tiny_skip_case.to_string_lossy().to_string(),
            "--output".to_string(),
            tmp_dir.join("exit_size.png").to_string_lossy().to_string(),
            "--skip-if-larger".to_string(),
            "--force".to_string(),
        ],
        None,
    )?;
    let io_failure_res = run_command(
        root,
        binary,
        &vec![
            sample_func.to_string_lossy().to_string(),
            "--output".to_string(),
            io_failure_output.to_string_lossy().to_string(),
            "--force".to_string(),
        ],
        None,
    )?;

    let compat_exit_ok = success_res.code == Some(0)
        && param_error_res.code == Some(2)
        && quality_low_res.code == Some(98)
        && size_not_reduced_res.code == Some(99)
        && io_failure_res.code == Some(3);

    let io_file_output = tmp_dir.join("io_file.png");
    let io_file_res = run_command(
        root,
        binary,
        &vec![
            sample_func.to_string_lossy().to_string(),
            "--output".to_string(),
            io_file_output.to_string_lossy().to_string(),
            "--force".to_string(),
        ],
        None,
    )?;
    let file_io_ok = io_file_res.code == Some(0) && io_file_output.exists();

    let stdin_bytes = fs::read(&sample_func)?;
    let stdio_res = run_command(
        root,
        binary,
        &vec!["-".to_string(), "--output".to_string(), "-".to_string()],
        Some(&stdin_bytes),
    )?;
    let stdio_ok = stdio_res.code == Some(0) && stdio_res.stdout.starts_with(PNG_SIG);

    let batch_ext = ".batch.png";
    let batch_res = run_command(
        root,
        binary,
        &vec![
            sample_func.to_string_lossy().to_string(),
            sample_meta.to_string_lossy().to_string(),
            format!("--ext={batch_ext}"),
            "--force".to_string(),
            "--quiet".to_string(),
        ],
        None,
    )?;
    let batch_a = sample_func.with_file_name(format!(
        "{}{}",
        sample_func
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("pngquant_test"),
        batch_ext
    ));
    let batch_b = sample_meta.with_file_name(format!(
        "{}{}",
        sample_meta
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("pngquant_metadata"),
        batch_ext
    ));
    let batch_ok = batch_res.code == Some(0) && batch_a.exists() && batch_b.exists();
    if batch_a.exists() {
        let _ = fs::remove_file(batch_a);
    }
    if batch_b.exists() {
        let _ = fs::remove_file(batch_b);
    }

    let overwrite_path = tmp_dir.join("io_overwrite.png");
    let _ = run_command(
        root,
        binary,
        &vec![
            sample_func.to_string_lossy().to_string(),
            "--output".to_string(),
            overwrite_path.to_string_lossy().to_string(),
            "--force".to_string(),
        ],
        None,
    )?;
    let overwrite_res = run_command(
        root,
        binary,
        &vec![
            sample_func.to_string_lossy().to_string(),
            "--output".to_string(),
            overwrite_path.to_string_lossy().to_string(),
        ],
        None,
    )?;
    let overwrite_ok = overwrite_res.code == Some(2);

    let meta_preserve = tmp_dir.join("meta_preserve.png");
    let meta_strip = tmp_dir.join("meta_strip.png");
    let meta_keep_res = run_command(
        root,
        binary,
        &vec![
            sample_meta.to_string_lossy().to_string(),
            "--output".to_string(),
            meta_preserve.to_string_lossy().to_string(),
            "--force".to_string(),
        ],
        None,
    )?;
    let meta_strip_res = run_command(
        root,
        binary,
        &vec![
            sample_meta.to_string_lossy().to_string(),
            "--output".to_string(),
            meta_strip.to_string_lossy().to_string(),
            "--strip".to_string(),
            "--force".to_string(),
        ],
        None,
    )?;
    let metadata_ok = meta_keep_res.code == Some(0) && meta_strip_res.code == Some(0);

    let compat_io_ok = file_io_ok && stdio_ok && batch_ok && overwrite_ok && metadata_ok;
    Ok((compat_exit_ok, compat_io_ok))
}

fn run_stability_check(
    root: &Path,
    binary: &Path,
    reports_dir: &Path,
    platform_label: &str,
) -> AppResult<(i32, i32)> {
    let mut crash_like_count = 0;
    let mut failures_count = 0;

    let samples = load_samples(root, &ALL_SPLITS)?;
    for sample in samples {
        let input_path = root
            .join("dataset")
            .join(&sample.split)
            .join(&sample.filename);
        let stem = Path::new(&sample.filename)
            .file_stem()
            .and_then(|v| v.to_str())
            .unwrap_or("sample");
        let output_path = reports_dir
            .join("out")
            .join(platform_label)
            .join("stability")
            .join("regression")
            .join(&sample.split)
            .join(format!("{stem}.png"));
        if let Some(parent) = output_path.parent() {
            fs::create_dir_all(parent)?;
        }
        if output_path.exists() {
            let _ = fs::remove_file(&output_path);
        }

        let output = run_command(
            root,
            binary,
            &vec![
                input_path.to_string_lossy().to_string(),
                "--quality".to_string(),
                "55-75".to_string(),
                "--speed".to_string(),
                "4".to_string(),
                "--force".to_string(),
                "--quiet".to_string(),
                "--output".to_string(),
                output_path.to_string_lossy().to_string(),
            ],
            None,
        )?;

        let success = output.code == Some(0) && output_path.exists();
        let panicked = output.stderr.to_ascii_lowercase().contains("panic");
        let signaled = output.code.is_none();
        let unstable = panicked || signaled;
        if unstable {
            crash_like_count += 1;
        }

        let behavior_ok = if sample.expected_success {
            success
        } else {
            output.code != Some(0)
        };
        if unstable || !behavior_ok {
            failures_count += 1;
        }
    }

    let fuzz_cases = 24usize;
    let seed_samples = load_samples(root, &COMPARE_SPLITS)?
        .into_iter()
        .filter(|s| s.expected_success)
        .collect::<Vec<_>>();
    let fuzz_input_dir = reports_dir
        .join("out")
        .join(platform_label)
        .join("stability")
        .join("fuzz-inputs");
    fs::create_dir_all(&fuzz_input_dir)?;

    for idx in 0..fuzz_cases {
        let seed = &seed_samples[idx % seed_samples.len()];
        let seed_path = root.join("dataset").join(&seed.split).join(&seed.filename);
        let src = fs::read(seed_path)?;
        let mutated = mutate_bytes(&src, idx as u64);
        let fuzz_name = format!("fuzz-{:04}.png", idx + 1);
        let fuzz_path = fuzz_input_dir.join(&fuzz_name);
        fs::write(&fuzz_path, mutated)?;

        let output_path = reports_dir
            .join("out")
            .join(platform_label)
            .join("stability")
            .join("fuzz")
            .join(format!("fuzz-{:04}.out.png", idx + 1));
        if let Some(parent) = output_path.parent() {
            fs::create_dir_all(parent)?;
        }
        if output_path.exists() {
            let _ = fs::remove_file(&output_path);
        }

        let output = run_command(
            root,
            binary,
            &vec![
                fuzz_path.to_string_lossy().to_string(),
                "--quality".to_string(),
                "55-75".to_string(),
                "--speed".to_string(),
                "4".to_string(),
                "--force".to_string(),
                "--quiet".to_string(),
                "--output".to_string(),
                output_path.to_string_lossy().to_string(),
            ],
            None,
        )?;

        let panicked = output.stderr.to_ascii_lowercase().contains("panic");
        let signaled = output.code.is_none();
        let unstable = panicked || signaled;
        if unstable {
            crash_like_count += 1;
            failures_count += 1;
        }
    }

    Ok((crash_like_count, failures_count))
}

fn load_samples(root: &Path, splits: &[&str]) -> AppResult<Vec<Sample>> {
    let mut samples = Vec::new();
    for split in splits {
        let manifest_path = root.join("dataset").join(split).join("manifest.json");
        if !manifest_path.exists() {
            continue;
        }
        let raw = fs::read_to_string(&manifest_path)?;
        let entries: Vec<ManifestEntry> = serde_json::from_str(&raw)?;
        for (idx, entry) in entries.into_iter().enumerate() {
            samples.push(Sample {
                split: (*split).to_string(),
                sample_id: entry
                    .id
                    .unwrap_or_else(|| format!("{}-{:03}", split, idx + 1)),
                filename: entry.filename,
                expected_success: entry.expected_success.unwrap_or(*split != "robustness"),
            });
        }
    }
    Ok(samples)
}

fn run_command(
    root: &Path,
    binary: &Path,
    args: &[String],
    stdin_bytes: Option<&[u8]>,
) -> AppResult<CmdOutput> {
    let mut cmd = Command::new(binary);
    cmd.current_dir(root)
        .args(args.iter().map(OsString::from))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    if stdin_bytes.is_some() {
        cmd.stdin(Stdio::piped());
    }

    let mut child = cmd.spawn()?;
    if let Some(input) = stdin_bytes {
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(input)?;
        }
    }

    let output = child.wait_with_output()?;
    Ok(CmdOutput {
        code: output.status.code(),
        stdout: output.stdout,
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

fn resolve_binary_path(root: &Path, raw_path: &str) -> PathBuf {
    let mut path = PathBuf::from(raw_path);
    if path.is_relative() {
        path = root.join(path);
    }
    if path.exists() {
        return path;
    }

    if path.extension().is_none() {
        if let Some(name) = path.file_name().and_then(|v| v.to_str()) {
            let exe = path.with_file_name(format!("{name}.exe"));
            if exe.exists() {
                return exe;
            }
        }
    }

    path
}

fn sha256_file(path: &Path) -> AppResult<String> {
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0_u8; 8192];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn spread(values: &[f64]) -> (f64, f64, f64) {
    if values.is_empty() {
        return (0.0, 0.0, 0.0);
    }
    let mut min_v = values[0];
    let mut max_v = values[0];
    for v in values.iter().copied() {
        if v < min_v {
            min_v = v;
        }
        if v > max_v {
            max_v = v;
        }
    }
    (min_v, max_v, max_v - min_v)
}

fn mean(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    values.iter().sum::<f64>() / values.len() as f64
}

fn median(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mid = sorted.len() / 2;
    if sorted.len() % 2 == 0 {
        (sorted[mid - 1] + sorted[mid]) / 2.0
    } else {
        sorted[mid]
    }
}

fn p95(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let idx = ((sorted.len() as f64 * 0.95).ceil() as usize).saturating_sub(1);
    sorted[idx]
}

fn default_platform_label() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}

fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        return s.to_string();
    }
    let mut out = s[..max_len].to_string();
    out.push_str("...");
    out
}

fn rustc_version() -> Option<String> {
    let output = Command::new("rustc").arg("--version").output().ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn write_tiny_png(path: &Path) -> AppResult<()> {
    let file = fs::File::create(path)?;
    let writer = BufWriter::new(file);
    let mut encoder = png::Encoder::new(writer, 1, 1);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    let mut png_writer = encoder.write_header()?;
    png_writer.write_image_data(&[0_u8, 0_u8, 0_u8])?;
    Ok(())
}

fn mutate_bytes(src: &[u8], seed: u64) -> Vec<u8> {
    if src.is_empty() {
        return PNG_SIG.to_vec();
    }

    let mode = (seed % 5) as u8;
    let mut data = src.to_vec();

    match mode {
        0 => {
            let n = ((seed as usize % data.len()).max(1)).min(data.len());
            data.truncate(n);
            data
        }
        1 => {
            let idx = (seed as usize) % data.len();
            data[idx] ^= 1 << ((seed as usize) % 8);
            data
        }
        2 => {
            let start = (seed as usize) % data.len();
            let block = ((seed as usize % 64) + 1).min(data.len() - start);
            for i in start..start + block {
                data[i] = ((seed + i as u64 * 131) % 256) as u8;
            }
            data
        }
        3 => {
            let start = (seed as usize) % data.len();
            let block = ((seed as usize % 128) + 1).min(data.len() - start);
            let insert_at = ((seed as usize * 7) % data.len()).min(data.len());
            let mut out = Vec::with_capacity(data.len() + block);
            out.extend_from_slice(&data[..insert_at]);
            out.extend_from_slice(&data[start..start + block]);
            out.extend_from_slice(&data[insert_at..]);
            out
        }
        _ => {
            let noise_len = (seed as usize % 128) + 1;
            for i in 0..noise_len {
                data.push(((seed + i as u64 * 17) % 256) as u8);
            }
            data
        }
    }
}
