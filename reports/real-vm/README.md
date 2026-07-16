# Real VM Qualification Report

These results were captured on 2026-07-16 from a real headless x86_64 QEMU
virtual machine running under the local WSL2 Linux virtualization layer. The VM
used TCG acceleration, two vCPUs, 64 MiB of RAM, and QEMU 7.2.22.

## Feature Result

The feature qualification passed every semantic check in
`qemu-features.json`: execution state, register access, physical-memory reads,
pause/resume and event delivery, range and ELF-core acquisition, byte equality,
resource stability, and fail-closed disconnect handling. The 60-second soak
completed 474 iterations with zero observed RSS or file-descriptor growth.

## Performance Result

The performance qualification in `qemu-performance.json` paused the guest for
coherent repeated reads, then measured the complete CLI and Rust provider path.
Each sample includes a fresh process, QMP connection and negotiation, operation,
and clean disconnect.

| Operation | Samples | Mean | Median | p95 |
| --- | ---: | ---: | ---: | ---: |
| Status | 100 | 58.616 ms | 59.454 ms | 60.676 ms |
| 4 KiB physical read | 100 | 720.962 ms | 720.310 ms | 724.469 ms |
| 1 MiB range acquisition | 8 | 59.772 ms | 59.949 ms | 60.017 ms |

Mean 1 MiB acquisition throughput was 16.730 MiB/s. The 4 KiB read path uses
the QEMU human-monitor `xp` command and formats every returned byte, so it is not
comparable to persistent-session direct-memory transports. These measurements
are a local regression baseline, not a cross-host performance guarantee.

QEMU 11.0.2 remains the supported production qualification target. This local
QEMU 7.2.22 run supplies additional compatibility evidence and does not replace
the retained QEMU 11 release qualification.
