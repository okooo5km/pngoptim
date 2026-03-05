#!/usr/bin/env python3
"""Run Phase-E performance and memory comparison against pngquant baseline."""

from __future__ import annotations

import argparse
import csv
import json
import math
import os
import platform
import re
import statistics
import subprocess
import time
from collections import defaultdict
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
DATASET = ROOT / "dataset"
REPORTS = ROOT / "reports" / "perf"
SPLITS = ("functional", "quality", "perf")

PROFILE_PREFIX = "profile_metrics\t"


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Run Phase-E perf + memory benchmark.")
    parser.add_argument("--run-id", default="perf-v1-20260305", help="Run id.")
    parser.add_argument(
        "--candidate",
        default=str(ROOT / "target" / "release" / "pngoptim"),
        help="Candidate binary path.",
    )
    parser.add_argument("--baseline", default="pngquant", help="Baseline tool path.")
    parser.add_argument("--build", action="store_true", help="Build candidate before run.")
    parser.add_argument("--quality", default="55-75", help="Quality range.")
    parser.add_argument("--speed", default="4", help="Speed value.")
    parser.add_argument(
        "--iterations",
        type=int,
        default=3,
        help="Iterations per sample for each tool.",
    )
    return parser.parse_args()


def resolve_binary_path(raw_path: str) -> Path:
    path = Path(raw_path)
    if path.exists():
        return path
    if os.name == "nt" and path.suffix.lower() != ".exe":
        exe_path = path.with_name(path.name + ".exe")
        if exe_path.exists():
            return exe_path
    return path


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


def percentile(values: list[float], pct: float) -> float:
    if not values:
        return 0.0
    idx = max(0, int(math.ceil(len(values) * pct)) - 1)
    return sorted(values)[idx]


def parse_profile_metrics(stderr_text: str) -> dict[str, float] | None:
    for line in stderr_text.splitlines():
        if not line.startswith(PROFILE_PREFIX):
            continue
        data: dict[str, float] = {}
        for part in line[len(PROFILE_PREFIX) :].split("\t"):
            if "=" not in part:
                continue
            key, val = part.split("=", 1)
            if key.endswith("_ms"):
                try:
                    data[key] = float(val)
                except ValueError:
                    pass
        if data:
            return data
    return None


def parse_max_rss_kb(stderr_text: str) -> int | None:
    system = platform.system()
    if system == "Darwin":
        m = re.search(r"(\d+)\s+maximum resident set size", stderr_text)
        if m:
            return int(m.group(1))
    elif system == "Linux":
        # Matched from "__MAXRSS_KB__<value>" marker.
        m = re.search(r"__MAXRSS_KB__(\d+)", stderr_text)
        if m:
            return int(m.group(1))
    return None


def run_with_metrics(cmd: list[str], env: dict[str, str] | None = None) -> dict:
    wrapped = cmd
    system = platform.system()
    if system == "Darwin":
        wrapped = ["/usr/bin/time", "-l", *cmd]
    elif system == "Linux":
        wrapped = ["/usr/bin/time", "-f", "__MAXRSS_KB__%M", *cmd]

    start = time.perf_counter()
    proc = subprocess.run(
        wrapped,
        cwd=ROOT,
        capture_output=True,
        text=True,
        env=env,
    )
    elapsed_ms = (time.perf_counter() - start) * 1000.0
    stderr = proc.stderr or ""
    return {
        "exit_code": proc.returncode,
        "stdout": proc.stdout or "",
        "stderr": stderr,
        "elapsed_ms": elapsed_ms,
        "max_rss_kb": parse_max_rss_kb(stderr),
        "profile": parse_profile_metrics(stderr),
    }


def mean_or_zero(values: list[float]) -> float:
    return statistics.mean(values) if values else 0.0


def median_or_zero(values: list[float]) -> float:
    return statistics.median(values) if values else 0.0


def main() -> int:
    args = parse_args()
    if args.build:
        subprocess.run(["cargo", "build", "--release"], cwd=ROOT, check=True)

    candidate = resolve_binary_path(args.candidate)
    if not candidate.exists():
        print(f"candidate binary not found: {candidate}")
        return 2

    run_dir = REPORTS / args.run_id
    if run_dir.exists():
        subprocess.run(["rm", "-rf", str(run_dir)], check=True)
    run_dir.mkdir(parents=True, exist_ok=True)

    out_dir = run_dir / "out"
    samples = load_samples()
    rows: list[dict] = []
    failures: list[dict] = []

    for sample in samples:
        split = sample["split"]
        sample_id = sample["sample_id"]
        filename = sample["filename"]
        src = DATASET / split / filename
        input_bytes = src.stat().st_size

        for tool in ("baseline", "candidate"):
            for iteration in range(1, args.iterations + 1):
                out_png = (
                    out_dir
                    / tool
                    / split
                    / f"{Path(filename).stem}.{tool}.{iteration}.png"
                )
                out_png.parent.mkdir(parents=True, exist_ok=True)
                if out_png.exists():
                    out_png.unlink()

                if tool == "baseline":
                    cmd = [
                        args.baseline,
                        f"--quality={args.quality}",
                        "--speed",
                        args.speed,
                        "--force",
                        "--output",
                        str(out_png),
                        "--",
                        str(src),
                    ]
                    env = None
                else:
                    cmd = [
                        str(candidate),
                        str(src),
                        "--quality",
                        args.quality,
                        "--speed",
                        args.speed,
                        "--strip",
                        "--force",
                        "--output",
                        str(out_png),
                        "--quiet",
                    ]
                    env = dict(os.environ)
                    env["PNGOPTIM_PROFILE_METRICS"] = "1"

                result = run_with_metrics(cmd, env=env)
                ok = result["exit_code"] == 0 and out_png.exists()
                output_bytes = out_png.stat().st_size if ok else None
                profile = result["profile"] if tool == "candidate" else None

                row = {
                    "run_id": args.run_id,
                    "tool": tool,
                    "split": split,
                    "sample_id": sample_id,
                    "input_file": filename,
                    "iteration": iteration,
                    "input_bytes": input_bytes,
                    "output_bytes": output_bytes if output_bytes is not None else "",
                    "elapsed_ms": f"{result['elapsed_ms']:.3f}",
                    "max_rss_kb": result["max_rss_kb"] if result["max_rss_kb"] is not None else "",
                    "decode_ms": f"{profile.get('decode_ms', 0.0):.3f}" if profile else "",
                    "quantize_ms": f"{profile.get('quantize_ms', 0.0):.3f}" if profile else "",
                    "encode_ms": f"{profile.get('encode_ms', 0.0):.3f}" if profile else "",
                    "total_ms": f"{profile.get('total_ms', 0.0):.3f}" if profile else "",
                    "exit_code": result["exit_code"],
                    "status": "success" if ok else "failed",
                }
                rows.append(row)

                if not ok:
                    failures.append(
                        {
                            "tool": tool,
                            "split": split,
                            "sample_id": sample_id,
                            "input_file": filename,
                            "iteration": iteration,
                            "exit_code": result["exit_code"],
                            "stderr": result["stderr"][:500],
                        }
                    )

    compare_path = run_dir / "perf_compare.csv"
    with compare_path.open("w", newline="", encoding="utf-8") as f:
        fields = list(rows[0].keys()) if rows else []
        writer = csv.DictWriter(f, fieldnames=fields)
        writer.writeheader()
        writer.writerows(rows)

    (run_dir / "failures.json").write_text(
        json.dumps(failures, ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
    )

    by_tool_elapsed: dict[str, list[float]] = defaultdict(list)
    by_tool_rss: dict[str, list[float]] = defaultdict(list)
    module_decode: list[float] = []
    module_quantize: list[float] = []
    module_encode: list[float] = []
    module_total: list[float] = []
    by_sample_tool_elapsed: dict[tuple[str, str], list[float]] = defaultdict(list)

    for row in rows:
        if row["status"] != "success":
            continue
        tool = row["tool"]
        elapsed = float(row["elapsed_ms"])
        by_tool_elapsed[tool].append(elapsed)
        by_sample_tool_elapsed[(row["sample_id"], tool)].append(elapsed)
        if row["max_rss_kb"] != "":
            by_tool_rss[tool].append(float(row["max_rss_kb"]))
        if tool == "candidate" and row["decode_ms"] != "":
            module_decode.append(float(row["decode_ms"]))
            module_quantize.append(float(row["quantize_ms"]))
            module_encode.append(float(row["encode_ms"]))
            module_total.append(float(row["total_ms"]))

    sample_speedups = []
    for sample in samples:
        sid = sample["sample_id"]
        c_vals = by_sample_tool_elapsed.get((sid, "candidate"), [])
        b_vals = by_sample_tool_elapsed.get((sid, "baseline"), [])
        if c_vals and b_vals:
            c_mean = statistics.mean(c_vals)
            b_mean = statistics.mean(b_vals)
            if c_mean > 0:
                sample_speedups.append(b_mean / c_mean)

    aggregate_rows = []
    for tool in ("baseline", "candidate"):
        elapsed_vals = by_tool_elapsed.get(tool, [])
        rss_vals = by_tool_rss.get(tool, [])
        aggregate_rows.append(
            {
                "run_id": args.run_id,
                "tool": tool,
                "samples": len(elapsed_vals),
                "elapsed_ms_mean": f"{mean_or_zero(elapsed_vals):.3f}",
                "elapsed_ms_median": f"{median_or_zero(elapsed_vals):.3f}",
                "elapsed_ms_p95": f"{percentile(elapsed_vals, 0.95):.3f}",
                "rss_kb_mean": f"{mean_or_zero(rss_vals):.1f}" if rss_vals else "",
                "rss_kb_median": f"{median_or_zero(rss_vals):.1f}" if rss_vals else "",
                "rss_kb_p95": f"{percentile(rss_vals, 0.95):.1f}" if rss_vals else "",
                "rss_kb_peak": f"{max(rss_vals):.1f}" if rss_vals else "",
            }
        )

    with (run_dir / "perf_aggregate.csv").open("w", newline="", encoding="utf-8") as f:
        fields = list(aggregate_rows[0].keys()) if aggregate_rows else []
        writer = csv.DictWriter(f, fieldnames=fields)
        writer.writeheader()
        writer.writerows(aggregate_rows)

    memory_profile = {
        "run_id": args.run_id,
        "memory_unit": "KB",
        "tools": {
            tool: {
                "samples": len(by_tool_rss.get(tool, [])),
                "rss_kb_mean": mean_or_zero(by_tool_rss.get(tool, [])),
                "rss_kb_median": median_or_zero(by_tool_rss.get(tool, [])),
                "rss_kb_p95": percentile(by_tool_rss.get(tool, []), 0.95),
                "rss_kb_peak": max(by_tool_rss.get(tool, [0.0])),
            }
            for tool in ("baseline", "candidate")
        },
        "candidate_module_ms": {
            "decode_mean": mean_or_zero(module_decode),
            "quantize_mean": mean_or_zero(module_quantize),
            "encode_mean": mean_or_zero(module_encode),
            "total_mean": mean_or_zero(module_total),
        },
    }
    (run_dir / "memory_profile.json").write_text(
        json.dumps(memory_profile, ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
    )

    baseline_mean = mean_or_zero(by_tool_elapsed.get("baseline", []))
    candidate_mean = mean_or_zero(by_tool_elapsed.get("candidate", []))
    speedup = (baseline_mean / candidate_mean) if candidate_mean > 0 else 0.0

    summary = [
        "# Perf Report v1",
        "",
        f"- run_id: `{args.run_id}`",
        f"- samples_total: {len(samples)}",
        f"- iterations_per_sample: {args.iterations}",
        f"- failures: {len(failures)}",
        f"- baseline_elapsed_ms_mean: {baseline_mean:.3f}",
        f"- candidate_elapsed_ms_mean: {candidate_mean:.3f}",
        f"- speedup_baseline_div_candidate: {speedup:.3f}",
        f"- speedup_p95_baseline_div_candidate: {percentile(sample_speedups, 0.95):.3f}"
        if sample_speedups
        else "- speedup_p95_baseline_div_candidate: n/a",
        f"- baseline_rss_kb_peak: {max(by_tool_rss.get('baseline', [0.0])):.1f}",
        f"- candidate_rss_kb_peak: {max(by_tool_rss.get('candidate', [0.0])):.1f}",
        "",
        "Candidate Module Means (ms):",
        f"- decode: {mean_or_zero(module_decode):.3f}",
        f"- quantize: {mean_or_zero(module_quantize):.3f}",
        f"- encode: {mean_or_zero(module_encode):.3f}",
        f"- total: {mean_or_zero(module_total):.3f}",
        "",
        "Artifacts:",
        f"- `reports/perf/{args.run_id}/perf_compare.csv`",
        f"- `reports/perf/{args.run_id}/perf_aggregate.csv`",
        f"- `reports/perf/{args.run_id}/memory_profile.json`",
        f"- `reports/perf/{args.run_id}/failures.json`",
    ]
    (run_dir / "summary.md").write_text("\n".join(summary) + "\n", encoding="utf-8")

    print(f"Phase-E perf run complete: {run_dir}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
