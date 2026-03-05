#!/usr/bin/env python3
"""Run Phase-D quality/size comparison against pngquant baseline."""

from __future__ import annotations

import argparse
import csv
import json
import math
import statistics
import subprocess
from pathlib import Path

import numpy as np
from PIL import Image


ROOT = Path(__file__).resolve().parents[2]
DATASET = ROOT / "dataset"
REPORTS = ROOT / "reports" / "quality-size"
SPLITS = ("functional", "quality", "perf")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Run phase D quality/size comparison.")
    parser.add_argument("--run-id", default="quality-size-v1-20260305", help="Run id.")
    parser.add_argument(
        "--candidate",
        default=str(ROOT / "target" / "debug" / "pngoptim"),
        help="Candidate binary path.",
    )
    parser.add_argument("--baseline", default="pngquant", help="Baseline tool path.")
    parser.add_argument("--build", action="store_true", help="Build candidate before run.")
    parser.add_argument("--quality", default="55-75", help="Quality range.")
    parser.add_argument("--speed", default="4", help="Speed value.")
    return parser.parse_args()


def run_cmd(cmd: list[str]) -> dict:
    proc = subprocess.run(cmd, cwd=ROOT, capture_output=True)
    return {
        "exit_code": proc.returncode,
        "stdout": proc.stdout.decode("utf-8", errors="replace"),
        "stderr": proc.stderr.decode("utf-8", errors="replace"),
    }


def load_samples() -> list[dict]:
    samples: list[dict] = []
    for split in SPLITS:
        manifest = DATASET / split / "manifest.json"
        if not manifest.exists():
            continue
        entries = json.loads(manifest.read_text(encoding="utf-8"))
        for entry in entries:
            if not bool(entry.get("expected_success", True)):
                continue
            samples.append(
                {
                    "split": split,
                    "sample_id": entry["id"],
                    "filename": entry["filename"],
                }
            )
    return samples


def load_rgba(path: Path) -> np.ndarray:
    with Image.open(path) as im:
        return np.array(im.convert("RGBA"), dtype=np.float64)


def calc_psnr(src: np.ndarray, out: np.ndarray) -> float:
    mse = np.mean((src - out) ** 2)
    if mse == 0:
        return 99.0
    return 20.0 * math.log10(255.0 / math.sqrt(mse))


def calc_ssim_global(src: np.ndarray, out: np.ndarray) -> float:
    c1 = (0.01 * 255) ** 2
    c2 = (0.03 * 255) ** 2
    vals = []
    for c in range(src.shape[2]):
        x = src[:, :, c]
        y = out[:, :, c]
        mu_x = float(np.mean(x))
        mu_y = float(np.mean(y))
        sigma_x = float(np.mean((x - mu_x) ** 2))
        sigma_y = float(np.mean((y - mu_y) ** 2))
        sigma_xy = float(np.mean((x - mu_x) * (y - mu_y)))
        num = (2 * mu_x * mu_y + c1) * (2 * sigma_xy + c2)
        den = (mu_x**2 + mu_y**2 + c1) * (sigma_x + sigma_y + c2)
        vals.append(num / den if den != 0 else 1.0)
    return float(sum(vals) / len(vals))


def p95(values: list[float]) -> float:
    if not values:
        return 0.0
    idx = max(0, int(math.ceil(len(values) * 0.95)) - 1)
    return sorted(values)[idx]


def main() -> int:
    args = parse_args()
    if args.build:
        subprocess.run(["cargo", "build"], cwd=ROOT, check=True)

    candidate = Path(args.candidate)
    if not candidate.exists():
        print(f"candidate binary not found: {candidate}")
        return 2

    run_dir = REPORTS / args.run_id
    if run_dir.exists():
        subprocess.run(["rm", "-rf", str(run_dir)], check=True)
    base_out = run_dir / "baseline-out"
    cand_out = run_dir / "candidate-out"
    base_out.mkdir(parents=True, exist_ok=True)
    cand_out.mkdir(parents=True, exist_ok=True)

    samples = load_samples()
    size_rows = []
    quality_rows = []
    failures = []

    for sample in samples:
        split = sample["split"]
        sample_id = sample["sample_id"]
        filename = sample["filename"]
        src = DATASET / split / filename
        baseline_png = base_out / split / f"{Path(filename).stem}.baseline.png"
        candidate_png = cand_out / split / f"{Path(filename).stem}.candidate.png"
        baseline_png.parent.mkdir(parents=True, exist_ok=True)
        candidate_png.parent.mkdir(parents=True, exist_ok=True)

        baseline_res = run_cmd(
            [
                args.baseline,
                f"--quality={args.quality}",
                "--speed",
                args.speed,
                "--force",
                "--output",
                str(baseline_png),
                "--",
                str(src),
            ]
        )
        candidate_res = run_cmd(
            [
                str(candidate),
                str(src),
                "--quality",
                args.quality,
                "--speed",
                args.speed,
                "--strip",
                "--force",
                "--output",
                str(candidate_png),
                "--quiet",
            ]
        )

        baseline_ok = baseline_res["exit_code"] == 0 and baseline_png.exists()
        candidate_ok = candidate_res["exit_code"] == 0 and candidate_png.exists()
        input_bytes = src.stat().st_size
        baseline_bytes = baseline_png.stat().st_size if baseline_ok else None
        candidate_bytes = candidate_png.stat().st_size if candidate_ok else None

        if not baseline_ok or not candidate_ok:
            failures.append(
                {
                    "sample_id": sample_id,
                    "split": split,
                    "input_file": filename,
                    "baseline_exit": baseline_res["exit_code"],
                    "candidate_exit": candidate_res["exit_code"],
                    "baseline_stderr": baseline_res["stderr"][:400],
                    "candidate_stderr": candidate_res["stderr"][:400],
                }
            )

        ratio_baseline = (baseline_bytes / input_bytes) if baseline_bytes is not None else None
        ratio_candidate = (candidate_bytes / input_bytes) if candidate_bytes is not None else None
        delta_vs_baseline = (
            ((candidate_bytes - baseline_bytes) / baseline_bytes)
            if baseline_bytes is not None and candidate_bytes is not None and baseline_bytes > 0
            else None
        )

        size_rows.append(
            {
                "run_id": args.run_id,
                "split": split,
                "sample_id": sample_id,
                "input_file": filename,
                "input_bytes": input_bytes,
                "baseline_bytes": baseline_bytes if baseline_bytes is not None else "",
                "candidate_bytes": candidate_bytes if candidate_bytes is not None else "",
                "baseline_ratio": f"{ratio_baseline:.6f}" if ratio_baseline is not None else "",
                "candidate_ratio": f"{ratio_candidate:.6f}" if ratio_candidate is not None else "",
                "delta_candidate_vs_baseline": f"{delta_vs_baseline:.6f}"
                if delta_vs_baseline is not None
                else "",
                "baseline_exit": baseline_res["exit_code"],
                "candidate_exit": candidate_res["exit_code"],
            }
        )

        if baseline_ok and candidate_ok and split in ("functional", "quality"):
            src_rgba = load_rgba(src)
            b_rgba = load_rgba(baseline_png)
            c_rgba = load_rgba(candidate_png)
            if src_rgba.shape == b_rgba.shape == c_rgba.shape:
                psnr_b = calc_psnr(src_rgba, b_rgba)
                psnr_c = calc_psnr(src_rgba, c_rgba)
                ssim_b = calc_ssim_global(src_rgba, b_rgba)
                ssim_c = calc_ssim_global(src_rgba, c_rgba)
            else:
                psnr_b = psnr_c = ssim_b = ssim_c = float("nan")

            quality_rows.append(
                {
                    "run_id": args.run_id,
                    "split": split,
                    "sample_id": sample_id,
                    "input_file": filename,
                    "psnr_baseline": f"{psnr_b:.6f}" if not math.isnan(psnr_b) else "",
                    "psnr_candidate": f"{psnr_c:.6f}" if not math.isnan(psnr_c) else "",
                    "psnr_delta_candidate_minus_baseline": f"{(psnr_c - psnr_b):.6f}"
                    if not math.isnan(psnr_b) and not math.isnan(psnr_c)
                    else "",
                    "ssim_baseline": f"{ssim_b:.6f}" if not math.isnan(ssim_b) else "",
                    "ssim_candidate": f"{ssim_c:.6f}" if not math.isnan(ssim_c) else "",
                    "ssim_delta_candidate_minus_baseline": f"{(ssim_c - ssim_b):.6f}"
                    if not math.isnan(ssim_b) and not math.isnan(ssim_c)
                    else "",
                }
            )

    with (run_dir / "size_report.csv").open("w", newline="", encoding="utf-8") as f:
        fields = list(size_rows[0].keys()) if size_rows else []
        writer = csv.DictWriter(f, fieldnames=fields)
        writer.writeheader()
        writer.writerows(size_rows)

    with (run_dir / "quality_report.csv").open("w", newline="", encoding="utf-8") as f:
        fields = list(quality_rows[0].keys()) if quality_rows else []
        writer = csv.DictWriter(f, fieldnames=fields)
        writer.writeheader()
        writer.writerows(quality_rows)

    (run_dir / "failures.json").write_text(json.dumps(failures, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")

    size_deltas = [
        float(r["delta_candidate_vs_baseline"])
        for r in size_rows
        if r["delta_candidate_vs_baseline"] != ""
    ]
    psnr_deltas = [
        float(r["psnr_delta_candidate_minus_baseline"])
        for r in quality_rows
        if r["psnr_delta_candidate_minus_baseline"] != ""
    ]
    ssim_deltas = [
        float(r["ssim_delta_candidate_minus_baseline"])
        for r in quality_rows
        if r["ssim_delta_candidate_minus_baseline"] != ""
    ]
    top_regressions = sorted(
        [r for r in size_rows if r["delta_candidate_vs_baseline"] != ""],
        key=lambda r: float(r["delta_candidate_vs_baseline"]),
        reverse=True,
    )[:10]
    (run_dir / "top_regressions.json").write_text(
        json.dumps(top_regressions, ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
    )

    summary = [
        "# Quality & Size Report v1",
        "",
        f"- run_id: `{args.run_id}`",
        f"- samples_total: {len(size_rows)}",
        f"- failures: {len(failures)}",
        f"- avg_delta_size_candidate_vs_baseline: {statistics.mean(size_deltas):.6f}"
        if size_deltas
        else "- avg_delta_size_candidate_vs_baseline: n/a",
        f"- median_delta_size_candidate_vs_baseline: {statistics.median(size_deltas):.6f}"
        if size_deltas
        else "- median_delta_size_candidate_vs_baseline: n/a",
        f"- p95_delta_size_candidate_vs_baseline: {p95(size_deltas):.6f}"
        if size_deltas
        else "- p95_delta_size_candidate_vs_baseline: n/a",
        f"- avg_psnr_delta_candidate_minus_baseline: {statistics.mean(psnr_deltas):.6f}"
        if psnr_deltas
        else "- avg_psnr_delta_candidate_minus_baseline: n/a",
        f"- avg_ssim_delta_candidate_minus_baseline: {statistics.mean(ssim_deltas):.6f}"
        if ssim_deltas
        else "- avg_ssim_delta_candidate_minus_baseline: n/a",
        "",
        "Artifacts:",
        f"- `reports/quality-size/{args.run_id}/size_report.csv`",
        f"- `reports/quality-size/{args.run_id}/quality_report.csv`",
        f"- `reports/quality-size/{args.run_id}/failures.json`",
        f"- `reports/quality-size/{args.run_id}/top_regressions.json`",
    ]
    (run_dir / "summary.md").write_text("\n".join(summary) + "\n", encoding="utf-8")

    print(f"Quality-size run complete: {run_dir}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
