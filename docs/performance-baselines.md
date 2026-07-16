# Performance Baselines

Performance results are meaningful only when the hardware, operating system,
toolchain, build profile, and workload are held constant. The repository
therefore emits machine-readable local results but does not compare timing
between unrelated CI hosts.

## Core Microbenchmarks

Run the release-mode harness from the workspace root:

```console
bash scripts/run-benchmarks.sh
```

The output is written to `target/vmi-benchmark.json`. Set
`VMI_BENCH_ITERATIONS` to control the sample count. The default is 2,000
operations after up to 100 untimed warm-up operations.

The harness currently measures:

- 4 KiB physical reads through the deterministic fake provider, including
  output allocation and the provider contract boundary;
- cached virtual-to-physical translation through `VmiSession`;
- physical-read throughput derived from the same elapsed duration.

To compare a result with a baseline captured on the same machine:

```console
VMI_BENCH_BASELINE=baseline.json bash scripts/run-benchmarks.sh
```

The default limit is a 10% regression. Override it with
`VMI_BENCH_MAX_REGRESSION` only for an intentional experiment. Latency metrics
regress when they increase; throughput regresses when it decreases. The
comparator validates its input schema and fails closed on missing, unknown,
zero, or negative metrics.

## Initial Measurement

The first local Windows release measurement on 2026-07-14 used 10,000 timed
iterations:

| Metric | Result |
| --- | ---: |
| 4 KiB physical-read latency | 3,548.620 ns/op |
| Physical-read throughput | 1,100.780 MiB/s |
| Cached translation latency | 19.550 ns/op |

This is a reproducibility record, not a cross-machine release threshold. Live
hypervisor attach, acquisition, and event-latency baselines require dedicated
QEMU/Xen/VirtualBox lab hosts and are tracked separately from deterministic
portable-core microbenchmarks.
