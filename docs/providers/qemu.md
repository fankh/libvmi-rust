# QEMU Provider Qualification

## v1 Contract

The `qemu-qmp` provider targets the Supported tier for QEMU 11.0.2 on Linux hosts.
TCP QMP is portable and Unix QMP sockets are supported on Unix hosts, but Windows
and macOS remain compatibility-build platforms until the same real-host matrix is
recorded there. AMD64 guests are qualified; other guest architectures fail outside
the provider's advertised target descriptor.

The default connector advertises physical-memory read, register read, VM control,
acquisition, and asynchronous events. Register write is advertised only when an
explicit GDB endpoint is configured. Consistency is `LiveBestEffort`; callers that
need a coherent image must pause the VM or acquire an immutable core.

## Evidence

- QMP greeting, capability negotiation, correlated replies, interleaved events,
  framing limits, queue bounds, request-ID exhaustion, protocol errors, and total
  deadlines have deterministic transport tests.
- HMP physical-memory and register text is decoded at fail-closed boundaries.
- GDB register selection, widths, checksums, acknowledgement handling, read-back
  verification, slow-byte deadlines, and malformed responses are tested.
- Acquisition rejects zero/overflowing ranges, control characters, non-UTF-8 or
  ambiguous destinations, and existing local outputs before monitor submission.
- A real QEMU 11 Linux run validated status/control, two-vCPU register access,
  physical reads, ordered events, `pmemsave`, and ELF core acquisition and re-read.
- The optimized QEMU suite passes a 30-run local stress loop after normalizing
  command-boundary timeout classification.

## Remaining Release Evidence

Before changing the provider maturity to Supported and closing the gate:

1. Run the real-host workflow on the pinned Linux QEMU 11.0.2 build from a clean CI
   runner and retain its machine-readable transcript as a release artifact.
2. Execute a minimum one-hour reconnect/control/read/acquisition soak with bounded
   memory and file-descriptor growth.
3. Exercise abrupt QEMU exit, QMP disconnect, stalled acquisition, and destination
   permission/exhaustion failures, confirming bounded recovery and typed errors.
4. Record whether Windows and macOS real-host validation is included in v1; until
   then they remain build-compatible rather than operationally supported.

## Known Limits

- QMP/HMP provides events, not Xen-style blocking guest-access events or alternate
  memory views.
- QMP writes acquisition files in the QEMU host namespace. Remote publication
  cannot provide an atomic no-replace guarantee, so operators must isolate and
  pre-authorize the destination directory.
- Cancellation is cooperative at the public operation boundary; current QMP and
  GDB socket deadlines bound stalled calls, but cancellation does not interrupt a
  kernel socket call immediately.

Run the repeatable real-transport workflow with
`bash scripts/test-qemu-integration.sh`. The transcript is written to
`target/qemu-qualification.json` by default. CI runs a 30-second resource-budgeted
smoke soak. Release candidates use `VMI_QEMU_SOAK_SECONDS=3600` for the required
one-hour run; RSS and file-descriptor growth limits are configurable through
`VMI_QEMU_MAX_RSS_GROWTH_KIB` and `VMI_QEMU_MAX_FD_GROWTH`.

`bash scripts/build-qemu.sh` builds the reviewed upstream QEMU 11.0.2 source
tarball with a pinned SHA-256 and a minimal x86_64 TCG configuration. CI caches
the installation but validates its reported version before use.
