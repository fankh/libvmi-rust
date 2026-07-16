#!/usr/bin/env bash
set -euo pipefail

workspace=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
evidence=${VMI_QEMU_EVIDENCE:-"$workspace/target/qemu-qualification.json"}
work=$(mktemp -d)
qemu_pid=

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

mkdir -p "$(dirname -- "$evidence")"
python3 - "$work/results" "$evidence" "$(qemu-system-x86_64 --version | head -n 1)" <<'PY'
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
}
document["semantic_checks"] = semantic_checks
document["passed"] = document["passed"] and all(semantic_checks.values())
pathlib.Path(sys.argv[2]).write_text(json.dumps(document, indent=2) + "\n", encoding="utf-8")
if not document["passed"]:
    raise SystemExit(1)
PY

echo "QEMU integration evidence written to $evidence"
