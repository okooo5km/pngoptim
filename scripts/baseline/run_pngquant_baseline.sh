#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
RUN_ID="${1:-baseline-$(date +%Y%m%d-%H%M%S)}"
PROFILE="${2:-Q_MED}"
DATASET_DIR="${ROOT_DIR}/dataset/functional"
REPORT_DIR="${ROOT_DIR}/reports/baseline/${RUN_ID}"
OUT_DIR="${REPORT_DIR}/out"

if ! command -v pngquant >/dev/null 2>&1; then
  echo "pngquant not found in PATH" >&2
  exit 2
fi

case "${PROFILE}" in
  Q_HIGH)
    QUALITY_MIN=70
    QUALITY_MAX=90
    SPEED=3
    DITHER_MODE="fs"
    ;;
  Q_MED)
    QUALITY_MIN=55
    QUALITY_MAX=75
    SPEED=4
    DITHER_MODE="fs"
    ;;
  Q_LOW)
    QUALITY_MIN=35
    QUALITY_MAX=55
    SPEED=6
    DITHER_MODE="fs"
    ;;
  FAST)
    QUALITY_MIN=55
    QUALITY_MAX=75
    SPEED=10
    DITHER_MODE="fs"
    ;;
  NO_DITHER)
    QUALITY_MIN=55
    QUALITY_MAX=75
    SPEED=4
    DITHER_MODE="nofs"
    ;;
  FUNC_BASE)
    QUALITY_MIN=""
    QUALITY_MAX=""
    SPEED=4
    DITHER_MODE="fs"
    ;;
  *)
    echo "Unsupported profile: ${PROFILE}" >&2
    exit 3
    ;;
esac

mkdir -p "${OUT_DIR}"

SIZE_CSV="${REPORT_DIR}/size_report.csv"
PERF_CSV="${REPORT_DIR}/perf_report.csv"
SUMMARY_MD="${REPORT_DIR}/summary.md"

echo "run_id,profile,input_file,input_bytes,output_file,output_bytes,size_ratio,exit_code" > "${SIZE_CSV}"
echo "run_id,profile,input_file,elapsed_ms,exit_code" > "${PERF_CSV}"

total=0
success=0
failed=0

for input in "${DATASET_DIR}"/*.png; do
  [[ -e "${input}" ]] || continue
  total=$((total + 1))

  base="$(basename "${input}")"
  stem="${base%.png}"
  output="${OUT_DIR}/${stem}.q.png"
  input_bytes="$(wc -c < "${input}" | tr -d ' ')"

  start_ns="$(python3 -c 'import time; print(time.time_ns())')"
  args=(--speed "${SPEED}" --force --output "${output}")
  if [[ "${DITHER_MODE}" == "nofs" ]]; then
    args=(--nofs "${args[@]}")
  fi
  if [[ -n "${QUALITY_MIN}" && -n "${QUALITY_MAX}" ]]; then
    args=(--quality="${QUALITY_MIN}-${QUALITY_MAX}" "${args[@]}")
  fi
  set +e
  pngquant "${args[@]}" -- "${input}" >/dev/null 2>&1
  exit_code=$?
  set -e
  end_ns="$(python3 -c 'import time; print(time.time_ns())')"

  elapsed_ms="$(( (end_ns - start_ns) / 1000000 ))"

  if [[ "${exit_code}" -eq 0 && -f "${output}" ]]; then
    output_bytes="$(wc -c < "${output}" | tr -d ' ')"
    size_ratio="$(awk -v in_bytes="${input_bytes}" -v out_bytes="${output_bytes}" 'BEGIN { if (in_bytes == 0) { print "0.000000" } else { printf "%.6f", out_bytes / in_bytes } }')"
    success=$((success + 1))
  else
    output_bytes=""
    size_ratio=""
    failed=$((failed + 1))
  fi

  echo "${RUN_ID},${PROFILE},${base},${input_bytes},$(basename "${output}"),${output_bytes},${size_ratio},${exit_code}" >> "${SIZE_CSV}"
  echo "${RUN_ID},${PROFILE},${base},${elapsed_ms},${exit_code}" >> "${PERF_CSV}"
done

cat > "${SUMMARY_MD}" <<EOF
# Baseline Run Summary

- run_id: \`${RUN_ID}\`
- profile: \`${PROFILE}\`
- dataset: \`dataset/functional\`
- total_samples: ${total}
- success: ${success}
- failed: ${failed}
- size_report: \`reports/baseline/${RUN_ID}/size_report.csv\`
- perf_report: \`reports/baseline/${RUN_ID}/perf_report.csv\`
EOF

echo "Baseline run complete: ${REPORT_DIR}"
