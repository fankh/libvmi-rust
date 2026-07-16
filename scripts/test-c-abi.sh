#!/usr/bin/env sh
set -eu

workspace=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
temporary=$(mktemp -d)
trap 'rm -rf "$temporary"' EXIT HUP INT TERM

cd "$workspace"
cargo build --release -p vmi-ffi
printf 'VMI!' > "$temporary/memory.raw"

expected_exports=$(printf '%s\n' \
    vmi_abi_version \
    vmi_last_error \
    vmi_snapshot_close \
    vmi_snapshot_open \
    vmi_snapshot_read \
    vmi_snapshot_segment \
    vmi_snapshot_segment_count)
actual_exports=$(nm -D --defined-only target/release/libvmi_ffi.so \
    | awk '$2 == "T" {print $3}' \
    | sort)
if [ "$actual_exports" != "$expected_exports" ]; then
    printf 'unexpected vmi-ffi exports:\n%s\n' "$actual_exports" >&2
    exit 1
fi

cc -std=c11 -Wall -Wextra -Werror \
    -I crates/vmi-ffi/include \
    crates/vmi-ffi/tests/c_smoke.c \
    -L target/release \
    -Wl,-rpath,"$workspace/target/release" \
    -lvmi_ffi \
    -o "$temporary/vmi-ffi-c-smoke"
"$temporary/vmi-ffi-c-smoke" "$temporary/memory.raw"

cc -std=c11 -Wall -Wextra -Werror -DVMI_STATIC \
    -I crates/vmi-ffi/include \
    -c crates/vmi-ffi/tests/c_smoke.c \
    -o "$temporary/vmi-ffi-static-c.o"
cc "$temporary/vmi-ffi-static-c.o" \
    target/release/libvmi_ffi.a \
    -ldl -lpthread -lm -llzma -lbz2 -lz -lzstd \
    -o "$temporary/vmi-ffi-static-c"
"$temporary/vmi-ffi-static-c" "$temporary/memory.raw"

c++ -x c++ -std=c++17 -Wall -Wextra -Werror -DVMI_STATIC \
    -I crates/vmi-ffi/include \
    -c crates/vmi-ffi/tests/c_smoke.c \
    -o "$temporary/vmi-ffi-static-cxx.o"
c++ "$temporary/vmi-ffi-static-cxx.o" \
    target/release/libvmi_ffi.a \
    -ldl -lpthread -lm -llzma -lbz2 -lz -lzstd \
    -o "$temporary/vmi-ffi-static-cxx"
"$temporary/vmi-ffi-static-cxx" "$temporary/memory.raw"
