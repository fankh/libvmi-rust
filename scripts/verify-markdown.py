#!/usr/bin/env python3
"""Validate local Markdown links and fenced code blocks without dependencies."""

from __future__ import annotations

import re
import sys
from pathlib import Path
from urllib.parse import unquote


ROOT = Path(__file__).resolve().parent.parent
INLINE_LINK = re.compile(r"!?\[[^\]]*\]\(([^)]+)\)")


def markdown_files(root: Path = ROOT) -> list[Path]:
    return sorted([*root.glob("*.md"), *(root / "docs").rglob("*.md")])


def heading_anchors(path: Path) -> set[str]:
    """Return GitHub-style anchors for ATX headings outside fenced blocks."""
    anchors: set[str] = set()
    counts: dict[str, int] = {}
    fenced = False
    for line in path.read_text(encoding="utf-8").splitlines():
        stripped = line.strip()
        if stripped.startswith("```"):
            fenced = not fenced
            continue
        if fenced:
            continue
        match = re.match(r"^#{1,6}\s+(.+?)\s*#*\s*$", stripped)
        if not match:
            continue
        heading = re.sub(r"<[^>]+>", "", match.group(1))
        heading = re.sub(r"[`*_~]", "", heading).lower()
        base = re.sub(r"[^\w\- ]", "", heading)
        base = re.sub(r"\s", "-", base)
        duplicate = counts.get(base, 0)
        counts[base] = duplicate + 1
        anchors.add(base if duplicate == 0 else f"{base}-{duplicate}")
    return anchors


def validate_file(path: Path, root: Path = ROOT) -> list[str]:
    errors: list[str] = []
    text = path.read_text(encoding="utf-8")
    fence_line = 0
    for line_number, line in enumerate(text.splitlines(), 1):
        stripped = line.strip()
        if stripped.startswith("```"):
            if fence_line:
                fence_line = 0
            else:
                fence_line = line_number
                if stripped == "```":
                    errors.append(f"{path}:{line_number}: fenced block needs a language")

        for raw_target in INLINE_LINK.findall(line):
            target = raw_target.strip().split(maxsplit=1)[0].strip("<>")
            if not target or target.startswith("mailto:") or "://" in target:
                continue
            target_parts = target.split("#", 1)
            relative = unquote(target_parts[0])
            anchor = unquote(target_parts[1]).lower() if len(target_parts) == 2 else ""
            resolved = (path.parent / relative).resolve() if relative else path.resolve()
            try:
                resolved.relative_to(root.resolve())
            except ValueError:
                errors.append(f"{path}:{line_number}: link escapes workspace: {target}")
                continue
            if not resolved.exists():
                errors.append(f"{path}:{line_number}: missing link target: {target}")
            elif anchor and resolved.suffix.lower() == ".md" and anchor not in heading_anchors(resolved):
                errors.append(f"{path}:{line_number}: missing heading anchor: {target}")

    if fence_line:
        errors.append(f"{path}:{fence_line}: unclosed fenced block")
    return errors


def main() -> int:
    files = markdown_files()
    errors = [error for path in files for error in validate_file(path)]
    if errors:
        print("\n".join(errors), file=sys.stderr)
        return 1
    print(f"Markdown verified: {len(files)} files")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
