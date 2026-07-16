#!/usr/bin/env python3
"""Validate the committed provider support contract without third-party modules."""

from __future__ import annotations

import re
import sys
import tomllib
from pathlib import Path


ROOT = Path(__file__).resolve().parent.parent
MATRIX = ROOT / "support-matrix.toml"
EXPECTED_PROVIDER_IDS = {
    "bhyve",
    "bhyve-core",
    "cloud-hypervisor",
    "fake-read-only",
    "firecracker",
    "hyperv",
    "hyperv-core",
    "qemu-qmp",
    "raw-dump",
    "snapshot-manifest",
    "virtualbox",
    "virtualbox-core",
    "vmware",
    "vmware-core",
    "xen",
}
MATURITIES = {"supported", "preview", "experimental", "compile-only"}
V1_TARGETS = {"supported", "preview", "experimental", "internal"}
PLATFORMS = {"linux", "windows", "macos", "freebsd"}
CAPABILITIES = {
    "acquisition",
    "control",
    "events",
    "memory_read",
    "memory_write",
    "register_read",
    "register_write",
    "views",
}
ID_PATTERN = re.compile(r"^[a-z0-9]+(?:-[a-z0-9]+)*$")


def fail(message: str) -> None:
    raise ValueError(f"{MATRIX.name}: {message}")


def nonempty_string(provider_id: str, field: str, value: object) -> None:
    if not isinstance(value, str) or not value.strip():
        fail(f"provider {provider_id!r} has invalid {field}")


def main() -> int:
    try:
        document = tomllib.loads(MATRIX.read_text(encoding="utf-8"))
        if document.get("schema_version") != 2:
            fail("schema_version must equal 2")
        if set(document) != {"schema_version", "providers"}:
            fail("top-level keys must be schema_version and providers")

        providers = document.get("providers")
        if not isinstance(providers, list) or not providers:
            fail("providers must be a non-empty array")

        seen: set[str] = set()
        required = {
            "id",
            "implemented",
            "display_name",
            "maturity",
            "v1_target",
            "platforms",
            "capabilities",
            "mechanism",
        }
        allowed = required | {"optional_capabilities"}
        for provider in providers:
            if not isinstance(provider, dict):
                fail("each provider must be a table")
            missing = required - set(provider)
            unknown = set(provider) - allowed
            if missing or unknown:
                fail(f"provider table has missing={sorted(missing)} unknown={sorted(unknown)}")

            provider_id = provider["id"]
            if not isinstance(provider_id, str) or not ID_PATTERN.fullmatch(provider_id):
                fail(f"invalid provider id {provider_id!r}")
            if provider_id in seen:
                fail(f"duplicate provider id {provider_id!r}")
            seen.add(provider_id)

            if not isinstance(provider["implemented"], bool):
                fail(f"provider {provider_id!r} implemented must be boolean")
            nonempty_string(provider_id, "display_name", provider["display_name"])
            nonempty_string(provider_id, "mechanism", provider["mechanism"])
            if provider["maturity"] not in MATURITIES:
                fail(f"provider {provider_id!r} has unknown maturity {provider['maturity']!r}")
            if provider["v1_target"] not in V1_TARGETS:
                fail(f"provider {provider_id!r} has unknown v1 target {provider['v1_target']!r}")
            if provider["v1_target"] == "internal" and provider_id != "fake-read-only":
                fail(f"provider {provider_id!r} cannot use the internal v1 target")

            platforms = provider["platforms"]
            if not isinstance(platforms, list) or not platforms:
                fail(f"provider {provider_id!r} platforms must be non-empty")
            if len(platforms) != len(set(platforms)):
                fail(f"provider {provider_id!r} repeats a platform")
            unknown_platforms = set(platforms) - PLATFORMS
            if unknown_platforms:
                fail(
                    f"provider {provider_id!r} has unknown platforms "
                    f"{sorted(unknown_platforms)}"
                )

            capabilities = provider["capabilities"]
            if not isinstance(capabilities, list) or not capabilities:
                fail(f"provider {provider_id!r} capabilities must be non-empty")
            if len(capabilities) != len(set(capabilities)):
                fail(f"provider {provider_id!r} repeats a capability")
            unknown_capabilities = set(capabilities) - CAPABILITIES
            if unknown_capabilities:
                fail(
                    f"provider {provider_id!r} has unknown capabilities "
                    f"{sorted(unknown_capabilities)}"
                )

            optional = provider.get("optional_capabilities", [])
            if not isinstance(optional, list) or any(
                not isinstance(item, str) or not item.strip() for item in optional
            ):
                fail(f"provider {provider_id!r} optional_capabilities must be strings")
            if len(optional) != len(set(optional)):
                fail(f"provider {provider_id!r} repeats an optional capability")

        missing_ids = EXPECTED_PROVIDER_IDS - seen
        unexpected_ids = seen - EXPECTED_PROVIDER_IDS
        if missing_ids or unexpected_ids:
            fail(
                f"provider inventory mismatch: missing={sorted(missing_ids)} "
                f"unexpected={sorted(unexpected_ids)}"
            )
    except (OSError, tomllib.TOMLDecodeError, ValueError) as error:
        print(error, file=sys.stderr)
        return 1

    print(f"support matrix verified: {len(providers)} providers")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
