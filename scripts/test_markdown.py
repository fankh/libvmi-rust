#!/usr/bin/env python3

from __future__ import annotations

import importlib.util
import tempfile
import unittest
from pathlib import Path


SCRIPT = Path(__file__).with_name("verify-markdown.py")
SPEC = importlib.util.spec_from_file_location("verify_markdown", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
validator = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(validator)


class MarkdownValidatorTests(unittest.TestCase):
    def test_valid_links_and_labeled_fences_pass(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            target = root / "target.md"
            target.write_text("# Heading\n", encoding="utf-8")
            source = root / "source.md"
            source.write_text(
                "[target](target.md#heading)\n\n```rust\nlet value = 1;\n```\n",
                encoding="utf-8",
            )
            self.assertEqual(validator.validate_file(source, root), [])

    def test_missing_and_escaping_links_fail(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = root / "source.md"
            source.write_text("[missing](none.md) [escape](../outside.md)\n", encoding="utf-8")
            errors = validator.validate_file(source, root)
            self.assertEqual(len(errors), 2)

    def test_unlabeled_and_unclosed_fences_fail(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = root / "source.md"
            source.write_text("```\ntext\n", encoding="utf-8")
            errors = validator.validate_file(source, root)
            self.assertEqual(len(errors), 2)

    def test_missing_heading_anchor_fails(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            target = root / "target.md"
            target.write_text("# Existing Heading\n", encoding="utf-8")
            source = root / "source.md"
            source.write_text("[stale](target.md#renamed-heading)\n", encoding="utf-8")
            errors = validator.validate_file(source, root)
            self.assertEqual(len(errors), 1)
            self.assertIn("missing heading anchor", errors[0])

    def test_duplicate_heading_anchors_are_numbered(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "headings.md"
            path.write_text("# Same Heading\n## Same Heading\n", encoding="utf-8")
            self.assertEqual(
                validator.heading_anchors(path), {"same-heading", "same-heading-1"}
            )


if __name__ == "__main__":
    unittest.main()
