#!/usr/bin/env bash
set -euo pipefail

version=${VMI_QEMU_VERSION:-11.0.2}
expected_version=11.0.2
expected_sha256=3745f6ea88e2e87fe0dc838b2b1d4e0a770bf48e01a1d5a186842a1fff76ccf5
workspace=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
prefix=${VMI_QEMU_PREFIX:-"$workspace/.tools/qemu-$version"}
jobs=${VMI_QEMU_BUILD_JOBS:-2}

if [ "$version" != "$expected_version" ]; then
    echo "unsupported QEMU version $version; update the reviewed hash before changing versions" >&2
    exit 2
fi
case "$jobs" in
    ''|*[!0-9]*|0) echo "VMI_QEMU_BUILD_JOBS must be a positive integer" >&2; exit 2 ;;
esac

if [ -x "$prefix/bin/qemu-system-x86_64" ]; then
    actual=$($prefix/bin/qemu-system-x86_64 --version | head -n 1)
    case "$actual" in
        "QEMU emulator version $version"*) echo "$actual already installed at $prefix"; exit 0 ;;
        *) echo "unexpected cached QEMU binary: $actual" >&2; exit 1 ;;
    esac
fi

for command in curl sha256sum tar ninja python3 pkg-config; do
    command -v "$command" >/dev/null || { echo "missing build dependency: $command" >&2; exit 2; }
done

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT HUP INT TERM
archive="$work/qemu-$version.tar.xz"
curl --fail --location --retry 3 --output "$archive" \
    "https://download.qemu.org/qemu-$version.tar.xz"
printf '%s  %s\n' "$expected_sha256" "$archive" | sha256sum --check --status

mkdir "$work/source" "$work/build"
tar -xf "$archive" -C "$work/source" --strip-components=1
(
    cd "$work/build"
    "$work/source/configure" \
        --prefix="$prefix" \
        --target-list=x86_64-softmmu \
        --without-default-features \
        --enable-tcg \
        --disable-docs
    ninja -j "$jobs"
    ninja install
)

"$prefix/bin/qemu-system-x86_64" --version | head -n 1
