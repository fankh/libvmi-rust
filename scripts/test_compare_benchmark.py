import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


SCRIPT = Path(__file__).with_name("compare-benchmark.py")


def result(metrics):
    return json.dumps({"schema": 1, "iterations": 100, "metrics": metrics})


class BenchmarkComparisonTests(unittest.TestCase):
    def run_comparison(self, baseline, current):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "baseline.json").write_text(result(baseline), encoding="utf-8")
            (root / "current.json").write_text(result(current), encoding="utf-8")
            return subprocess.run(
                [sys.executable, SCRIPT, root / "baseline.json", root / "current.json"],
                capture_output=True,
                text=True,
                check=False,
            )

    def test_accepts_improvement_and_small_regression(self):
        completed = self.run_comparison(
            {"raw_read_4k_ns": 100, "raw_read_mib_s": 100, "cached_translation_ns": 50},
            {"raw_read_4k_ns": 105, "raw_read_mib_s": 95, "cached_translation_ns": 45},
        )
        self.assertEqual(completed.returncode, 0, completed.stdout + completed.stderr)

    def test_rejects_latency_and_throughput_regressions(self):
        completed = self.run_comparison(
            {"raw_read_4k_ns": 100, "raw_read_mib_s": 100, "cached_translation_ns": 50},
            {"raw_read_4k_ns": 120, "raw_read_mib_s": 80, "cached_translation_ns": 60},
        )
        self.assertNotEqual(completed.returncode, 0)
        self.assertIn("raw_read_4k_ns regressed", completed.stdout)
        self.assertIn("raw_read_mib_s regressed", completed.stdout)

    def test_rejects_invalid_schema(self):
        completed = self.run_comparison(
            {"raw_read_4k_ns": 100},
            {"raw_read_4k_ns": 100},
        )
        self.assertNotEqual(completed.returncode, 0)


if __name__ == "__main__":
    unittest.main()
