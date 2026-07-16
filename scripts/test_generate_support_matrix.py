#!/usr/bin/env python3

from __future__ import annotations

import importlib.util
import contextlib
import io
import tempfile
import unittest
from pathlib import Path


SCRIPT = Path(__file__).with_name("generate-support-matrix.py")
SPEC = importlib.util.spec_from_file_location("generate_support_matrix", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
generator = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(generator)


class SupportMatrixGeneratorTests(unittest.TestCase):
    def test_render_is_deterministic_and_escapes_table_cells(self) -> None:
        document = {
            "providers": [{
                "id": "test",
                "implemented": True,
                "display_name": "A | B",
                "maturity": "preview",
                "capabilities": ["memory_read"],
                "mechanism": "first\nsecond",
            }]
        }
        rendered = generator.render(document)
        self.assertEqual(rendered, generator.render(document))
        self.assertIn("A \\| B", rendered)
        self.assertIn("first second", rendered)
        self.assertIn("| Yes |", rendered)

    def test_check_detects_stale_output(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = root / "matrix.toml"
            output = root / "matrix.md"
            source.write_text(
                '[[providers]]\nid="test"\nimplemented=true\n'
                'display_name="Test"\nmaturity="preview"\n'
                'capabilities=["memory_read"]\nmechanism="fixture"\n',
                encoding="utf-8",
            )
            output.write_text("stale", encoding="utf-8")
            with contextlib.redirect_stderr(io.StringIO()):
                self.assertEqual(
                    generator.main(
                        ["--check", "--source", str(source), "--output", str(output)]
                    ),
                    1,
                )

    def test_committed_output_is_current(self) -> None:
        self.assertEqual(generator.main(["--check"]), 0)


if __name__ == "__main__":
    unittest.main()
