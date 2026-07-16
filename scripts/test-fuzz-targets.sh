#!/usr/bin/env sh
set -eu

workspace=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$workspace"

cargo fuzz build artifact_parsers
cargo fuzz build text_profiles

if [ "${VMI_RUN_FUZZ:-0}" = 1 ]; then
    seconds=${VMI_FUZZ_SECONDS:-30}
    cargo fuzz run artifact_parsers -- -max_total_time="$seconds" -max_len=65536
    cargo fuzz run text_profiles -- -max_total_time="$seconds" -max_len=65536
fi

echo 'fuzz targets verified'
