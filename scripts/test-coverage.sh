#!/usr/bin/env bash
set -euo pipefail

# The Windows all-target baseline measured on 2026-07-14 is 75.87% lines.
# Keep the gate below that value to accommodate target-specific code while
# still rejecting a material workspace-wide regression.
minimum_lines="${VMI_MINIMUM_LINE_COVERAGE:-70}"

cargo llvm-cov \
  --workspace \
  --all-targets \
  --summary-only \
  --fail-under-lines "$minimum_lines"
