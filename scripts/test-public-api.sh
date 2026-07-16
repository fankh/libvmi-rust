#!/usr/bin/env sh
set -eu

workspace=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
temporary=$(mktemp)
trap 'rm -f "$temporary"' EXIT HUP INT TERM

cd "$workspace"
cargo public-api -p vmi -sss --color never >"$temporary"

if [ "${VMI_UPDATE_PUBLIC_API:-0}" = 1 ]; then
    cp "$temporary" docs/public-api.txt
    echo 'public API snapshot updated'
    exit 0
fi

if ! diff -u docs/public-api.txt "$temporary"; then
    echo 'public facade changed; review semver impact and update the snapshot intentionally' >&2
    exit 1
fi

echo 'public API snapshot verified'
