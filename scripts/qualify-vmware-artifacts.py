#!/usr/bin/env python3

from __future__ import annotations

import argparse
import hashlib
import json
import os
import pathlib
import platform
import statistics
import struct
import subprocess
import tempfile
import time


ROOT = pathlib.Path(__file__).resolve().parents[1]


def number(value: str) -> int:
    return int(value, 0)


def percentile(values: list[float], fraction: float) -> float:
    ordered = sorted(values)
    return ordered[int((len(ordered) - 1) * fraction)]


def summary(values: list[float]) -> dict[str, float]:
    return {
        "mean_ms": round(statistics.fmean(values), 3),
        "median_ms": round(statistics.median(values), 3),
        "p95_ms": round(percentile(values, 0.95), 3),
        "minimum_ms": round(min(values), 3),
        "maximum_ms": round(max(values), 3),
    }


def digest(path: pathlib.Path) -> str:
    result = hashlib.sha256()
    with path.open("rb") as source:
        while chunk := source.read(1024 * 1024):
            result.update(chunk)
    return result.hexdigest()


def run(command: list[str], expected_success: bool = True) -> tuple[float, str]:
    started = time.perf_counter_ns()
    result = subprocess.run(command, text=True, capture_output=True)
    elapsed_ms = (time.perf_counter_ns() - started) / 1_000_000
    if (result.returncode == 0) != expected_success:
        raise RuntimeError(
            f"unexpected command result {result.returncode}: "
            f"{' '.join(command)}\n{result.stderr}"
        )
    output = result.stdout if expected_success else result.stderr
    return elapsed_ms, output.strip()


def fixture_bytes(length: int) -> bytes:
    return bytes(index % 251 for index in range(length))


def create_fixtures(directory: pathlib.Path) -> tuple[pathlib.Path, pathlib.Path]:
    directory.mkdir(parents=True, exist_ok=True)
    payload = fixture_bytes(4 * 1024 * 1024)
    vmem = directory / "qualified-synthetic.vmem"
    vmem.write_bytes(payload)

    core = directory / "qualified-synthetic.core"
    header_size = 64
    program_header_size = 56
    payload_offset = header_size + program_header_size
    elf = bytearray(payload_offset + len(payload))
    elf[:16] = b"\x7fELF\x02\x01\x01" + bytes(9)
    struct.pack_into("<HHIQQQIHHHHHH", elf, 16, 4, 62, 1, 0, header_size, 0, 0, header_size, program_header_size, 1, 0, 0, 0)
    struct.pack_into(
        "<IIQQQQQQ",
        elf,
        header_size,
        1,
        0,
        payload_offset,
        0,
        0x1000,
        len(payload),
        len(payload),
        4096,
    )
    elf[payload_offset:] = payload
    core.write_bytes(elf)
    return vmem, core


def qualify(
    cli: pathlib.Path,
    kind: str,
    path: pathlib.Path,
    physical_base: int,
    address: int,
    length: int,
    iterations: int,
    source: str,
) -> dict[str, object]:
    if kind == "vmem":
        command = [
            str(cli),
            "read-vmware-vmem",
            str(path),
            hex(physical_base),
            hex(address),
            str(length),
        ]
    else:
        command = [str(cli), "read-vmware-core", str(path), hex(address), str(length)]

    _, reference = run(command)
    timings: list[float] = []
    for _ in range(iterations):
        elapsed, output = run(command)
        if output != reference:
            raise RuntimeError(f"{kind} reads were not repeatable")
        timings.append(elapsed)

    checks: dict[str, bool] = {
        "artifact is non-empty": path.stat().st_size > 0,
        "repeated reads are byte-identical": True,
        "requested byte count returned": sum(
            len(line.split(": ", 1)[1].split()) for line in reference.splitlines()
        )
        == length,
    }
    if kind == "vmem":
        outside = physical_base + path.stat().st_size
        _, error = run(
            [
                str(cli),
                "read-vmware-vmem",
                str(path),
                hex(physical_base),
                hex(outside),
                "1",
            ],
            expected_success=False,
        )
        checks["out-of-range read fails closed"] = "read failed" in error

    return {
        "kind": kind,
        "source": source,
        "file_name": path.name,
        "size_bytes": path.stat().st_size,
        "sha256": digest(path),
        "physical_base": physical_base if kind == "vmem" else None,
        "probe_address": address,
        "probe_length": length,
        "probe_sha256": hashlib.sha256(reference.encode()).hexdigest(),
        "iterations": iterations,
        "latency": summary(timings),
        "checks": checks,
        "passed": all(checks.values()),
    }


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--vmem", type=pathlib.Path)
    parser.add_argument("--converted-core", type=pathlib.Path)
    parser.add_argument("--generate-fixtures", type=pathlib.Path)
    parser.add_argument("--physical-base", type=number, default=0)
    parser.add_argument("--vmem-address", type=number)
    parser.add_argument("--core-address", type=number, default=0x1000)
    parser.add_argument("--length", type=number, default=4096)
    parser.add_argument("--iterations", type=int, default=25)
    parser.add_argument("--source", choices=["vendor-captured", "synthetic"], default="vendor-captured")
    parser.add_argument("--output", type=pathlib.Path, required=True)
    args = parser.parse_args()
    if args.iterations < 1 or args.length < 1:
        parser.error("iterations and length must be positive")
    if args.generate_fixtures:
        if args.vmem or args.converted_core:
            parser.error("generated fixtures cannot be combined with supplied artifacts")
        args.vmem, args.converted_core = create_fixtures(args.generate_fixtures)
        args.source = "synthetic"
    if not args.vmem and not args.converted_core:
        parser.error("provide an artifact or --generate-fixtures")

    subprocess.run(["cargo", "build", "--locked", "-p", "vmi-cli"], cwd=ROOT, check=True)
    executable = "vmi-cli.exe" if os.name == "nt" else "vmi-cli"
    cli = ROOT / "target/debug" / executable
    artifacts = []
    if args.vmem:
        address = args.vmem_address
        if address is None:
            address = args.physical_base
        artifacts.append(
            qualify(
                cli,
                "vmem",
                args.vmem.resolve(),
                args.physical_base,
                address,
                args.length,
                args.iterations,
                args.source,
            )
        )
    if args.converted_core:
        artifacts.append(
            qualify(
                cli,
                "converted-core",
                args.converted_core.resolve(),
                0,
                args.core_address,
                args.length,
                args.iterations,
                args.source,
            )
        )

    document = {
        "schema": 1,
        "provider_scope": "offline-vmware-artifacts",
        "host": platform.platform(),
        "artifacts": artifacts,
        "passed": all(artifact["passed"] for artifact in artifacts),
    }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(document, indent=2) + "\n", encoding="utf-8")
    print(f"VMware artifact evidence written to {args.output}")
    if not document["passed"]:
        raise SystemExit(1)


if __name__ == "__main__":
    main()
