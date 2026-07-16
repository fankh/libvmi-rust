#!/usr/bin/env python3

from __future__ import annotations

import importlib.util
import json
import tempfile
import unittest
from pathlib import Path


SCRIPT = Path(__file__).with_name("normalize-sbom.py")
SPEC = importlib.util.spec_from_file_location("normalize_sbom", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
normalizer = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(normalizer)


class NormalizeSbomTests(unittest.TestCase):
    def test_removes_volatile_fields_and_canonicalizes_root(self) -> None:
        root_uri = "file:///temporary/checkout"
        document = {
            "serialNumber": "urn:uuid:random",
            "metadata": {"timestamp": "now", "component": {"name": "vmi"}},
            "components": [{"bom-ref": f"path+{root_uri}/crates/vmi"}],
        }
        output = normalizer.normalize(document, root_uri)
        self.assertNotIn("serialNumber", output)
        self.assertNotIn("timestamp", output["metadata"])
        self.assertEqual(
            output["components"][0]["bom-ref"], "path+file:///workspace/crates/vmi"
        )

    def test_file_output_is_idempotent_and_sorted(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            path = root / "sample.cdx.json"
            path.write_text(json.dumps({"z": 1, "a": root.as_uri()}), encoding="utf-8")
            normalizer.normalize_file(path, root)
            once = path.read_bytes()
            normalizer.normalize_file(path, root)
            self.assertEqual(path.read_bytes(), once)
            self.assertTrue(once.startswith(b'{\n  "a":'))


if __name__ == "__main__":
    unittest.main()
