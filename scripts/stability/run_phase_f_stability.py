#!/usr/bin/env python3
"""Run Phase-F stability checks (regression + mutational fuzz)."""

from __future__ import annotations

import argparse
import csv
import json
import os
import random
import subprocess
import sys
import time
from dataclasses import dataclass
from datetime import datetime
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
DATASET = ROOT / "dataset"
REPORTS = ROOT / "reports" / "stability"
VALID_SPLITS = ("functional", "quality", "perf")
ROBUSTNESS_SPLIT = "robustness"


@dataclass
class CaseResult:
    case_type: str
    case_id: str
    input_file: str
    expected_success: bool
    exit_code: int
    elapsed_ms: float
    timed_out: bool
    panicked: bool
    signaled: bool
    success: bool
    stderr: str


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Run phase-F stability checks.")
    parser.add_argument(
        "--run-id",
        default=f"stability-v1-{datetime.now().strftime('%Y%m%d-%H%M%S')}",
        help="Unique run id.",
    )
    parser.add_argument(
        "--binary",
        default=str(ROOT / "target" / "release" / "pngoptim"),
        help="Path to pngoptim binary.",
    )
    parser.add_argument("--build", action="store_true", help="Build release binary first.")
    parser.add_argument(
        "--timeout-sec",
        type=float,
        default=8.0,
        help="Timeout per case in seconds.",
    )
    parser.add_argument(
        "--fuzz-cases",
        type=int,
        default=32,
        help="Number of mutational fuzz cases to generate.",
    )
    parser.add_argument(
        "--seed",
        type=int,
        default=20260305,
        help="Deterministic RNG seed for fuzz mutations.",
    )
    return parser.parse_args()


def resolve_binary_path(raw_path: str) -> Path:
    path = Path(raw_path)
    if path.exists():
        return path
    if sys.platform.startswith("win") and path.suffix.lower() != ".exe":
        exe_path = path.with_name(path.name + ".exe")
        if exe_path.exists():
            return exe_path
    return path


def load_manifest_samples(split: str) -> list[dict]:
    manifest = DATASET / split / "manifest.json"
    if not manifest.exists():
        return []
    entries = json.loads(manifest.read_text(encoding="utf-8"))
    out = []
    for i, entry in enumerate(entries):
        out.append(
            {
                "id": entry.get("id", f"{split}-{i+1:03d}"),
                "filename": entry["filename"],
                "expected_success": bool(
                    entry.get("expected_success", split != ROBUSTNESS_SPLIT)
                ),
            }
        )
    return out


def run_case(
    binary: Path,
    input_path: Path,
    output_path: Path,
    timeout_sec: float,
) -> CaseResult:
    cmd = [
        str(binary),
        str(input_path),
        "--quality",
        "55-75",
        "--speed",
        "4",
        "--force",
        "--output",
        str(output_path),
        "--quiet",
    ]
    start = time.perf_counter()
    try:
        proc = subprocess.run(
            cmd,
            capture_output=True,
            text=True,
            timeout=timeout_sec,
        )
        timed_out = False
        exit_code = proc.returncode
        stderr = (proc.stderr or "").strip()
    except subprocess.TimeoutExpired as exc:
        elapsed_ms = (time.perf_counter() - start) * 1000.0
        return CaseResult(
            case_type="",
            case_id="",
            input_file=input_path.name,
            expected_success=False,
            exit_code=124,
            elapsed_ms=elapsed_ms,
            timed_out=True,
            panicked=False,
            signaled=False,
            success=False,
            stderr=(exc.stderr or "")[:500] if exc.stderr else "timeout",
        )

    elapsed_ms = (time.perf_counter() - start) * 1000.0
    signaled = exit_code < 0
    panicked = ("thread 'main' panicked" in stderr) or ("panic" in stderr.lower())
    success = exit_code == 0 and output_path.exists()
    return CaseResult(
        case_type="",
        case_id="",
        input_file=input_path.name,
        expected_success=False,
        exit_code=exit_code,
        elapsed_ms=elapsed_ms,
        timed_out=timed_out,
        panicked=panicked,
        signaled=signaled,
        success=success,
        stderr=stderr[:500],
    )


def mutate_bytes(src: bytes, rng: random.Random) -> bytes:
    if not src:
        return b"\x89PNG\r\n\x1a\n"
    mode = rng.randint(0, 4)
    data = bytearray(src)

    if mode == 0:
        # Truncate to random size (including very small file).
        n = rng.randint(0, max(1, len(data) - 1))
        return bytes(data[:n])
    if mode == 1:
        # Flip random bits in random positions.
        flips = max(1, len(data) // 256)
        for _ in range(flips):
            idx = rng.randrange(len(data))
            bit = 1 << rng.randrange(8)
            data[idx] ^= bit
        return bytes(data)
    if mode == 2:
        # Overwrite a contiguous block with random bytes.
        start = rng.randrange(len(data))
        block = rng.randint(1, min(128, len(data) - start))
        for i in range(start, start + block):
            data[i] = rng.randrange(256)
        return bytes(data)
    if mode == 3:
        # Duplicate a random slice (size expansion).
        start = rng.randrange(len(data))
        block = rng.randint(1, min(256, len(data) - start))
        insert_at = rng.randrange(len(data))
        return bytes(data[:insert_at] + data[start : start + block] + data[insert_at:])

    # Append arbitrary noise.
    noise = bytes(rng.randrange(256) for _ in range(rng.randint(1, 256)))
    return bytes(data) + noise


def main() -> int:
    args = parse_args()
    if args.build:
        subprocess.run(["cargo", "build", "--release"], cwd=ROOT, check=True)

    binary = resolve_binary_path(args.binary)
    if not binary.exists():
        print(f"binary not found: {binary}", file=sys.stderr)
        return 2

    run_dir = REPORTS / args.run_id
    if run_dir.exists():
        subprocess.run(["rm", "-rf", str(run_dir)], check=True)
    run_dir.mkdir(parents=True, exist_ok=True)

    out_dir = run_dir / "out"
    cases: list[CaseResult] = []
    failures: list[dict] = []

    # 1) Regression suite: valid inputs + robustness inputs.
    for split in (*VALID_SPLITS, ROBUSTNESS_SPLIT):
        for sample in load_manifest_samples(split):
            input_path = DATASET / split / sample["filename"]
            output_path = out_dir / "regression" / split / f"{Path(sample['filename']).stem}.png"
            output_path.parent.mkdir(parents=True, exist_ok=True)
            if output_path.exists():
                output_path.unlink()

            result = run_case(binary, input_path, output_path, timeout_sec=args.timeout_sec)
            result.case_type = "regression"
            result.case_id = sample["id"]
            result.expected_success = sample["expected_success"]
            result.input_file = sample["filename"]
            cases.append(result)

            # Stability gate: no crash/panic/timeout.
            unstable = result.timed_out or result.panicked or result.signaled
            behavior_ok = (result.success == sample["expected_success"]) or (
                split == ROBUSTNESS_SPLIT and not result.success
            )
            if unstable or not behavior_ok:
                failures.append(
                    {
                        "case_type": result.case_type,
                        "case_id": result.case_id,
                        "input_file": result.input_file,
                        "expected_success": result.expected_success,
                        "exit_code": result.exit_code,
                        "timed_out": result.timed_out,
                        "panicked": result.panicked,
                        "signaled": result.signaled,
                        "actual_success": result.success,
                        "stderr": result.stderr,
                    }
                )

    # 2) Mutational fuzz cases generated from valid PNGs.
    rng = random.Random(args.seed)
    valid_seed_files: list[Path] = []
    for split in VALID_SPLITS:
        for sample in load_manifest_samples(split):
            valid_seed_files.append(DATASET / split / sample["filename"])

    fuzz_input_dir = run_dir / "fuzz-inputs"
    fuzz_input_dir.mkdir(parents=True, exist_ok=True)
    for idx in range(args.fuzz_cases):
        seed_path = valid_seed_files[idx % len(valid_seed_files)]
        src_bytes = seed_path.read_bytes()
        mutated = mutate_bytes(src_bytes, rng)
        fuzz_name = f"fuzz-{idx+1:04d}.png"
        fuzz_path = fuzz_input_dir / fuzz_name
        fuzz_path.write_bytes(mutated)

        output_path = out_dir / "fuzz" / f"{Path(fuzz_name).stem}.out.png"
        output_path.parent.mkdir(parents=True, exist_ok=True)
        if output_path.exists():
            output_path.unlink()

        result = run_case(binary, fuzz_path, output_path, timeout_sec=args.timeout_sec)
        result.case_type = "fuzz"
        result.case_id = f"fuzz-{idx+1:04d}"
        result.expected_success = False
        result.input_file = fuzz_name
        cases.append(result)

        unstable = result.timed_out or result.panicked or result.signaled
        if unstable:
            failures.append(
                {
                    "case_type": result.case_type,
                    "case_id": result.case_id,
                    "input_file": result.input_file,
                    "expected_success": False,
                    "exit_code": result.exit_code,
                    "timed_out": result.timed_out,
                    "panicked": result.panicked,
                    "signaled": result.signaled,
                    "actual_success": result.success,
                    "stderr": result.stderr,
                }
            )

    # Emit reports.
    with (run_dir / "stability_report.csv").open("w", newline="", encoding="utf-8") as f:
        fields = [
            "run_id",
            "case_type",
            "case_id",
            "input_file",
            "expected_success",
            "exit_code",
            "elapsed_ms",
            "timed_out",
            "panicked",
            "signaled",
            "actual_success",
        ]
        writer = csv.DictWriter(f, fieldnames=fields)
        writer.writeheader()
        for case in cases:
            writer.writerow(
                {
                    "run_id": args.run_id,
                    "case_type": case.case_type,
                    "case_id": case.case_id,
                    "input_file": case.input_file,
                    "expected_success": str(case.expected_success).lower(),
                    "exit_code": case.exit_code,
                    "elapsed_ms": f"{case.elapsed_ms:.3f}",
                    "timed_out": str(case.timed_out).lower(),
                    "panicked": str(case.panicked).lower(),
                    "signaled": str(case.signaled).lower(),
                    "actual_success": str(case.success).lower(),
                }
            )

    (run_dir / "failures.json").write_text(
        json.dumps(failures, ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
    )

    regression_cases = [c for c in cases if c.case_type == "regression"]
    fuzz_cases = [c for c in cases if c.case_type == "fuzz"]
    crash_like = [c for c in cases if c.timed_out or c.panicked or c.signaled]

    fuzz_summary = {
        "run_id": args.run_id,
        "seed": args.seed,
        "total_cases": len(cases),
        "regression_cases": len(regression_cases),
        "fuzz_cases": len(fuzz_cases),
        "crash_like_count": len(crash_like),
        "timeout_count": sum(1 for c in cases if c.timed_out),
        "panic_count": sum(1 for c in cases if c.panicked),
        "signal_count": sum(1 for c in cases if c.signaled),
        "failures_count": len(failures),
        "artifacts": {
            "stability_report_csv": f"reports/stability/{args.run_id}/stability_report.csv",
            "failures_json": f"reports/stability/{args.run_id}/failures.json",
        },
    }
    (run_dir / "fuzz_summary.json").write_text(
        json.dumps(fuzz_summary, ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
    )

    summary = [
        "# Stability Report v1",
        "",
        f"- run_id: `{args.run_id}`",
        f"- total_cases: {len(cases)}",
        f"- regression_cases: {len(regression_cases)}",
        f"- fuzz_cases: {len(fuzz_cases)}",
        f"- crash_like_count: {len(crash_like)}",
        f"- failures: {len(failures)}",
        "",
        "Artifacts:",
        f"- `reports/stability/{args.run_id}/stability_report.csv`",
        f"- `reports/stability/{args.run_id}/fuzz_summary.json`",
        f"- `reports/stability/{args.run_id}/failures.json`",
    ]
    (run_dir / "summary.md").write_text("\n".join(summary) + "\n", encoding="utf-8")

    print(f"Phase-F stability run complete: {run_dir}")
    return 0 if not failures else 1


if __name__ == "__main__":
    raise SystemExit(main())
