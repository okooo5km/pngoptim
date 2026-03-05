#!/usr/bin/env python3
"""Validate release bundle prerequisites and emit structured report."""

from __future__ import annotations

import argparse
import json
from datetime import datetime
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
REPORTS = ROOT / "reports" / "release"


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Validate release bundle prerequisites.")
    parser.add_argument(
        "--run-id",
        default=f"release-check-v1-{datetime.now().strftime('%Y%m%d-%H%M%S')}",
        help="Run id.",
    )
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    run_dir = REPORTS / args.run_id
    run_dir.mkdir(parents=True, exist_ok=True)

    required_paths = [
        "docs/phase-f/STABILITY_REPORT_V1.md",
        "docs/phase-f/CROSS_PLATFORM_REPORT_V1.md",
        "docs/phase-e/PERF_REPORT_V1.md",
        "docs/phase-d/QUALITY_SIZE_REPORT_V1.md",
        ".github/workflows/phase-f-cross-platform.yml",
        ".github/workflows/nightly-regression.yml",
        "scripts/stability/run_phase_f_stability.py",
        "scripts/cross_platform/run_phase_f_cross_platform.py",
        "scripts/release/export_third_party_licenses.py",
    ]

    checks = []
    for rel in required_paths:
        p = ROOT / rel
        checks.append({"path": rel, "exists": p.exists(), "is_file": p.is_file()})

    passed = all(c["exists"] and c["is_file"] for c in checks)
    result = {
        "run_id": args.run_id,
        "passed": passed,
        "checks": checks,
    }
    (run_dir / "release_bundle_check.json").write_text(
        json.dumps(result, ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
    )

    summary = [
        "# Release Bundle Check",
        "",
        f"- run_id: `{args.run_id}`",
        f"- status: {'pass' if passed else 'fail'}",
        "",
        "Checks:",
    ]
    for c in checks:
        summary.append(f"- {c['path']}: {'ok' if c['exists'] and c['is_file'] else 'missing'}")
    summary.extend(
        [
            "",
            "Artifacts:",
            f"- `reports/release/{args.run_id}/release_bundle_check.json`",
        ]
    )
    (run_dir / "summary.md").write_text("\n".join(summary) + "\n", encoding="utf-8")

    print(f"Release bundle check complete: {run_dir}")
    return 0 if passed else 1


if __name__ == "__main__":
    raise SystemExit(main())
