#!/usr/bin/env python3

from __future__ import annotations

import contextlib
import importlib.util
import io
import tempfile
import unittest
from pathlib import Path


SCRIPT = Path(__file__).with_name("verify-support-matrix.py")
SPEC = importlib.util.spec_from_file_location("verify_support_matrix", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
validator = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(validator)


class SupportMatrixValidatorTests(unittest.TestCase):
    def run_document(self, document: str) -> int:
        with tempfile.TemporaryDirectory() as temporary:
            matrix = Path(temporary) / "support-matrix.toml"
            matrix.write_text(document, encoding="utf-8")
            original = validator.MATRIX
            validator.MATRIX = matrix
            try:
                with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(
                    io.StringIO()
                ):
                    return validator.main()
            finally:
                validator.MATRIX = original

    def test_committed_matrix_passes(self) -> None:
        self.assertEqual(self.run_document(validator.MATRIX.read_text(encoding="utf-8")), 0)

    def test_duplicate_provider_fails(self) -> None:
        document = validator.MATRIX.read_text(encoding="utf-8").replace(
            'id = "raw-dump"', 'id = "fake-read-only"', 1
        )
        self.assertEqual(self.run_document(document), 1)

    def test_unknown_capability_fails(self) -> None:
        document = validator.MATRIX.read_text(encoding="utf-8").replace(
            'capabilities = ["memory_read"]', 'capabilities = ["teleport"]', 1
        )
        self.assertEqual(self.run_document(document), 1)

    def test_unknown_v1_target_fails(self) -> None:
        document = validator.MATRIX.read_text(encoding="utf-8").replace(
            'v1_target = "supported"', 'v1_target = "unbounded"', 1
        )
        self.assertEqual(self.run_document(document), 1)

    def test_unknown_platform_fails(self) -> None:
        document = validator.MATRIX.read_text(encoding="utf-8").replace(
            'platforms = ["linux", "windows", "macos"]',
            'platforms = ["mainframe"]',
            1,
        )
        self.assertEqual(self.run_document(document), 1)


if __name__ == "__main__":
    unittest.main()
