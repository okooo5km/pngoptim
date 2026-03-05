#!/usr/bin/env python3
"""Export third-party dependency license snapshot from cargo metadata."""

from __future__ import annotations

import argparse
import csv
import json
import subprocess
from collections import Counter
from datetime import datetime
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
REPORTS = ROOT / "reports" / "release"


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Export third-party licenses.")
    parser.add_argument(
        "--run-id",
        default=f"release-v1-{datetime.now().strftime('%Y%m%d-%H%M%S')}",
        help="Run id.",
    )
    return parser.parse_args()


def run_metadata() -> dict:
    proc = subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--locked"],
        cwd=ROOT,
        capture_output=True,
        text=True,
        check=True,
    )
    return json.loads(proc.stdout)


def main() -> int:
    args = parse_args()
    run_dir = REPORTS / args.run_id
    run_dir.mkdir(parents=True, exist_ok=True)

    meta = run_metadata()
    workspace_members = set(meta.get("workspace_members", []))
    rows = []
    for pkg in meta.get("packages", []):
        pkg_id = pkg.get("id", "")
        if pkg_id in workspace_members:
            continue
        rows.append(
            {
                "name": pkg.get("name", ""),
                "version": pkg.get("version", ""),
                "license": pkg.get("license", "UNKNOWN"),
                "repository": pkg.get("repository", ""),
                "source": pkg.get("source", ""),
            }
        )

    rows.sort(key=lambda r: (r["name"], r["version"]))
    with (run_dir / "third_party_licenses.csv").open("w", newline="", encoding="utf-8") as f:
        fields = ["name", "version", "license", "repository", "source"]
        writer = csv.DictWriter(f, fieldnames=fields)
        writer.writeheader()
        writer.writerows(rows)

    license_counter = Counter(r["license"] or "UNKNOWN" for r in rows)
    summary = [
        "# Third-party License Snapshot",
        "",
        f"- run_id: `{args.run_id}`",
        f"- total_dependencies: {len(rows)}",
        "",
        "License Counts:",
    ]
    for lic, cnt in sorted(license_counter.items(), key=lambda kv: (-kv[1], kv[0])):
        summary.append(f"- {lic}: {cnt}")
    summary.extend(
        [
            "",
            "Artifacts:",
            f"- `reports/release/{args.run_id}/third_party_licenses.csv`",
        ]
    )
    (run_dir / "summary.md").write_text("\n".join(summary) + "\n", encoding="utf-8")

    # Machine-readable summary
    stats = {
        "run_id": args.run_id,
        "total_dependencies": len(rows),
        "license_counts": dict(license_counter),
    }
    (run_dir / "license_stats.json").write_text(
        json.dumps(stats, ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
    )

    print(f"License export complete: {run_dir}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
