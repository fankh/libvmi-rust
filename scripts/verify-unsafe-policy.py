#!/usr/bin/env python3
"""Enforce that unsafe Rust is confined to explicitly reviewed boundary crates."""

from __future__ import annotations

import re
import sys
import tomllib
from pathlib import Path


ROOT = Path(__file__).resolve().parent.parent
CRATES = ROOT / "crates"
WORKSPACE = ROOT / "Cargo.toml"
UNSAFE_CRATES = {"vmi-driver-xen", "vmi-ffi"}
UNSAFE_SYNTAX = re.compile(r"\bunsafe\s*(?:extern\b|fn\b|impl\b|trait\b|\{)")


def fail(message: str) -> None:
    raise ValueError(f"unsafe policy: {message}")


def validate(root: Path = ROOT) -> int:
    try:
        workspace = tomllib.loads((root / "Cargo.toml").read_text(encoding="utf-8"))
        level = workspace.get("workspace", {}).get("lints", {}).get("rust", {}).get("unsafe_code")
        if level != "forbid":
            fail("workspace unsafe_code lint must be forbid")

        crates = root / "crates"
        names: set[str] = set()
        observed_unsafe: set[str] = set()
        for manifest_path in sorted(crates.glob("*/Cargo.toml")):
            manifest = tomllib.loads(manifest_path.read_text(encoding="utf-8"))
            name = manifest.get("package", {}).get("name")
            if not isinstance(name, str) or not name:
                fail(f"{manifest_path} has no package name")
            names.add(name)

            source_root = manifest_path.parent / "src"
            contains_unsafe = any(
                UNSAFE_SYNTAX.search(path.read_text(encoding="utf-8"))
                for path in source_root.rglob("*.rs")
            )
            if contains_unsafe:
                observed_unsafe.add(name)

            inherits = manifest.get("lints", {}).get("workspace") is True
            if name in UNSAFE_CRATES and inherits:
                fail(f"reviewed unsafe crate {name!r} cannot inherit unsafe_code=forbid")
            if name not in UNSAFE_CRATES and not inherits:
                fail(f"safe crate {name!r} must inherit workspace lints")

        unknown_exemptions = UNSAFE_CRATES - names
        if unknown_exemptions:
            fail(f"unsafe allowlist contains missing crates {sorted(unknown_exemptions)}")
        if observed_unsafe != UNSAFE_CRATES:
            fail(
                f"unsafe syntax inventory mismatch: expected={sorted(UNSAFE_CRATES)} "
                f"observed={sorted(observed_unsafe)}"
            )
    except (OSError, tomllib.TOMLDecodeError, ValueError) as error:
        print(error, file=sys.stderr)
        return 1

    print(f"unsafe policy verified: {len(names) - len(UNSAFE_CRATES)} safe crates, "
          f"{len(UNSAFE_CRATES)} reviewed boundary crates")
    return 0


if __name__ == "__main__":
    raise SystemExit(validate())
