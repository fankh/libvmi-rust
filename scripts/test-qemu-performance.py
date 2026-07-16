#!/usr/bin/env python3

from __future__ import annotations

import json
import os
import pathlib
import platform
import socket
import statistics
import subprocess
import tempfile
import time


ROOT = pathlib.Path(__file__).resolve().parents[1]
OUTPUT = pathlib.Path(
    os.environ.get("VMI_QEMU_PERF_OUTPUT", ROOT / "target/qemu-performance.json")
)
ITERATIONS = int(os.environ.get("VMI_QEMU_PERF_ITERATIONS", "100"))
ACQUIRE_ITERATIONS = int(os.environ.get("VMI_QEMU_ACQUIRE_ITERATIONS", "8"))
ACQUIRE_BYTES = int(os.environ.get("VMI_QEMU_ACQUIRE_BYTES", str(1024 * 1024)))
QEMU = os.environ.get("VMI_QEMU_BINARY", "qemu-system-x86_64")


def percentile(values: list[float], percent: float) -> float:
    ordered = sorted(values)
    index = min(len(ordered) - 1, max(0, int((len(ordered) - 1) * percent)))
    return ordered[index]


def summary(values: list[float]) -> dict[str, float]:
    return {
        "mean_ms": round(statistics.fmean(values), 3),
        "median_ms": round(statistics.median(values), 3),
        "p95_ms": round(percentile(values, 0.95), 3),
        "minimum_ms": round(min(values), 3),
        "maximum_ms": round(max(values), 3),
    }


def measured(command: list[str]) -> tuple[float, str]:
    started = time.perf_counter_ns()
    result = subprocess.run(command, check=True, text=True, capture_output=True)
    elapsed_ms = (time.perf_counter_ns() - started) / 1_000_000
    return elapsed_ms, result.stdout.strip()


def wait_for_qmp(process: subprocess.Popen[bytes]) -> None:
    for _ in range(200):
        if process.poll() is not None:
            raise RuntimeError("QEMU exited before its QMP endpoint became ready")
        try:
            with socket.create_connection(("127.0.0.1", 4444), timeout=0.1):
                return
        except OSError:
            time.sleep(0.05)
    raise TimeoutError("QEMU QMP endpoint did not become ready")


def main() -> None:
    if ITERATIONS < 1 or ACQUIRE_ITERATIONS < 1 or ACQUIRE_BYTES < 1:
        raise ValueError("performance iteration counts and acquisition size must be positive")

    subprocess.run(["cargo", "build", "--locked", "-p", "vmi-cli"], cwd=ROOT, check=True)
    cli = ROOT / "target/debug/vmi-cli"
    with tempfile.TemporaryDirectory() as temporary:
        work = pathlib.Path(temporary)
        with (work / "qemu.stdout").open("wb") as stdout, (work / "qemu.stderr").open(
            "wb"
        ) as stderr:
            process = subprocess.Popen(
                [
                    QEMU,
                    "-machine",
                    "q35,accel=tcg",
                    "-cpu",
                    "qemu64",
                    "-smp",
                    "2",
                    "-m",
                    "64",
                    "-nodefaults",
                    "-display",
                    "none",
                    "-serial",
                    "none",
                    "-monitor",
                    "none",
                    "-qmp",
                    "tcp:127.0.0.1:4444,server=on,wait=off",
                ],
                stdout=stdout,
                stderr=stderr,
            )
            try:
                wait_for_qmp(process)
                endpoint = "127.0.0.1:4444"
                pause = subprocess.run(
                    [str(cli), "qemu-pause", endpoint],
                    check=True,
                    text=True,
                    capture_output=True,
                )
                if pause.stdout.strip() != "Paused":
                    raise RuntimeError("QEMU did not enter the paused state")
                measured([str(cli), "qemu-status", endpoint])
                measured([str(cli), "qemu-read", endpoint, "0", "4096"])

                status_ms = [
                    measured([str(cli), "qemu-status", endpoint])[0]
                    for _ in range(ITERATIONS)
                ]
                read_results = [
                    measured([str(cli), "qemu-read", endpoint, "0", "4096"])
                    for _ in range(ITERATIONS)
                ]
                read_ms = [result[0] for result in read_results]
                if len({result[1] for result in read_results}) != 1:
                    raise RuntimeError("live memory reads changed during the benchmark")

                acquire_ms: list[float] = []
                for index in range(ACQUIRE_ITERATIONS):
                    destination = work / f"acquire-{index}.bin"
                    elapsed, _ = measured(
                        [
                            str(cli),
                            "qemu-acquire",
                            endpoint,
                            str(destination),
                            "0",
                            str(ACQUIRE_BYTES),
                        ]
                    )
                    if destination.stat().st_size != ACQUIRE_BYTES:
                        raise RuntimeError("QEMU acquisition returned an unexpected size")
                    acquire_ms.append(elapsed)
            finally:
                process.terminate()
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()

        qemu_version = subprocess.run(
            [QEMU, "--version"], check=True, text=True, capture_output=True
        ).stdout.splitlines()[0]
        mean_acquire_seconds = statistics.fmean(acquire_ms) / 1000
        document = {
            "schema": 1,
            "provider": "qemu-qmp",
            "host": platform.platform(),
            "qemu": qemu_version,
            "configuration": {
                "acceleration": "tcg",
                "vcpus": 2,
                "memory_mib": 64,
                "command_iterations": ITERATIONS,
                "acquire_iterations": ACQUIRE_ITERATIONS,
                "acquire_bytes": ACQUIRE_BYTES,
            },
            "metrics": {
                "status_command": summary(status_ms),
                "read_4k_command": summary(read_ms),
                "acquire_1mib_command": summary(acquire_ms),
                "acquire_mib_s": round(
                    (ACQUIRE_BYTES / (1024 * 1024)) / mean_acquire_seconds, 3
                ),
            },
            "passed": True,
        }
        OUTPUT.parent.mkdir(parents=True, exist_ok=True)
        OUTPUT.write_text(json.dumps(document, indent=2) + "\n", encoding="utf-8")
        print(f"QEMU performance evidence written to {OUTPUT}")


if __name__ == "__main__":
    main()
