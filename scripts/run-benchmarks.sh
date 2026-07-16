#!/usr/bin/env bash
set -euo pipefail

output="${VMI_BENCH_OUTPUT:-target/vmi-benchmark.json}"
mkdir -p "$(dirname "$output")"
cargo bench -p vmi --bench core -- "$@" | tee "$output"

if [[ -n "${VMI_BENCH_BASELINE:-}" ]]; then
  if [[ -n "${PYTHON:-}" ]]; then
    python_command="$PYTHON"
  elif command -v python3 >/dev/null 2>&1; then
    python_command="python3"
  elif command -v python >/dev/null 2>&1; then
    python_command="python"
  else
    echo "Python 3 is required to compare benchmark results" >&2
    exit 1
  fi
  "$python_command" scripts/compare-benchmark.py \
    "${VMI_BENCH_BASELINE}" "$output" \
    --maximum-regression "${VMI_BENCH_MAX_REGRESSION:-10}"
fi
