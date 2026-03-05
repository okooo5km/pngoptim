#!/usr/bin/env python3
"""Run baseline evaluation against pngquant and export contract reports."""

from __future__ import annotations

import argparse
import csv
import json
import math
import platform
import statistics
import subprocess
import sys
import time
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path

import numpy as np
from PIL import Image

if sys.version_info >= (3, 11):
    import tomllib
else:
    import tomli as tomllib  # type: ignore[no-redef]


ROOT = Path(__file__).resolve().parents[2]
DEFAULT_SPLITS = ("functional", "quality", "perf", "robustness")
QUALITY_METRIC_SPLITS = {"functional", "quality"}


@dataclass
class Sample:
    split: str
    sample_id: str
    filename: str
    expected_success: bool

    @property
    def input_path(self) -> Path:
        return ROOT / "dataset" / self.split / self.filename


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Run baseline reports for phase A.")
    parser.add_argument("--run-id", required=True, help="Unique run id, e.g. baseline-v1-20260305")
    parser.add_argument("--profile", default="Q_MED", help="Profile from parameter_matrix_v1.toml")
    parser.add_argument(
        "--splits",
        default=",".join(DEFAULT_SPLITS),
        help="Comma separated dataset splits to run (functional,quality,perf,robustness)",
    )
    parser.add_argument("--pngquant", default="pngquant", help="Path to pngquant executable")
    return parser.parse_args()


def load_profile(profile_name: str) -> dict:
    matrix_path = ROOT / "config" / "evaluation" / "parameter_matrix_v1.toml"
    matrix = tomllib.loads(matrix_path.read_text(encoding="utf-8"))
    profiles = matrix.get("profiles", {})
    if profile_name not in profiles:
        raise ValueError(f"Profile not found: {profile_name}")
    return profiles[profile_name]


def load_samples(splits: list[str]) -> list[Sample]:
    samples: list[Sample] = []
    for split in splits:
        manifest_path = ROOT / "dataset" / split / "manifest.json"
        if not manifest_path.exists():
            continue
        entries = json.loads(manifest_path.read_text(encoding="utf-8"))
        for i, entry in enumerate(entries):
            sample_id = entry.get("id", f"{split}-{i+1:03d}")
            filename = entry["filename"]
            expected_success = bool(entry.get("expected_success", split != "robustness"))
            samples.append(Sample(split=split, sample_id=sample_id, filename=filename, expected_success=expected_success))
    return samples


def run_pngquant(
    pngquant: str,
    profile: dict,
    input_path: Path,
    output_path: Path,
) -> tuple[int, int, str]:
    args = [pngquant]
    qmin = profile.get("quality_min")
    qmax = profile.get("quality_max")
    if qmin is not None and qmax is not None:
        args.append(f"--quality={qmin}-{qmax}")
    speed = profile.get("speed")
    if speed is not None:
        args.extend(["--speed", str(speed)])
    dither = profile.get("dither", "fs")
    if dither == "nofs":
        args.append("--nofs")
    args.extend(["--force", "--output", str(output_path), "--", str(input_path)])

    start = time.perf_counter()
    proc = subprocess.run(args, capture_output=True, text=True)
    elapsed_ms = int((time.perf_counter() - start) * 1000)
    stderr = (proc.stderr or "").strip()
    return proc.returncode, elapsed_ms, stderr


def load_rgba(path: Path) -> np.ndarray:
    with Image.open(path) as im:
        return np.array(im.convert("RGBA"), dtype=np.float64)


def calc_psnr(a: np.ndarray, b: np.ndarray) -> float:
    mse = np.mean((a - b) ** 2)
    if mse == 0:
        return 99.0
    return 20.0 * math.log10(255.0 / math.sqrt(mse))


def calc_global_ssim(a: np.ndarray, b: np.ndarray) -> float:
    # Global SSIM approximation by channel; deterministic and dependency-light.
    c1 = (0.01 * 255) ** 2
    c2 = (0.03 * 255) ** 2
    vals = []
    for c in range(a.shape[2]):
        x = a[:, :, c]
        y = b[:, :, c]
        mu_x = float(np.mean(x))
        mu_y = float(np.mean(y))
        sigma_x = float(np.mean((x - mu_x) ** 2))
        sigma_y = float(np.mean((y - mu_y) ** 2))
        sigma_xy = float(np.mean((x - mu_x) * (y - mu_y)))
        num = (2 * mu_x * mu_y + c1) * (2 * sigma_xy + c2)
        den = (mu_x**2 + mu_y**2 + c1) * (sigma_x + sigma_y + c2)
        vals.append(num / den if den != 0 else 1.0)
    return float(sum(vals) / len(vals))


def safe_float(v: str | float | None) -> float | None:
    if v is None or v == "":
        return None
    return float(v)


def p95(values: list[float]) -> float:
    if not values:
        return 0.0
    if len(values) == 1:
        return values[0]
    idx = max(0, int(math.ceil(len(values) * 0.95)) - 1)
    return sorted(values)[idx]


def main() -> int:
    args = parse_args()
    splits = [s.strip() for s in args.splits.split(",") if s.strip()]
    profile = load_profile(args.profile)
    samples = load_samples(splits)
    if not samples:
        print("No samples found for selected splits.", file=sys.stderr)
        return 2

    report_dir = ROOT / "reports" / "baseline" / args.run_id
    out_dir = report_dir / "out"
    report_dir.mkdir(parents=True, exist_ok=True)

    size_rows: list[dict] = []
    quality_rows: list[dict] = []
    perf_rows: list[dict] = []
    failures: list[dict] = []

    for sample in samples:
        input_path = sample.input_path
        output_path = out_dir / sample.split / f"{Path(sample.filename).stem}.q.png"
        output_path.parent.mkdir(parents=True, exist_ok=True)

        if not input_path.exists():
            failures.append(
                {
                    "split": sample.split,
                    "sample_id": sample.sample_id,
                    "filename": sample.filename,
                    "reason": "input_not_found",
                }
            )
            continue

        input_bytes = input_path.stat().st_size
        exit_code, elapsed_ms, stderr = run_pngquant(args.pngquant, profile, input_path, output_path)
        success = exit_code == 0 and output_path.exists()
        status = "success" if success else "failed"

        output_bytes = output_path.stat().st_size if success else None
        size_ratio = (output_bytes / input_bytes) if success and input_bytes else None
        delta_bytes = (output_bytes - input_bytes) if success else None

        size_rows.append(
            {
                "run_id": args.run_id,
                "profile": args.profile,
                "dataset_split": sample.split,
                "sample_id": sample.sample_id,
                "input_file": sample.filename,
                "input_bytes": input_bytes,
                "output_file": output_path.name if success else "",
                "output_bytes": output_bytes if output_bytes is not None else "",
                "size_ratio": f"{size_ratio:.6f}" if size_ratio is not None else "",
                "delta_bytes": delta_bytes if delta_bytes is not None else "",
                "exit_code": exit_code,
                "expected_success": str(sample.expected_success).lower(),
                "status": status,
            }
        )

        perf_rows.append(
            {
                "run_id": args.run_id,
                "profile": args.profile,
                "dataset_split": sample.split,
                "sample_id": sample.sample_id,
                "input_file": sample.filename,
                "elapsed_ms": elapsed_ms,
                "exit_code": exit_code,
                "expected_success": str(sample.expected_success).lower(),
                "status": status,
            }
        )

        if success and sample.split in QUALITY_METRIC_SPLITS:
            shape_match = False
            psnr = ""
            ssim = ""
            try:
                src = load_rgba(input_path)
                dst = load_rgba(output_path)
                shape_match = src.shape == dst.shape
                if shape_match:
                    psnr = f"{calc_psnr(src, dst):.6f}"
                    ssim = f"{calc_global_ssim(src, dst):.6f}"
            except Exception as exc:  # noqa: BLE001
                failures.append(
                    {
                        "split": sample.split,
                        "sample_id": sample.sample_id,
                        "filename": sample.filename,
                        "reason": "quality_metric_error",
                        "detail": str(exc),
                    }
                )

            quality_rows.append(
                {
                    "run_id": args.run_id,
                    "profile": args.profile,
                    "dataset_split": sample.split,
                    "sample_id": sample.sample_id,
                    "input_file": sample.filename,
                    "output_file": output_path.name,
                    "psnr_db": psnr,
                    "ssim": ssim,
                    "shape_match": str(shape_match).lower(),
                    "exit_code": exit_code,
                    "status": status,
                }
            )
        elif sample.split in QUALITY_METRIC_SPLITS:
            quality_rows.append(
                {
                    "run_id": args.run_id,
                    "profile": args.profile,
                    "dataset_split": sample.split,
                    "sample_id": sample.sample_id,
                    "input_file": sample.filename,
                    "output_file": "",
                    "psnr_db": "",
                    "ssim": "",
                    "shape_match": "false",
                    "exit_code": exit_code,
                    "status": status,
                }
            )

        if sample.expected_success != success:
            failures.append(
                {
                    "split": sample.split,
                    "sample_id": sample.sample_id,
                    "filename": sample.filename,
                    "reason": "unexpected_result",
                    "expected_success": sample.expected_success,
                    "actual_success": success,
                    "exit_code": exit_code,
                    "stderr": stderr[:800],
                }
            )

    size_fields = [
        "run_id",
        "profile",
        "dataset_split",
        "sample_id",
        "input_file",
        "input_bytes",
        "output_file",
        "output_bytes",
        "size_ratio",
        "delta_bytes",
        "exit_code",
        "expected_success",
        "status",
    ]
    quality_fields = [
        "run_id",
        "profile",
        "dataset_split",
        "sample_id",
        "input_file",
        "output_file",
        "psnr_db",
        "ssim",
        "shape_match",
        "exit_code",
        "status",
    ]
    perf_fields = [
        "run_id",
        "profile",
        "dataset_split",
        "sample_id",
        "input_file",
        "elapsed_ms",
        "exit_code",
        "expected_success",
        "status",
    ]

    (report_dir / "size_report.csv").write_text("", encoding="utf-8")
    with (report_dir / "size_report.csv").open("w", newline="", encoding="utf-8") as f:
        writer = csv.DictWriter(f, fieldnames=size_fields)
        writer.writeheader()
        writer.writerows(size_rows)

    with (report_dir / "quality_report.csv").open("w", newline="", encoding="utf-8") as f:
        writer = csv.DictWriter(f, fieldnames=quality_fields)
        writer.writeheader()
        writer.writerows(quality_rows)

    with (report_dir / "perf_report.csv").open("w", newline="", encoding="utf-8") as f:
        writer = csv.DictWriter(f, fieldnames=perf_fields)
        writer.writeheader()
        writer.writerows(perf_rows)

    (report_dir / "failures.json").write_text(json.dumps(failures, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")

    size_ratios = [safe_float(r["size_ratio"]) for r in size_rows if r["status"] == "success"]
    size_ratios = [v for v in size_ratios if v is not None]
    elapsed_vals = [float(r["elapsed_ms"]) for r in perf_rows]
    psnr_vals = [safe_float(r["psnr_db"]) for r in quality_rows if r["status"] == "success"]
    psnr_vals = [v for v in psnr_vals if v is not None]
    ssim_vals = [safe_float(r["ssim"]) for r in quality_rows if r["status"] == "success"]
    ssim_vals = [v for v in ssim_vals if v is not None]

    success_count = sum(1 for r in size_rows if r["status"] == "success")
    fail_count = len(size_rows) - success_count
    expected_fail_count = sum(1 for s in samples if not s.expected_success)
    unexpected_count = sum(1 for f in failures if f.get("reason") == "unexpected_result")

    summary = [
        "# Baseline Run Summary",
        "",
        f"- run_id: `{args.run_id}`",
        f"- profile: `{args.profile}`",
        f"- splits: `{','.join(splits)}`",
        f"- total_samples: {len(size_rows)}",
        f"- success: {success_count}",
        f"- failed: {fail_count}",
        f"- expected_failures_configured: {expected_fail_count}",
        f"- unexpected_results: {unexpected_count}",
        "",
        "## Aggregate Metrics",
        "",
        f"- size_ratio_mean: {statistics.mean(size_ratios):.6f}" if size_ratios else "- size_ratio_mean: n/a",
        f"- size_ratio_median: {statistics.median(size_ratios):.6f}" if size_ratios else "- size_ratio_median: n/a",
        f"- size_ratio_p95: {p95(size_ratios):.6f}" if size_ratios else "- size_ratio_p95: n/a",
        f"- elapsed_ms_mean: {statistics.mean(elapsed_vals):.2f}" if elapsed_vals else "- elapsed_ms_mean: n/a",
        f"- elapsed_ms_median: {statistics.median(elapsed_vals):.2f}" if elapsed_vals else "- elapsed_ms_median: n/a",
        f"- elapsed_ms_p95: {p95(elapsed_vals):.2f}" if elapsed_vals else "- elapsed_ms_p95: n/a",
        f"- psnr_db_mean: {statistics.mean(psnr_vals):.6f}" if psnr_vals else "- psnr_db_mean: n/a",
        f"- ssim_mean: {statistics.mean(ssim_vals):.6f}" if ssim_vals else "- ssim_mean: n/a",
        "",
        "## Output Files",
        "",
        f"- `reports/baseline/{args.run_id}/size_report.csv`",
        f"- `reports/baseline/{args.run_id}/quality_report.csv`",
        f"- `reports/baseline/{args.run_id}/perf_report.csv`",
        f"- `reports/baseline/{args.run_id}/failures.json`",
    ]
    (report_dir / "summary.md").write_text("\n".join(summary) + "\n", encoding="utf-8")

    run_meta = {
        "generated_at_utc": datetime.now(timezone.utc).isoformat(),
        "run_id": args.run_id,
        "profile": args.profile,
        "splits": splits,
        "tool": args.pngquant,
        "platform": {
            "system": platform.system(),
            "release": platform.release(),
            "machine": platform.machine(),
            "python_version": platform.python_version(),
        },
    }
    (report_dir / "run_meta.json").write_text(json.dumps(run_meta, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")

    print(f"Baseline run complete: {report_dir}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

