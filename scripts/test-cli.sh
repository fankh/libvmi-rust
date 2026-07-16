#!/usr/bin/env sh
set -eu

workspace=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
temporary=$(mktemp -d)
trap 'rm -rf "$temporary"' EXIT HUP INT TERM

cd "$workspace"
printf 'VMI!' > "$temporary/memory.raw"

output=$(cargo run --quiet -p vmi-cli -- \
    read-raw "$temporary/memory.raw" 0X0 4)
expected='0000000000000000: 56 4d 49 21'
if [ "$output" != "$expected" ]; then
    printf 'unexpected CLI memory output:\n%s\n' "$output" >&2
    exit 1
fi

vmware_output=$(cargo run --quiet -p vmi-cli -- \
    read-vmware-vmem "$temporary/memory.raw" 0x2000 0x2000 4)
if [ "$vmware_output" != '0000000000002000: 56 4d 49 21' ]; then
    printf 'unexpected VMware VMEM output:\n%s\n' "$vmware_output" >&2
    exit 1
fi

example_output=$(cargo run --quiet -p vmi --example inspect_raw -- \
    "$temporary/memory.raw" 0X0 4)
if [ "$example_output" != '564d4921' ]; then
    printf 'unexpected facade example output:\n%s\n' "$example_output" >&2
    exit 1
fi

if cargo run --quiet -p vmi-cli -- \
    read-raw "$temporary/memory.raw" 0xg 4 \
    >"$temporary/invalid.out" 2>"$temporary/invalid.err"; then
    echo 'invalid CLI number unexpectedly succeeded' >&2
    exit 1
fi
grep -F 'invalid number 0xg' "$temporary/invalid.err" >/dev/null

if cargo run --quiet -p vmi-cli -- unknown-command \
    >"$temporary/usage.out" 2>"$temporary/usage.err"; then
    echo 'unknown CLI command unexpectedly succeeded' >&2
    exit 1
fi
grep -F 'qemu-event' "$temporary/usage.err" >/dev/null
grep -F 'vbox-reg-write' "$temporary/usage.err" >/dev/null
grep -F 'read-vmware-core' "$temporary/usage.err" >/dev/null

echo 'CLI and facade example smoke tests passed'
