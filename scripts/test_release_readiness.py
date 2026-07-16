#!/usr/bin/env python3

from __future__ import annotations

import contextlib
import importlib.util
import io
import tempfile
import unittest
from pathlib import Path


SCRIPT = Path(__file__).with_name("verify-release-readiness.py")
SPEC = importlib.util.spec_from_file_location("verify_release_readiness", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
validator = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(validator)


class ReleaseReadinessValidatorTests(unittest.TestCase):
    def run_document(self, document: str) -> int:
        with tempfile.TemporaryDirectory() as temporary:
            ledger = Path(temporary) / "release-readiness.toml"
            ledger.write_text(document, encoding="utf-8")
            original = validator.LEDGER
            validator.LEDGER = ledger
            try:
                with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(
                    io.StringIO()
                ):
                    return validator.main()
            finally:
                validator.LEDGER = original

    def test_committed_ledger_passes(self) -> None:
        self.assertEqual(validator.main(), 0)

    def test_incomplete_critical_gate_blocks_release_mode(self) -> None:
        self.assertEqual(validator.main(require_complete=True), 1)

    def test_unknown_status_fails(self) -> None:
        document = validator.LEDGER.read_text(encoding="utf-8").replace(
            'status = "in-progress"', 'status = "maybe"', 1
        )
        self.assertEqual(self.run_document(document), 1)

    def test_duplicate_gate_fails(self) -> None:
        document = validator.LEDGER.read_text(encoding="utf-8").replace(
            'id = "core-api"', 'id = "support-contract"', 1
        )
        self.assertEqual(self.run_document(document), 1)


if __name__ == "__main__":
    unittest.main()
