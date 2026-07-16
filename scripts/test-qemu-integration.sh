#!/usr/bin/env bash
set -euo pipefail

workspace=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
evidence=${VMI_QEMU_EVIDENCE:-"$workspace/target/qemu-qualification.json"}
soak_seconds=${VMI_QEMU_SOAK_SECONDS:-10}
max_rss_growth_kib=${VMI_QEMU_MAX_RSS_GROWTH_KIB:-32768}
max_fd_growth=${VMI_QEMU_MAX_FD_GROWTH:-16}
work=$(mktemp -d)
qemu_pid=

for value in "$soak_seconds" "$max_rss_growth_kib" "$max_fd_growth"; do
    case "$value" in
        ''|*[!0-9]*) echo "QEMU soak limits must be non-negative integers" >&2; exit 2 ;;
    esac
done

cleanup() {
    if [ -n "$qemu_pid" ]; then
        kill "$qemu_pid" 2>/dev/null || true
        wait "$qemu_pid" 2>/dev/null || true
    fi
    rm -rf "$work"
}
trap cleanup EXIT HUP INT TERM

command -v qemu-system-x86_64 >/dev/null
command -v python3 >/dev/null

qemu-system-x86_64 \
    -machine q35,accel=tcg \
    -cpu qemu64 \
    -smp 2 \
    -m 64 \
    -nodefaults \
    -display none \
    -serial none \
    -monitor none \
    -qmp tcp:127.0.0.1:4444,server=on,wait=off \
    -gdb tcp::1234 \
    >"$work/qemu.stdout" 2>"$work/qemu.stderr" &
qemu_pid=$!

for _ in $(seq 1 100); do
    if python3 - <<'PY'
import socket
with socket.create_connection(("127.0.0.1", 4444), timeout=0.1):
    pass
PY
    then
        break
    fi
    sleep 0.1
done

if ! kill -0 "$qemu_pid" 2>/dev/null; then
    cat "$work/qemu.stderr" >&2
    exit 1
fi

cd "$workspace"
cargo build --locked -p vmi-cli
cli="$workspace/target/debug/vmi-cli"
range="$work/range.bin"
core="$work/core.elf"

run() {
    name=$1
    shift
    set +e
    output=$($cli "$@" 2>&1)
    status=$?
    set -e
    if [ "$name" = read ] && [ "$status" -eq 0 ]; then
        printf '%s' "$output" >"$work/live-read"
    fi
    printf '%s\0%s\0%s\0' "$name" "$status" "$output" >>"$work/results"
}

run_expected_failure() {
    name=$1
    shift
    set +e
    output=$($cli "$@" 2>&1)
    actual_status=$?
    set -e
    if [ "$actual_status" -eq 0 ]; then
        status=1
    else
        status=0
    fi
    printf '%s\0%s\0%s\0' "$name" "$status" "$output" >>"$work/results"
}

run status qemu-status 127.0.0.1:4444
run register qemu-reg-read 127.0.0.1:4444 0 rip
run read qemu-read 127.0.0.1:4444 0 16
run pause qemu-pause 127.0.0.1:4444
run paused_status qemu-status 127.0.0.1:4444
run resume_event qemu-event 127.0.0.1:4444 2000 resume
run acquire qemu-acquire 127.0.0.1:4444 "$range" 0 4096
run inspect_range read-raw "$range" 0 16
run dump qemu-dump 127.0.0.1:4444 "$core"
run inspect_core read-elf "$core" 0 16

soak_started=$(date +%s)
soak_deadline=$((soak_started + soak_seconds))
soak_iterations=0
initial_rss_kib=$(awk '/^VmRSS:/ { print $2 }' "/proc/$qemu_pid/status")
initial_fds=$(find "/proc/$qemu_pid/fd" -mindepth 1 -maxdepth 1 | wc -l)
max_rss_kib=$initial_rss_kib
max_fds=$initial_fds
soak_status=0
while [ "$(date +%s)" -lt "$soak_deadline" ] || [ "$soak_iterations" -eq 0 ]; do
    status_output=$($cli qemu-status 127.0.0.1:4444 2>&1) || soak_status=1
    read_output=$($cli qemu-read 127.0.0.1:4444 0 16 2>&1) || soak_status=1
    if [ "$status_output" != "Running" ] || [ "$read_output" != "$(cat "$work/live-read")" ]; then
        soak_status=1
    fi
    rss_kib=$(awk '/^VmRSS:/ { print $2 }' "/proc/$qemu_pid/status")
    fds=$(find "/proc/$qemu_pid/fd" -mindepth 1 -maxdepth 1 | wc -l)
    [ "$rss_kib" -le "$max_rss_kib" ] || max_rss_kib=$rss_kib
    [ "$fds" -le "$max_fds" ] || max_fds=$fds
    soak_iterations=$((soak_iterations + 1))
done
rss_growth_kib=$((max_rss_kib - initial_rss_kib))
fd_growth=$((max_fds - initial_fds))
if [ "$rss_growth_kib" -gt "$max_rss_growth_kib" ] || [ "$fd_growth" -gt "$max_fd_growth" ]; then
    soak_status=1
fi
printf '%s\0%s\0%s\0' soak "$soak_status" "iterations=$soak_iterations rss_growth_kib=$rss_growth_kib fd_growth=$fd_growth" >>"$work/results"

kill "$qemu_pid"
wait "$qemu_pid" 2>/dev/null || true
qemu_pid=
run_expected_failure abrupt_disconnect qemu-status 127.0.0.1:4444

mkdir -p "$(dirname -- "$evidence")"
python3 - "$work/results" "$evidence" "$(qemu-system-x86_64 --version | head -n 1)" "$soak_seconds" "$soak_iterations" "$rss_growth_kib" "$fd_growth" <<'PY'
import datetime
import json
import pathlib
import platform
import sys

parts = pathlib.Path(sys.argv[1]).read_bytes().split(b"\0")
commands = []
for index in range(0, len(parts) - 1, 3):
    commands.append({
        "name": parts[index].decode("utf-8", "replace"),
        "status": int(parts[index + 1]),
        "output": parts[index + 2].decode("utf-8", "replace"),
    })
document = {
    "schema": 1,
    "provider": "qemu-qmp",
    "timestamp_utc": datetime.datetime.now(datetime.timezone.utc).isoformat(),
    "host": platform.platform(),
    "qemu": sys.argv[3],
    "soak": {
        "requested_seconds": int(sys.argv[4]),
        "iterations": int(sys.argv[5]),
        "rss_growth_kib": int(sys.argv[6]),
        "fd_growth": int(sys.argv[7]),
    },
    "commands": commands,
    "passed": all(command["status"] == 0 for command in commands),
}
by_name = {command["name"]: command for command in commands}
semantic_checks = {
    "initial state is running": by_name["status"]["output"] == "Running",
    "pause changes state": by_name["paused_status"]["output"] == "Paused",
    "resume event delivered": "event=RESUME" in by_name["resume_event"]["output"],
    "range bytes match live read": by_name["read"]["output"] == by_name["inspect_range"]["output"],
    "core bytes match live read": by_name["read"]["output"] == by_name["inspect_core"]["output"],
    "soak completed within resource budgets": by_name["soak"]["status"] == 0,
    "abrupt disconnect fails closed": by_name["abrupt_disconnect"]["status"] == 0,
}
document["semantic_checks"] = semantic_checks
document["passed"] = document["passed"] and all(semantic_checks.values())
pathlib.Path(sys.argv[2]).write_text(json.dumps(document, indent=2) + "\n", encoding="utf-8")
if not document["passed"]:
    raise SystemExit(1)
PY

echo "QEMU integration evidence written to $evidence"
