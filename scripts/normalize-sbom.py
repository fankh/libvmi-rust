#!/usr/bin/env python3
"""Normalize generated CycloneDX JSON for reproducible release archives."""

from __future__ import annotations

import json
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parent.parent
CANONICAL_ROOT_URI = "file:///workspace"


def normalize(document: object, root_uri: str) -> object:
    if isinstance(document, dict):
        output = {
            key: normalize(value, root_uri)
            for key, value in document.items()
            if key != "serialNumber"
        }
        metadata = output.get("metadata")
        if isinstance(metadata, dict):
            metadata.pop("timestamp", None)
        return output
    if isinstance(document, list):
        return [normalize(value, root_uri) for value in document]
    if isinstance(document, str):
        return document.replace(root_uri, CANONICAL_ROOT_URI)
    return document


def normalize_file(path: Path, root: Path = ROOT) -> None:
    document = json.loads(path.read_text(encoding="utf-8"))
    normalized = normalize(document, root.resolve().as_uri())
    path.write_text(
        json.dumps(normalized, indent=2, sort_keys=True, ensure_ascii=False) + "\n",
        encoding="utf-8",
        newline="\n",
    )


def main(arguments: list[str]) -> int:
    if not arguments:
        print("usage: normalize-sbom.py <bom.cdx.json> [...]", file=sys.stderr)
        return 2
    for argument in arguments:
        path = Path(argument)
        if not path.is_file() or path.suffixes[-2:] != [".cdx", ".json"]:
            print(f"invalid CycloneDX JSON path: {argument}", file=sys.stderr)
            return 1
        normalize_file(path)
    print(f"normalized {len(arguments)} CycloneDX SBOM files")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
