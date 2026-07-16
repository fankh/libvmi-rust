import hashlib
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


SCRIPT = Path(__file__).with_name("verify-fixtures.py")


def manifest(digest, size, path="corpus/target/seed"):
    return f'''schema_version = 1
[[fixtures]]
path = "{path}"
sha256 = "{digest}"
size = {size}
license = "MIT OR Apache-2.0"
provenance = "generated test seed"
generator = "unit-test-v1"
architecture = "unknown"
endianness = "little"
page_size = 0
physical_ranges = "not applicable"
expected = "accepted safely"
'''


class FixtureManifestTests(unittest.TestCase):
    def run_validator(self, mutate=None):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            seed = root / "corpus" / "target" / "seed"
            seed.parent.mkdir(parents=True)
            seed.write_bytes(b"seed")
            digest = hashlib.sha256(b"seed").hexdigest()
            document = manifest(digest, 4)
            if mutate:
                document = mutate(document, root)
            manifest_path = root / "fixtures.toml"
            manifest_path.write_text(document, encoding="utf-8")
            return subprocess.run(
                [sys.executable, SCRIPT, "--root", root, "--manifest", manifest_path],
                capture_output=True,
                text=True,
                check=False,
            )

    def test_accepts_complete_matching_manifest(self):
        completed = self.run_validator()
        self.assertEqual(completed.returncode, 0, completed.stderr)

    def test_rejects_hash_drift(self):
        completed = self.run_validator(lambda text, _: text.replace("sha256 = \"", "sha256 = \"0"))
        self.assertNotEqual(completed.returncode, 0)
        self.assertIn("sha256", completed.stderr.lower())

    def test_rejects_undeclared_file(self):
        def add_file(text, root):
            (root / "corpus" / "extra").write_bytes(b"extra")
            return text
        completed = self.run_validator(add_file)
        self.assertNotEqual(completed.returncode, 0)
        self.assertIn("not declared", completed.stderr)

    def test_rejects_path_escape(self):
        completed = self.run_validator(
            lambda text, _: text.replace("corpus/target/seed", "../outside")
        )
        self.assertNotEqual(completed.returncode, 0)
        self.assertIn("escapes root", completed.stderr)


if __name__ == "__main__":
    unittest.main()
