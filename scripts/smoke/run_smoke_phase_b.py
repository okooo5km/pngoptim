#!/usr/bin/env python3
"""Run phase-B smoke checks for all dataset manifests."""

from __future__ import annotations

import argparse
import csv
import json
import subprocess
import sys
import time
from datetime import datetime
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
SPLITS = ("functional", "quality", "perf", "robustness")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Run phase-B smoke suite.")
    parser.add_argument(
        "--run-id",
        default=f"smoke-v1-{datetime.now().strftime('%Y%m%d-%H%M%S')}",
        help="Unique run id for output folder.",
    )
    parser.add_argument(
        "--binary",
        default=str(ROOT / "target" / "debug" / "pngoptim"),
        help="Path to pngoptim binary.",
    )
    parser.add_argument(
        "--build",
        action="store_true",
        help="Build binary before running smoke checks.",
    )
    return parser.parse_args()


def load_samples() -> list[dict]:
    samples: list[dict] = []
    for split in SPLITS:
        manifest = ROOT / "dataset" / split / "manifest.json"
        if not manifest.exists():
            continue
        entries = json.loads(manifest.read_text(encoding="utf-8"))
        for i, entry in enumerate(entries):
            samples.append(
                {
                    "split": split,
                    "sample_id": entry.get("id", f"{split}-{i+1:03d}"),
                    "filename": entry["filename"],
                    "expected_success": bool(entry.get("expected_success", split != "robustness")),
                }
            )
    return samples


def run_smoke_sample(binary: str, input_path: Path, output_path: Path) -> dict:
    cmd = [
        binary,
        str(input_path),
        "--output",
        str(output_path),
        "--force",
        "--quality",
        "60-85",
        "--speed",
        "4",
    ]
    start = time.perf_counter()
    proc = subprocess.run(cmd, capture_output=True, text=True)
    elapsed_ms = int((time.perf_counter() - start) * 1000)
    return {
        "exit_code": proc.returncode,
        "elapsed_ms": elapsed_ms,
        "stdout": (proc.stdout or "").strip(),
        "stderr": (proc.stderr or "").strip(),
    }


def main() -> int:
    args = parse_args()
    if args.build:
        subprocess.run(["cargo", "build"], cwd=ROOT, check=True)

    binary = Path(args.binary)
    if not binary.exists():
        print(f"binary not found: {binary}", file=sys.stderr)
        return 2

    samples = load_samples()
    if not samples:
        print("no samples found in dataset manifests", file=sys.stderr)
        return 2

    run_dir = ROOT / "reports" / "smoke" / args.run_id
    out_dir = run_dir / "out"
    run_dir.mkdir(parents=True, exist_ok=True)

    rows = []
    failures = []
    passed = 0

    for sample in samples:
        split = sample["split"]
        sample_id = sample["sample_id"]
        filename = sample["filename"]
        expected_success = sample["expected_success"]
        input_path = ROOT / "dataset" / split / filename
        output_path = out_dir / split / f"{Path(filename).stem}.smoke.png"
        output_path.parent.mkdir(parents=True, exist_ok=True)

        result = run_smoke_sample(str(binary), input_path, output_path)
        success = result["exit_code"] == 0 and output_path.exists()
        row_passed = success if expected_success else result["exit_code"] != 0

        if row_passed:
            passed += 1
        else:
            failures.append(
                {
                    "split": split,
                    "sample_id": sample_id,
                    "filename": filename,
                    "expected_success": expected_success,
                    "exit_code": result["exit_code"],
                    "stdout": result["stdout"][:500],
                    "stderr": result["stderr"][:500],
                }
            )

        rows.append(
            {
                "run_id": args.run_id,
                "dataset_split": split,
                "sample_id": sample_id,
                "input_file": filename,
                "expected_success": str(expected_success).lower(),
                "exit_code": result["exit_code"],
                "elapsed_ms": result["elapsed_ms"],
                "actual_success": str(success).lower(),
                "passed": str(row_passed).lower(),
                "output_file": output_path.name if output_path.exists() else "",
                "stderr": result["stderr"][:200].replace("\n", "\\n"),
            }
        )

    with (run_dir / "smoke_report.csv").open("w", newline="", encoding="utf-8") as f:
        fields = [
            "run_id",
            "dataset_split",
            "sample_id",
            "input_file",
            "expected_success",
            "exit_code",
            "elapsed_ms",
            "actual_success",
            "passed",
            "output_file",
            "stderr",
        ]
        writer = csv.DictWriter(f, fieldnames=fields)
        writer.writeheader()
        writer.writerows(rows)

    (run_dir / "failures.json").write_text(json.dumps(failures, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")

    summary = [
        "# Smoke Report v1",
        "",
        f"- run_id: `{args.run_id}`",
        f"- total: {len(rows)}",
        f"- passed: {passed}",
        f"- failed: {len(rows) - passed}",
        f"- failures_file: `reports/smoke/{args.run_id}/failures.json`",
        f"- report_file: `reports/smoke/{args.run_id}/smoke_report.csv`",
    ]
    (run_dir / "summary.md").write_text("\n".join(summary) + "\n", encoding="utf-8")

    print(f"Smoke run complete: {run_dir}")
    return 0 if passed == len(rows) else 1


if __name__ == "__main__":
    raise SystemExit(main())

