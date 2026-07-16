# Xen Provider Qualification

## v1 Contract

The `xen` provider targets Preview on Linux AMD64 hosts. Its baseline connector
advertises VM control and core acquisition through `xl`. Direct memory read/write,
register access, register write, and events are advertised only when their explicit
native or injected transport is configured.

No alternate-memory-view capability is advertised. The portable event transport
accepts typed events from a native adapter, but this workspace does not bundle a
vm_event ring implementation for v1.

## Capability Evidence

| Capability | Activation | Evidence |
| --- | --- | --- |
| Control | Baseline `xl` transport | State parsing, pause/resume commands, timeout and bounded-output tests |
| Acquisition | Baseline `xl dump-core` | Snapshot/range extraction, overflow, cleanup, synchronization, and atomic no-clobber tests |
| Memory read/write | `with_xenctrl` or memory transport | Cross-page contract tests and reviewed dynamically loaded foreign-page mapping boundary |
| Register read | `with_xenctx` or CPU transport | Bounded parser tests and deterministic multi-vCPU transport tests |
| Register write | Writable CPU transport | Capability negotiation, write/read behavior, and fail-closed read-only session tests |
| Events | vm_event transport | Capability negotiation, timeout propagation, and typed event delivery tests |

Every subprocess has a total deadline, bounded stdout/stderr capture, and
kill/reap behavior on timeout. Domain names reject empty, option-like, and control
character values before process creation. Native page addressing, GFN conversion,
buffer progress, and mapping cleanup use checked operations.

## Known Limits and Release Evidence

- Preview is currently qualified by deterministic transport contracts and portable
  Linux compilation. A real Xen dom0 transcript remains required before promotion
  beyond Preview.
- The maintained Rust `libxen` bindings do not currently compile a compatible
  vm_event ring against the audited Debian Xen headers. Event support therefore
  requires an operator-supplied adapter implementing `XenEventTransport`.
- `xenctx` output is vendor text and supports only registers recognized by its
  fail-closed parser. Coherent bulk CPU snapshots are not exposed.
- Core acquisition temporarily writes a Xen core and is bounded by the destination
  filesystem. Operators must budget space for the full guest even for range reads.
- Cancellation is cooperative at operation boundaries; the configured subprocess
  timeout bounds stalled vendor tooling.

Promotion requires a pinned Xen release/hardware matrix covering control, direct
cross-page memory access, CPU state, core acquisition/re-read, domain destruction,
permission failures, and at least a 30-minute repeated-operation soak.
