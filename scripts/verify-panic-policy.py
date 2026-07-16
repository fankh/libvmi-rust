#!/usr/bin/env python3
"""Reject explicit panic primitives in production Rust crate sources."""

from __future__ import annotations

import re
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parent.parent
TEST_MARKER = "#[cfg(test)]"
FORBIDDEN = re.compile(
    r"(?:\.(?:unwrap|expect)\s*\(|\b(?:panic|unreachable|todo|unimplemented)!\s*\()"
)


def production_prefix(text: str) -> str:
    """Exclude the conventional trailing cfg(test) module from policy checks."""
    return text.split(TEST_MARKER, 1)[0]


def validate_source(path: Path) -> list[str]:
    errors: list[str] = []
    production = production_prefix(path.read_text(encoding="utf-8"))
    for line_number, line in enumerate(production.splitlines(), 1):
        if FORBIDDEN.search(line):
            errors.append(f"{path}:{line_number}: production panic primitive: {line.strip()}")
    return errors


def source_files(root: Path = ROOT) -> list[Path]:
    return sorted((root / "crates").glob("*/src/*.rs"))


def main() -> int:
    files = source_files()
    errors = [error for path in files for error in validate_source(path)]
    if errors:
        print("\n".join(errors), file=sys.stderr)
        return 1
    print(f"panic policy verified: {len(files)} production source files")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
