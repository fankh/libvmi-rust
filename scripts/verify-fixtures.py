#!/usr/bin/env python3
import argparse
import hashlib
import re
import sys
import tomllib
from pathlib import Path


SHA256 = re.compile(r"[0-9a-f]{64}")
REQUIRED_TEXT = (
    "path",
    "license",
    "provenance",
    "generator",
    "architecture",
    "endianness",
    "physical_ranges",
    "expected",
)


def validate(root: Path, manifest_path: Path) -> list[str]:
    errors = []
    root = root.resolve()
    manifest_path = manifest_path.resolve()
    try:
        document = tomllib.loads(manifest_path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, tomllib.TOMLDecodeError) as error:
        return [f"cannot read fixture manifest: {error}"]
    if document.get("schema_version") != 1:
        errors.append("schema_version must be 1")
    fixtures = document.get("fixtures")
    if not isinstance(fixtures, list) or not fixtures:
        return errors + ["fixtures must be a non-empty array"]

    declared = set()
    for index, fixture in enumerate(fixtures):
        label = f"fixtures[{index}]"
        if not isinstance(fixture, dict):
            errors.append(f"{label} must be a table")
            continue
        for field in REQUIRED_TEXT:
            if not isinstance(fixture.get(field), str) or not fixture[field].strip():
                errors.append(f"{label}.{field} must be a non-empty string")
        relative = fixture.get("path")
        if not isinstance(relative, str) or not relative:
            continue
        normalized = Path(relative).as_posix()
        if normalized in declared:
            errors.append(f"duplicate fixture path: {normalized}")
            continue
        declared.add(normalized)
        candidate = (manifest_path.parent / relative).resolve()
        try:
            candidate.relative_to(root)
        except ValueError:
            errors.append(f"fixture escapes root: {relative}")
            continue
        if not candidate.is_file():
            errors.append(f"fixture is missing or not a regular file: {relative}")
            continue
        expected_size = fixture.get("size")
        if not isinstance(expected_size, int) or isinstance(expected_size, bool) or expected_size < 0:
            errors.append(f"{label}.size must be a non-negative integer")
        elif candidate.stat().st_size != expected_size:
            errors.append(f"size mismatch for {relative}")
        expected_hash = fixture.get("sha256")
        if not isinstance(expected_hash, str) or not SHA256.fullmatch(expected_hash):
            errors.append(f"{label}.sha256 must be lowercase SHA-256")
        elif hashlib.sha256(candidate.read_bytes()).hexdigest() != expected_hash:
            errors.append(f"SHA-256 mismatch for {relative}")
        page_size = fixture.get("page_size")
        if not isinstance(page_size, int) or isinstance(page_size, bool) or page_size < 0:
            errors.append(f"{label}.page_size must be a non-negative integer")

    corpus = manifest_path.parent / "corpus"
    actual = {
        path.relative_to(manifest_path.parent).as_posix()
        for path in corpus.rglob("*")
        if path.is_file()
    }
    for missing in sorted(actual - declared):
        errors.append(f"fixture is not declared: {missing}")
    for stale in sorted(declared - actual):
        errors.append(f"manifest path is not in the corpus: {stale}")
    return errors


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", type=Path, default=Path("fuzz"))
    parser.add_argument("--manifest", type=Path, default=Path("fuzz/fixtures.toml"))
    args = parser.parse_args()
    errors = validate(args.root, args.manifest)
    if errors:
        for error in errors:
            print(error, file=sys.stderr)
        return 1
    count = len(tomllib.loads(args.manifest.read_text(encoding="utf-8"))["fixtures"])
    print(f"fixture manifest verified: {count} files")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
