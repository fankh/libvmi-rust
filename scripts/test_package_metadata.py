#!/usr/bin/env python3

from __future__ import annotations

import importlib.util
import tempfile
import unittest
from pathlib import Path


SCRIPT = Path(__file__).with_name("verify-package-metadata.py")
SPEC = importlib.util.spec_from_file_location("verify_package_metadata", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
validator = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(validator)


class PackageMetadataValidatorTests(unittest.TestCase):
    def package(self, readme: Path) -> dict[str, object]:
        return {
            "name": "vmi-example",
            "license": validator.EXPECTED_LICENSE,
            "repository": validator.EXPECTED_REPOSITORY,
            "rust_version": validator.EXPECTED_RUST_VERSION,
            "description": "Example package",
            "keywords": sorted(validator.EXPECTED_KEYWORDS),
            "categories": sorted(validator.EXPECTED_CATEGORIES),
            "readme": str(readme),
            "manifest_path": str(readme.parent / "Cargo.toml"),
            "version": "0.1.0",
            "dependencies": [],
        }

    def test_complete_metadata_passes(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            readme = Path(temporary) / "README.md"
            readme.write_text("# Example\n", encoding="utf-8")
            self.assertEqual(validator.validate_package(self.package(readme)), [])

    def test_missing_fields_and_readme_fail(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            package = self.package(Path(temporary) / "missing.md")
            package["license"] = None
            package["repository"] = ""
            package["rust_version"] = "1.84"
            package["description"] = " "
            package["keywords"] = []
            package["categories"] = []
            errors = validator.validate_package(package)
            self.assertEqual(len(errors), 7)

    def test_relative_readme_is_resolved_from_manifest(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            readme = root / "README.md"
            readme.write_text("# Example\n", encoding="utf-8")
            package = self.package(readme)
            package["readme"] = "README.md"
            self.assertEqual(validator.validate_package(package), [])

    def test_internal_dependencies_require_exact_versions_and_paths(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            readme = root / "README.md"
            readme.write_text("# Example\n", encoding="utf-8")
            dependency = self.package(readme)
            dependency["name"] = "vmi-dependency"
            consumer = self.package(readme)
            consumer["dependencies"] = [
                {"name": "vmi-dependency", "req": "0.1", "path": None}
            ]
            errors = validator.validate_internal_dependencies([dependency, consumer])
            self.assertEqual(len(errors), 2)

    def test_publication_order_is_dependency_first_and_deterministic(self) -> None:
        packages = [
            {"name": "top", "dependencies": [{"name": "middle"}]},
            {"name": "leaf", "dependencies": []},
            {"name": "middle", "dependencies": [{"name": "leaf"}]},
        ]
        self.assertEqual(validator.publication_order(packages), ["leaf", "middle", "top"])

    def test_publication_order_rejects_cycles(self) -> None:
        packages = [
            {"name": "one", "dependencies": [{"name": "two"}]},
            {"name": "two", "dependencies": [{"name": "one"}]},
        ]
        with self.assertRaisesRegex(ValueError, "cycle"):
            validator.publication_order(packages)


if __name__ == "__main__":
    unittest.main()
