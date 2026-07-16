#!/usr/bin/env python3

from __future__ import annotations

import importlib.util
import tempfile
import unittest
from pathlib import Path


SCRIPT = Path(__file__).with_name("qualify-vmware-artifacts.py")
SPEC = importlib.util.spec_from_file_location("qualify_vmware_artifacts", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
qualifier = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(qualifier)


class VMwareArtifactQualifierTests(unittest.TestCase):
    def test_number_accepts_decimal_and_hex(self) -> None:
        self.assertEqual(qualifier.number("4096"), 4096)
        self.assertEqual(qualifier.number("0x1000"), 4096)

    def test_summary_reports_ordered_latency_statistics(self) -> None:
        result = qualifier.summary([4.0, 1.0, 3.0, 2.0])
        self.assertEqual(result["mean_ms"], 2.5)
        self.assertEqual(result["median_ms"], 2.5)
        self.assertEqual(result["minimum_ms"], 1.0)
        self.assertEqual(result["maximum_ms"], 4.0)

    def test_generated_fixtures_are_deterministic_and_nonempty(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            vmem, core = qualifier.create_fixtures(root)
            self.assertEqual(vmem.stat().st_size, 4 * 1024 * 1024)
            self.assertGreater(core.stat().st_size, vmem.stat().st_size)
            self.assertEqual(vmem.read_bytes()[:251], bytes(range(251)))
            self.assertEqual(core.read_bytes()[:4], b"\x7fELF")


if __name__ == "__main__":
    unittest.main()
