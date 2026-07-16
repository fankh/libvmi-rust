#!/usr/bin/env python3
"""Validate the machine-readable v1 release gate ledger."""

from __future__ import annotations

import re
import sys
import tomllib
from pathlib import Path


ROOT = Path(__file__).resolve().parent.parent
LEDGER = ROOT / "release-readiness.toml"
STATUSES = {"complete", "in-progress", "pending", "blocked"}
ID_PATTERN = re.compile(r"^[a-z0-9]+(?:-[a-z0-9]+)*$")


def fail(message: str) -> None:
    raise ValueError(f"{LEDGER.name}: {message}")


def main() -> int:
    try:
        document = tomllib.loads(LEDGER.read_text(encoding="utf-8"))
        if set(document) != {"schema_version", "release", "gates"}:
            fail("top-level keys must be schema_version, release, and gates")
        if document["schema_version"] != 1:
            fail("schema_version must equal 1")
        if document["release"] != "1.0.0":
            fail("release must equal 1.0.0")
        gates = document["gates"]
        if not isinstance(gates, list) or not gates:
            fail("gates must be a non-empty array")
        seen: set[str] = set()
        for gate in gates:
            if not isinstance(gate, dict):
                fail("each gate must be a table")
            if set(gate) != {"id", "title", "status", "critical", "evidence"}:
                fail(f"gate has invalid fields: {sorted(gate)}")
            gate_id = gate["id"]
            if not isinstance(gate_id, str) or not ID_PATTERN.fullmatch(gate_id):
                fail(f"invalid gate id {gate_id!r}")
            if gate_id in seen:
                fail(f"duplicate gate id {gate_id!r}")
            seen.add(gate_id)
            if not isinstance(gate["title"], str) or not gate["title"].strip():
                fail(f"gate {gate_id!r} has invalid title")
            if gate["status"] not in STATUSES:
                fail(f"gate {gate_id!r} has invalid status {gate['status']!r}")
            if not isinstance(gate["critical"], bool):
                fail(f"gate {gate_id!r} critical must be boolean")
            evidence = gate["evidence"]
            if not isinstance(evidence, list) or not evidence:
                fail(f"gate {gate_id!r} evidence must be non-empty")
            for item in evidence:
                if not isinstance(item, str) or not item.strip():
                    fail(f"gate {gate_id!r} has invalid evidence path")
                candidate = (ROOT / item).resolve()
                try:
                    candidate.relative_to(ROOT.resolve())
                except ValueError:
                    fail(f"gate {gate_id!r} evidence escapes the repository: {item!r}")
                if not candidate.exists():
                    fail(f"gate {gate_id!r} evidence does not exist: {item!r}")
    except (OSError, tomllib.TOMLDecodeError, KeyError, TypeError, ValueError) as error:
        print(error, file=sys.stderr)
        return 1
    print(f"release readiness verified: {len(gates)} gates")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
