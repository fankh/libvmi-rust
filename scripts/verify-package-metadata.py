#!/usr/bin/env python3
"""Validate release metadata for every workspace package."""

from __future__ import annotations

import json
import subprocess
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parent.parent
EXPECTED_LICENSE = "MIT OR Apache-2.0"
EXPECTED_REPOSITORY = "https://github.com/fankh/new-research"
EXPECTED_RUST_VERSION = "1.85"
EXPECTED_KEYWORDS = {"forensics", "memory", "virtualization", "vmi"}
EXPECTED_CATEGORIES = {"development-tools::debugging", "virtualization"}


def validate_package(package: dict[str, object], root: Path = ROOT) -> list[str]:
    """Return human-readable metadata violations for one Cargo package."""
    name = package.get("name", "<unnamed>")
    errors: list[str] = []

    expected = {
        "license": EXPECTED_LICENSE,
        "repository": EXPECTED_REPOSITORY,
        "rust_version": EXPECTED_RUST_VERSION,
    }
    for field, value in expected.items():
        if package.get(field) != value:
            errors.append(f"{name}: {field} must be {value!r}")

    description = package.get("description")
    if not isinstance(description, str) or not description.strip():
        errors.append(f"{name}: description must be non-empty")

    keywords = package.get("keywords")
    if not isinstance(keywords, list) or set(keywords) != EXPECTED_KEYWORDS:
        errors.append(f"{name}: keywords must match the workspace discovery policy")

    categories = package.get("categories")
    if not isinstance(categories, list) or set(categories) != EXPECTED_CATEGORIES:
        errors.append(f"{name}: categories must match the workspace discovery policy")

    readme = package.get("readme")
    if not isinstance(readme, str) or not readme:
        errors.append(f"{name}: readme must be configured")
    else:
        readme_path = Path(readme)
        if not readme_path.is_absolute():
            manifest = package.get("manifest_path")
            base = Path(manifest).parent if isinstance(manifest, str) else root
            readme_path = base / readme_path
        if not readme_path.is_file():
            errors.append(f"{name}: readme does not exist: {readme_path}")

    return errors


def validate_internal_dependencies(packages: list[dict[str, object]]) -> list[str]:
    """Validate local workspace edges and return dependency-policy violations."""
    by_name = {package["name"]: package for package in packages}
    errors: list[str] = []
    for package in packages:
        for dependency in package.get("dependencies", []):
            name = dependency.get("name")
            if name not in by_name:
                continue
            expected = f"={by_name[name]['version']}"
            if dependency.get("req") != expected:
                errors.append(
                    f"{package['name']}: workspace dependency {name} must use {expected!r}"
                )
            if not dependency.get("path"):
                errors.append(
                    f"{package['name']}: workspace dependency {name} must retain a local path"
                )
    return errors


def publication_order(packages: list[dict[str, object]]) -> list[str]:
    """Return a deterministic dependency-first order or raise for an internal cycle."""
    names = {package["name"] for package in packages}
    dependencies = {
        package["name"]: {
            dependency["name"]
            for dependency in package.get("dependencies", [])
            if dependency.get("name") in names
        }
        for package in packages
    }
    order: list[str] = []
    remaining = set(names)
    while remaining:
        ready = sorted(name for name in remaining if not (dependencies[name] & remaining))
        if not ready:
            cycle = ", ".join(sorted(remaining))
            raise ValueError(f"workspace dependency cycle prevents publication: {cycle}")
        order.extend(ready)
        remaining.difference_update(ready)
    return order


def main() -> int:
    result = subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--no-deps"],
        cwd=ROOT,
        check=False,
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        print(result.stderr, file=sys.stderr, end="")
        return result.returncode

    document = json.loads(result.stdout)
    members = set(document["workspace_members"])
    packages = [package for package in document["packages"] if package["id"] in members]
    errors = [error for package in packages for error in validate_package(package)]
    errors.extend(validate_internal_dependencies(packages))
    try:
        order = publication_order(packages)
    except ValueError as error:
        errors.append(str(error))
    if errors:
        print("\n".join(errors), file=sys.stderr)
        return 1

    print(f"package metadata verified: {len(packages)} workspace crates")
    print("publication order: " + " ".join(order))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
