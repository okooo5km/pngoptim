#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
DENY_CONFIG="${ROOT_DIR}/config/compliance/deny.toml"
OUT_DIR="${ROOT_DIR}/reports/compliance"
OUT_FILE="${OUT_DIR}/cargo-deny-check.txt"

mkdir -p "${OUT_DIR}"

if ! command -v cargo-deny >/dev/null 2>&1; then
  echo "cargo-deny is not installed. Install it with: cargo install cargo-deny --locked" >&2
  exit 2
fi

(
  cd "${ROOT_DIR}"
  cargo-deny check --config "${DENY_CONFIG}" licenses advisories bans sources
) | tee "${OUT_FILE}"

echo "Compliance report: ${OUT_FILE}"

