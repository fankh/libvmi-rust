#!/usr/bin/env python3

from __future__ import annotations

import contextlib
import importlib.util
import io
import shutil
import tempfile
import unittest
from pathlib import Path


SCRIPT = Path(__file__).with_name("verify-unsafe-policy.py")
SPEC = importlib.util.spec_from_file_location("verify_unsafe_policy", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
policy = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(policy)


class UnsafePolicyTests(unittest.TestCase):
    def run_copy(self, mutate=None) -> int:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            shutil.copy(policy.ROOT / "Cargo.toml", root / "Cargo.toml")
            shutil.copytree(policy.ROOT / "crates", root / "crates")
            if mutate is not None:
                mutate(root)
            with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
                return policy.validate(root)

    def test_committed_policy_passes(self) -> None:
        self.assertEqual(policy.validate(), 0)

    def test_new_unsafe_syntax_in_safe_crate_fails(self) -> None:
        def mutate(root: Path) -> None:
            source = root / "crates" / "vmi-types" / "src" / "lib.rs"
            source.write_text(source.read_text(encoding="utf-8") + "\nunsafe {}\n", encoding="utf-8")

        self.assertEqual(self.run_copy(mutate), 1)

    def test_safe_crate_cannot_drop_workspace_lints(self) -> None:
        def mutate(root: Path) -> None:
            manifest = root / "crates" / "vmi-types" / "Cargo.toml"
            manifest.write_text(
                manifest.read_text(encoding="utf-8").replace("[lints]\nworkspace = true\n", ""),
                encoding="utf-8",
            )

        self.assertEqual(self.run_copy(mutate), 1)


if __name__ == "__main__":
    unittest.main()
