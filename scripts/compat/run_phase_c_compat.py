#!/usr/bin/env python3
"""Run Phase-C compatibility checks and emit structured reports."""

from __future__ import annotations

import argparse
import json
import shutil
import subprocess
from pathlib import Path

from PIL import Image
from PIL.PngImagePlugin import PngInfo

ROOT = Path(__file__).resolve().parents[2]
DATASET = ROOT / "dataset"
REPORTS = ROOT / "reports" / "compat"


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Run phase C compatibility checks.")
    parser.add_argument("--run-id", default="compat-v1-20260305", help="Compatibility run id.")
    parser.add_argument(
        "--binary",
        default=str(ROOT / "target" / "debug" / "pngoptim"),
        help="Path to pngoptim binary.",
    )
    parser.add_argument("--build", action="store_true", help="Build binary before running checks.")
    return parser.parse_args()


def run(cmd: list[str], *, stdin_bytes: bytes | None = None) -> dict:
    proc = subprocess.run(
        cmd,
        cwd=ROOT,
        input=stdin_bytes,
        capture_output=True,
    )
    return {
        "exit_code": proc.returncode,
        "stdout_bytes": proc.stdout,
        "stdout": proc.stdout.decode("utf-8", errors="replace"),
        "stderr": proc.stderr.decode("utf-8", errors="replace"),
    }


def main() -> int:
    args = parse_args()
    if args.build:
        subprocess.run(["cargo", "build"], cwd=ROOT, check=True)

    binary = Path(args.binary)
    if not binary.exists():
        print(f"binary not found: {binary}")
        return 2

    run_dir = REPORTS / args.run_id
    tmp_dir = run_dir / "tmp"
    if run_dir.exists():
        shutil.rmtree(run_dir)
    tmp_dir.mkdir(parents=True, exist_ok=True)

    sample_func = DATASET / "functional" / "pngquant_test.png"
    sample_meta = DATASET / "functional" / "pngquant_metadata.png"
    sample_perf = DATASET / "perf" / "p_large_alpha_pattern.png"

    # Deterministic sample that is usually already tiny, useful to verify --skip-if-larger.
    tiny_skip_case = tmp_dir / "tiny_skip_case.png"
    tiny_img = Image.new("1", (64, 64), 0)
    tiny_img.save(tiny_skip_case, optimize=True)

    # 1) Parameter coverage checks
    arg_checks: dict[str, dict] = {}

    out_quality = tmp_dir / "arg_quality.png"
    arg_checks["quality"] = run(
        [str(binary), str(sample_func), "--output", str(out_quality), "--quality", "60-85", "--force"]
    )
    arg_checks["speed"] = run(
        [str(binary), str(sample_func), "--output", str(tmp_dir / "arg_speed.png"), "--speed", "8", "--force"]
    )
    arg_checks["dither_nofs"] = run(
        [str(binary), str(sample_func), "--output", str(tmp_dir / "arg_nofs.png"), "--nofs", "--force"]
    )
    arg_checks["output"] = run([str(binary), str(sample_func), "--output", str(tmp_dir / "arg_output.png"), "--force"])
    arg_checks["ext"] = run(
        [
            str(binary),
            str(sample_func),
            str(sample_meta),
            "--ext=.compat.png",
            "--force",
            "--quiet",
        ]
    )
    arg_checks["strip"] = run(
        [str(binary), str(sample_func), "--output", str(tmp_dir / "arg_strip.png"), "--strip", "--force"]
    )
    arg_checks["skip_if_larger"] = run(
        [
            str(binary),
            str(tiny_skip_case),
            "--output",
            str(tmp_dir / "arg_skip.png"),
            "--skip-if-larger",
            "--force",
        ]
    )
    arg_checks["posterize"] = run(
        [str(binary), str(sample_func), "--output", str(tmp_dir / "arg_post.png"), "--posterize", "4", "--force"]
    )
    arg_checks["floyd"] = run(
        [str(binary), str(sample_func), "--output", str(tmp_dir / "arg_floyd.png"), "--floyd", "--force"]
    )

    # Cleanup ext outputs created next to dataset files.
    for p in [
        DATASET / "functional" / "pngquant_test.compat.png",
        DATASET / "functional" / "pngquant_metadata.compat.png",
    ]:
        if p.exists():
            p.unlink()

    args_status = {}
    for name, result in arg_checks.items():
        # skip-if-larger is expected to exit 99 on this sample by design.
        if name == "skip_if_larger":
            supported = result["exit_code"] == 99
        else:
            supported = result["exit_code"] == 0
        args_status[name] = {
            "supported": supported,
            "exit_code": result["exit_code"],
        }

    total_args = len(args_status)
    covered_args = sum(1 for v in args_status.values() if v["supported"])
    args_coverage = {
        "run_id": args.run_id,
        "supported": args_status,
        "coverage_percent": round((covered_args / total_args) * 100.0, 2),
        "total": total_args,
        "covered": covered_args,
    }

    # 2) Exit code checks
    exit_checks = {
        "success": run([str(binary), str(sample_func), "--output", str(tmp_dir / "exit_success.png"), "--force"]),
        "param_error": run([str(binary), "no-such-input.png"]),
        "quality_too_low": run(
            [
                str(binary),
                str(sample_func),
                "--output",
                str(tmp_dir / "exit_quality.png"),
                "--quality",
                "99-100",
                "--posterize",
                "8",
                "--force",
            ]
        ),
        "size_not_reduced": run(
            [
                str(binary),
                str(tiny_skip_case),
                "--output",
                str(tmp_dir / "exit_size.png"),
                "--skip-if-larger",
                "--force",
            ]
        ),
        "io_failure": run([str(binary), str(sample_func), "--output", "/dev/null/out.png", "--force"]),
    }

    expected_exit = {
        "success": 0,
        "param_error": 2,
        "quality_too_low": 98,
        "size_not_reduced": 99,
        "io_failure": 3,
    }
    exit_report = {
        "run_id": args.run_id,
        "checks": {
            name: {
                "expected": expected_exit[name],
                "actual": result["exit_code"],
                "passed": result["exit_code"] == expected_exit[name],
            }
            for name, result in exit_checks.items()
        },
    }

    # 3) I/O behavior checks
    file_output = tmp_dir / "io_file.png"
    file_res = run([str(binary), str(sample_func), "--output", str(file_output), "--force"])

    stdin_bytes = sample_func.read_bytes()
    stdio_res = run([str(binary), "-", "--output", "-"], stdin_bytes=stdin_bytes)
    png_sig_ok = stdio_res["stdout_bytes"].startswith(b"\x89PNG\r\n\x1a\n")

    batch_res = run(
        [
            str(binary),
            str(sample_func),
            str(sample_meta),
            "--ext=.batch.png",
            "--force",
            "--quiet",
        ]
    )
    batch_a = DATASET / "functional" / "pngquant_test.batch.png"
    batch_b = DATASET / "functional" / "pngquant_metadata.batch.png"
    batch_outputs_ok = batch_a.exists() and batch_b.exists()
    if batch_a.exists():
        batch_a.unlink()
    if batch_b.exists():
        batch_b.unlink()

    overwrite_path = tmp_dir / "io_overwrite.png"
    _ = run([str(binary), str(sample_func), "--output", str(overwrite_path), "--force"])
    overwrite_res = run([str(binary), str(sample_func), "--output", str(overwrite_path)])

    # Metadata preserve vs strip behavior
    meta_input = tmp_dir / "meta_input.png"
    meta_preserve = tmp_dir / "meta_preserve.png"
    meta_strip = tmp_dir / "meta_strip.png"
    img = Image.new("RGBA", (16, 16), (12, 34, 56, 255))
    pnginfo = PngInfo()
    pnginfo.add_text("Comment", "phase-c-metadata-preserve")
    img.save(meta_input, pnginfo=pnginfo)

    meta_keep_res = run([str(binary), str(meta_input), "--output", str(meta_preserve), "--force"])
    meta_strip_res = run(
        [str(binary), str(meta_input), "--output", str(meta_strip), "--strip", "--force"]
    )
    keep_comment = Image.open(meta_preserve).info.get("Comment") if meta_preserve.exists() else None
    strip_comment = Image.open(meta_strip).info.get("Comment") if meta_strip.exists() else None
    metadata_passed = (
        meta_keep_res["exit_code"] == 0
        and meta_strip_res["exit_code"] == 0
        and keep_comment == "phase-c-metadata-preserve"
        and strip_comment is None
    )

    io_behavior = {
        "run_id": args.run_id,
        "file_input_output": {
            "passed": file_res["exit_code"] == 0 and file_output.exists(),
            "exit_code": file_res["exit_code"],
        },
        "stdin_stdout": {
            "passed": stdio_res["exit_code"] == 0 and png_sig_ok,
            "exit_code": stdio_res["exit_code"],
            "png_signature_ok": png_sig_ok,
        },
        "batch_with_ext": {
            "passed": batch_res["exit_code"] == 0 and batch_outputs_ok,
            "exit_code": batch_res["exit_code"],
        },
        "overwrite_strategy": {
            "passed": overwrite_res["exit_code"] == 2,
            "exit_code": overwrite_res["exit_code"],
        },
        "metadata_strategy": {
            "passed": metadata_passed,
            "strip_flag_supported": args_status["strip"]["supported"],
            "preserve_comment": keep_comment,
            "strip_comment": strip_comment,
        },
    }

    # Persist reports
    (run_dir / "args_coverage.json").write_text(
        json.dumps(args_coverage, ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
    )
    (run_dir / "exit_codes.json").write_text(
        json.dumps(exit_report, ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
    )
    (run_dir / "io_behavior.json").write_text(
        json.dumps(io_behavior, ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
    )

    summary = [
        "# Compatibility Report v1",
        "",
        f"- run_id: `{args.run_id}`",
        f"- args_coverage: {args_coverage['coverage_percent']}% ({covered_args}/{total_args})",
        "- exit_codes: "
        + ", ".join(
            f"{name}={'ok' if check['passed'] else 'fail'}"
            for name, check in exit_report["checks"].items()
        ),
        "- io_behavior: "
        + ", ".join(
            f"{name}={'ok' if item['passed'] else 'fail'}"
            for name, item in io_behavior.items()
            if isinstance(item, dict) and "passed" in item
        ),
        "",
        "Artifacts:",
        f"- `reports/compat/{args.run_id}/args_coverage.json`",
        f"- `reports/compat/{args.run_id}/exit_codes.json`",
        f"- `reports/compat/{args.run_id}/io_behavior.json`",
    ]
    (run_dir / "summary.md").write_text("\n".join(summary) + "\n", encoding="utf-8")
    print(f"Compatibility run complete: {run_dir}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
