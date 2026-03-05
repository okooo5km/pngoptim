#!/usr/bin/env python3
"""Phase-F cross-platform consistency runner (collect + aggregate)."""

from __future__ import annotations

import argparse
import csv
import hashlib
import json
import math
import platform
import statistics
import subprocess
import sys
import time
from datetime import datetime
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
DATASET = ROOT / "dataset"
REPORTS = ROOT / "reports" / "cross_platform"
SPLITS = ("functional", "quality", "perf")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Run Phase-F cross-platform checks.")
    parser.add_argument(
        "--mode",
        choices=("collect", "aggregate"),
        default="collect",
        help="collect: run local checks and emit platform json; aggregate: combine jsons.",
    )
    parser.add_argument(
        "--run-id",
        default=f"cross-platform-v1-{datetime.now().strftime('%Y%m%d-%H%M%S')}",
        help="Cross-platform run id.",
    )
    parser.add_argument(
        "--platform-label",
        default=f"{platform.system().lower()}-{platform.machine().lower()}",
        help="Platform label written into platform metrics file.",
    )
    parser.add_argument(
        "--binary",
        default=str(ROOT / "target" / "release" / "pngoptim"),
        help="Path to pngoptim binary.",
    )
    parser.add_argument("--build", action="store_true", help="Build release binary before collect.")
    parser.add_argument(
        "--allow-partial",
        action="store_true",
        help="Allow aggregate pass with <3 platform reports.",
    )
    parser.add_argument(
        "--timeout-sec",
        type=float,
        default=12.0,
        help="Timeout per sample for collect mode.",
    )
    return parser.parse_args()


def resolve_binary_path(raw_path: str) -> Path:
    path = Path(raw_path)
    if path.exists():
        return path
    if platform.system() == "Windows" and path.suffix.lower() != ".exe":
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
        for i, entry in enumerate(entries):
            if not bool(entry.get("expected_success", True)):
                continue
            samples.append(
                {
                    "split": split,
                    "sample_id": entry.get("id", f"{split}-{i+1:03d}"),
                    "filename": entry["filename"],
                }
            )
    return samples


def sha256_file(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def parse_md_keyvals(path: Path) -> dict[str, str]:
    out: dict[str, str] = {}
    if not path.exists():
        return out
    for line in path.read_text(encoding="utf-8").splitlines():
        line = line.strip()
        if not line.startswith("- "):
            continue
        body = line[2:]
        if ":" not in body:
            continue
        k, v = body.split(":", 1)
        out[k.strip()] = v.strip().strip("`")
    return out


def run_cmd(cmd: list[str], timeout_sec: float = 0.0) -> tuple[int, str, str]:
    try:
        proc = subprocess.run(
            cmd,
            cwd=ROOT,
            capture_output=True,
            text=True,
            timeout=timeout_sec if timeout_sec > 0 else None,
        )
        return proc.returncode, proc.stdout or "", proc.stderr or ""
    except subprocess.TimeoutExpired as exc:
        return 124, exc.stdout or "", (exc.stderr or "") + "\n<timeout>"


def collect(args: argparse.Namespace) -> int:
    if args.build:
        subprocess.run(["cargo", "build", "--release"], cwd=ROOT, check=True)

    binary = resolve_binary_path(args.binary)
    if not binary.exists():
        print(f"binary not found: {binary}", file=sys.stderr)
        return 2

    run_dir = REPORTS / args.run_id
    platform_dir = run_dir / "platform"
    out_dir = run_dir / "out" / args.platform_label
    platform_dir.mkdir(parents=True, exist_ok=True)
    out_dir.mkdir(parents=True, exist_ok=True)

    # 1) Candidate-only consistency eval on standard samples.
    rows = []
    failures = []
    samples = load_samples()
    for sample in samples:
        split = sample["split"]
        sample_id = sample["sample_id"]
        input_path = DATASET / split / sample["filename"]
        output_path = out_dir / split / f"{Path(sample['filename']).stem}.cp.png"
        output_path.parent.mkdir(parents=True, exist_ok=True)
        if output_path.exists():
            output_path.unlink()

        cmd = [
            str(binary),
            str(input_path),
            "--quality",
            "55-75",
            "--speed",
            "4",
            "--strip",
            "--force",
            "--quiet",
            "--output",
            str(output_path),
        ]
        start = time.perf_counter()
        code, _stdout, stderr = run_cmd(cmd, timeout_sec=args.timeout_sec)
        elapsed_ms = (time.perf_counter() - start) * 1000.0
        success = code == 0 and output_path.exists()
        input_bytes = input_path.stat().st_size
        output_bytes = output_path.stat().st_size if success else None
        size_ratio = (output_bytes / input_bytes) if output_bytes is not None else None
        out_sha = sha256_file(output_path) if success else ""

        rows.append(
            {
                "sample_id": sample_id,
                "split": split,
                "input_file": sample["filename"],
                "exit_code": code,
                "elapsed_ms": elapsed_ms,
                "input_bytes": input_bytes,
                "output_bytes": output_bytes,
                "size_ratio": size_ratio,
                "output_sha256": out_sha,
                "stderr": stderr[:300],
            }
        )
        if not success:
            failures.append(
                {
                    "stage": "candidate_eval",
                    "sample_id": sample_id,
                    "input_file": sample["filename"],
                    "exit_code": code,
                    "stderr": stderr[:500],
                }
            )

    # 2) Reuse existing guards on this platform.
    smoke_id = f"smoke-{args.run_id}-{args.platform_label}"
    compat_id = f"compat-{args.run_id}-{args.platform_label}"
    stability_id = f"stability-{args.run_id}-{args.platform_label}"

    smoke_cmd = [
        sys.executable,
        str(ROOT / "scripts" / "smoke" / "run_smoke_phase_b.py"),
        "--run-id",
        smoke_id,
        "--binary",
        str(binary),
    ]
    compat_cmd = [
        sys.executable,
        str(ROOT / "scripts" / "compat" / "run_phase_c_compat.py"),
        "--run-id",
        compat_id,
        "--binary",
        str(binary),
    ]
    stability_cmd = [
        sys.executable,
        str(ROOT / "scripts" / "stability" / "run_phase_f_stability.py"),
        "--run-id",
        stability_id,
        "--binary",
        str(binary),
        "--fuzz-cases",
        "24",
        "--timeout-sec",
        "6",
    ]

    smoke_code, _sout, smoke_err = run_cmd(smoke_cmd)
    compat_code, _cout, compat_err = run_cmd(compat_cmd)
    stability_code, _tout, stability_err = run_cmd(stability_cmd)

    if smoke_code != 0:
        failures.append({"stage": "smoke", "exit_code": smoke_code, "stderr": smoke_err[:500]})
    if compat_code != 0:
        failures.append({"stage": "compat", "exit_code": compat_code, "stderr": compat_err[:500]})
    if stability_code != 0:
        failures.append(
            {"stage": "stability", "exit_code": stability_code, "stderr": stability_err[:500]}
        )

    smoke_meta = parse_md_keyvals(ROOT / "reports" / "smoke" / smoke_id / "summary.md")
    compat_exit_json = ROOT / "reports" / "compat" / compat_id / "exit_codes.json"
    compat_io_json = ROOT / "reports" / "compat" / compat_id / "io_behavior.json"
    stability_fuzz_json = ROOT / "reports" / "stability" / stability_id / "fuzz_summary.json"

    compat_exit = (
        json.loads(compat_exit_json.read_text(encoding="utf-8")) if compat_exit_json.exists() else {}
    )
    compat_io = json.loads(compat_io_json.read_text(encoding="utf-8")) if compat_io_json.exists() else {}
    stability_fuzz = (
        json.loads(stability_fuzz_json.read_text(encoding="utf-8"))
        if stability_fuzz_json.exists()
        else {}
    )

    compat_checks = compat_exit.get("checks", {})
    compat_exit_ok = all(v.get("passed") for v in compat_checks.values()) if compat_checks else False
    io_items = [v for v in compat_io.values() if isinstance(v, dict) and "passed" in v]
    compat_io_ok = all(v.get("passed") for v in io_items) if io_items else False

    success_rows = [r for r in rows if r["output_bytes"] is not None]
    size_ratios = [float(r["size_ratio"]) for r in success_rows]
    elapsed_vals = [float(r["elapsed_ms"]) for r in success_rows]

    platform_metrics = {
        "run_id": args.run_id,
        "platform_label": args.platform_label,
        "system": platform.system(),
        "release": platform.release(),
        "machine": platform.machine(),
        "python_version": platform.python_version(),
        "sample_count": len(rows),
        "success_count": len(success_rows),
        "failure_count": len(rows) - len(success_rows),
        "size_ratio_mean": statistics.mean(size_ratios) if size_ratios else 0.0,
        "size_ratio_median": statistics.median(size_ratios) if size_ratios else 0.0,
        "size_ratio_p95": sorted(size_ratios)[max(0, math.ceil(len(size_ratios) * 0.95) - 1)]
        if size_ratios
        else 0.0,
        "elapsed_ms_mean": statistics.mean(elapsed_vals) if elapsed_vals else 0.0,
        "elapsed_ms_median": statistics.median(elapsed_vals) if elapsed_vals else 0.0,
        "elapsed_ms_p95": sorted(elapsed_vals)[max(0, math.ceil(len(elapsed_vals) * 0.95) - 1)]
        if elapsed_vals
        else 0.0,
        "smoke_passed": smoke_meta.get("failed", "1") == "0",
        "compat_exit_passed": compat_exit_ok,
        "compat_io_passed": compat_io_ok,
        "stability_crash_like_count": int(stability_fuzz.get("crash_like_count", -1)),
        "stability_failures_count": int(stability_fuzz.get("failures_count", -1)),
        "scripts": {
            "smoke_run_id": smoke_id,
            "compat_run_id": compat_id,
            "stability_run_id": stability_id,
        },
        "samples": {
            r["sample_id"]: {
                "output_bytes": r["output_bytes"],
                "size_ratio": r["size_ratio"],
                "output_sha256": r["output_sha256"],
                "exit_code": r["exit_code"],
            }
            for r in rows
        },
        "collect_failures": failures,
    }

    (platform_dir / f"{args.platform_label}.json").write_text(
        json.dumps(platform_metrics, ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
    )

    with (run_dir / f"collect_{args.platform_label}.csv").open("w", newline="", encoding="utf-8") as f:
        fields = [
            "sample_id",
            "split",
            "input_file",
            "exit_code",
            "elapsed_ms",
            "input_bytes",
            "output_bytes",
            "size_ratio",
            "output_sha256",
        ]
        writer = csv.DictWriter(f, fieldnames=fields)
        writer.writeheader()
        for r in rows:
            writer.writerow({k: r[k] for k in fields})

    print(f"Cross-platform collect complete: {run_dir / 'platform' / (args.platform_label + '.json')}")
    return 0 if not failures else 1


def aggregate(args: argparse.Namespace) -> int:
    run_dir = REPORTS / args.run_id
    platform_dir = run_dir / "platform"
    if not platform_dir.exists():
        print(f"platform directory not found: {platform_dir}", file=sys.stderr)
        return 2

    files = sorted(platform_dir.glob("*.json"))
    if not files:
        print("no platform json found for aggregation", file=sys.stderr)
        return 2

    data = [json.loads(p.read_text(encoding="utf-8")) for p in files]
    labels = [d["platform_label"] for d in data]
    platform_count = len(data)

    def spread(metric: str) -> tuple[float, float, float]:
        vals = [float(d.get(metric, 0.0)) for d in data]
        return min(vals), max(vals), max(vals) - min(vals)

    checks = []
    checks.append(
        {
            "metric": "platform_count",
            "min": platform_count,
            "max": platform_count,
            "spread": 0.0,
            "threshold": 3,
            "passed": platform_count >= 3 or args.allow_partial,
        }
    )

    for metric, threshold in [
        ("size_ratio_mean", 1e-6),
        ("size_ratio_median", 1e-6),
        ("size_ratio_p95", 1e-6),
    ]:
        mn, mx, sp = spread(metric)
        checks.append(
            {
                "metric": metric,
                "min": mn,
                "max": mx,
                "spread": sp,
                "threshold": threshold,
                "passed": sp <= threshold,
            }
        )

    smoke_ok = all(bool(d.get("smoke_passed")) for d in data)
    compat_ok = all(bool(d.get("compat_exit_passed")) and bool(d.get("compat_io_passed")) for d in data)
    stability_ok = all(
        int(d.get("stability_crash_like_count", -1)) == 0
        and int(d.get("stability_failures_count", -1)) == 0
        for d in data
    )
    checks.extend(
        [
            {
                "metric": "smoke_passed_all_platforms",
                "min": 1.0 if smoke_ok else 0.0,
                "max": 1.0 if smoke_ok else 0.0,
                "spread": 0.0,
                "threshold": 1.0,
                "passed": smoke_ok,
            },
            {
                "metric": "compat_passed_all_platforms",
                "min": 1.0 if compat_ok else 0.0,
                "max": 1.0 if compat_ok else 0.0,
                "spread": 0.0,
                "threshold": 1.0,
                "passed": compat_ok,
            },
            {
                "metric": "stability_passed_all_platforms",
                "min": 1.0 if stability_ok else 0.0,
                "max": 1.0 if stability_ok else 0.0,
                "spread": 0.0,
                "threshold": 1.0,
                "passed": stability_ok,
            },
        ]
    )

    # Per-sample byte-level consistency
    sample_ids = sorted(set().union(*(d.get("samples", {}).keys() for d in data)))
    inconsistent_samples = []
    for sid in sample_ids:
        vals = [d.get("samples", {}).get(sid, {}).get("output_bytes") for d in data]
        vals = [v for v in vals if isinstance(v, int)]
        if len(vals) != platform_count:
            inconsistent_samples.append({"sample_id": sid, "reason": "missing_output"})
            continue
        if len(set(vals)) != 1:
            inconsistent_samples.append({"sample_id": sid, "reason": "bytes_mismatch", "values": vals})

    checks.append(
        {
            "metric": "sample_output_bytes_consistent",
            "min": float(len(inconsistent_samples)),
            "max": float(len(inconsistent_samples)),
            "spread": 0.0,
            "threshold": 0.0,
            "passed": len(inconsistent_samples) == 0,
        }
    )

    with (run_dir / "consistency.csv").open("w", newline="", encoding="utf-8") as f:
        fields = ["metric", "min", "max", "spread", "threshold", "passed"]
        writer = csv.DictWriter(f, fieldnames=fields)
        writer.writeheader()
        writer.writerows(
            {
                "metric": c["metric"],
                "min": c["min"],
                "max": c["max"],
                "spread": c["spread"],
                "threshold": c["threshold"],
                "passed": str(c["passed"]).lower(),
            }
            for c in checks
        )

    (run_dir / "inconsistent_samples.json").write_text(
        json.dumps(inconsistent_samples, ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
    )

    passed = all(c["passed"] for c in checks)
    failed_checks = [c for c in checks if not c["passed"]]
    summary = [
        "# Cross-platform Report v1",
        "",
        f"- run_id: `{args.run_id}`",
        f"- platforms: {platform_count}",
        f"- platform_labels: `{', '.join(labels)}`",
        f"- allow_partial: {str(args.allow_partial).lower()}",
        f"- inconsistent_samples: {len(inconsistent_samples)}",
        f"- status: {'pass' if passed else 'fail'}",
        "",
        "Artifacts:",
        f"- `reports/cross_platform/{args.run_id}/consistency.csv`",
        f"- `reports/cross_platform/{args.run_id}/inconsistent_samples.json`",
    ]
    if failed_checks:
        summary.append("")
        summary.append("Failed Checks:")
        for c in failed_checks:
            summary.append(
                f"- {c['metric']}: min={c['min']}, max={c['max']}, spread={c['spread']}, threshold={c['threshold']}"
            )
    (run_dir / "summary.md").write_text("\n".join(summary) + "\n", encoding="utf-8")

    print(f"Cross-platform aggregate complete: {run_dir / 'summary.md'}")
    if failed_checks:
        for c in failed_checks:
            print(
                "FAILED_CHECK\t"
                f"{c['metric']}\tmin={c['min']}\tmax={c['max']}\tspread={c['spread']}\tthreshold={c['threshold']}",
                file=sys.stderr,
            )
    return 0 if passed else 1


def main() -> int:
    args = parse_args()
    if args.mode == "collect":
        return collect(args)
    return aggregate(args)


if __name__ == "__main__":
    raise SystemExit(main())
