#!/usr/bin/env python3
import argparse
import json
from pathlib import Path


def load_result(path: Path) -> dict[str, float]:
    lines = [line for line in path.read_text(encoding="utf-8").splitlines() if line.startswith("{")]
    if not lines:
        raise ValueError(f"{path}: no benchmark JSON object found")
    document = json.loads(lines[-1])
    if document.get("schema") != 1 or not isinstance(document.get("metrics"), dict):
        raise ValueError(f"{path}: unsupported benchmark schema")
    metrics = document["metrics"]
    required = {"raw_read_4k_ns", "raw_read_mib_s", "cached_translation_ns"}
    if set(metrics) != required:
        raise ValueError(f"{path}: expected metrics {sorted(required)}")
    if any(not isinstance(value, (int, float)) or value <= 0 for value in metrics.values()):
        raise ValueError(f"{path}: metrics must be positive numbers")
    return metrics


def regressions(baseline: dict[str, float], current: dict[str, float], limit: float) -> list[str]:
    failures = []
    for name in ("raw_read_4k_ns", "cached_translation_ns"):
        change = (current[name] / baseline[name] - 1.0) * 100.0
        if change > limit:
            failures.append(f"{name} regressed by {change:.2f}% (limit {limit:.2f}%)")
    throughput_change = (1.0 - current["raw_read_mib_s"] / baseline["raw_read_mib_s"]) * 100.0
    if throughput_change > limit:
        failures.append(
            f"raw_read_mib_s regressed by {throughput_change:.2f}% (limit {limit:.2f}%)"
        )
    return failures


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("baseline", type=Path)
    parser.add_argument("current", type=Path)
    parser.add_argument("--maximum-regression", type=float, default=10.0)
    args = parser.parse_args()
    if args.maximum_regression < 0:
        parser.error("--maximum-regression must be non-negative")
    failures = regressions(
        load_result(args.baseline), load_result(args.current), args.maximum_regression
    )
    if failures:
        for failure in failures:
            print(failure)
        return 1
    print(f"benchmark comparison passed: maximum regression {args.maximum_regression:.2f}%")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
