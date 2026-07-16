#!/usr/bin/env sh
set -eu

workspace=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$workspace"

export RUSTFLAGS="${RUSTFLAGS:+$RUSTFLAGS }-Zsanitizer=address"
export ASAN_OPTIONS="detect_leaks=1:halt_on_error=1:abort_on_error=1${ASAN_OPTIONS:+:$ASAN_OPTIONS}"

cargo test --workspace --all-targets --target x86_64-unknown-linux-gnu

echo 'AddressSanitizer workspace tests passed'
