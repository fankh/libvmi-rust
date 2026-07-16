# Provider Authoring Guide

A provider consists of a `Connector`, which validates an attachment request,
and a `Session`, which exposes only the facets backed by the provider. Start
with the complete [`custom_provider`](../crates/vmi/examples/custom_provider.rs)
example; it compiles and runs in CI.

## Contract

1. Give the connector a stable, lowercase provider ID and an accurate maturity.
2. Advertise only capabilities whose corresponding session facets work.
3. During `connect`, calculate missing required capabilities and return
   `VmiError::AttachRejected` before performing analysis.
4. Apply `TargetSelector` when the provider exposes multiple or named targets.
5. Record the real guest architecture and consistency mode in `TargetDescriptor`.
6. Return `CapabilityMissing` for unadvertised optional facets by retaining the
   default `Session` methods.
7. Bound guest-derived lengths, counts, queues, and parser inputs. Use checked
   arithmetic and fallible allocation for untrusted sizes.
8. Keep native handles and unsafe code inside a narrowly reviewed boundary
   crate, with RAII cleanup and adjacent safety invariants.

## Capability Mapping

| Capability | Required session method |
| --- | --- |
| `MemoryRead` | `Session::memory` returning `MemoryAccess` |
| `MemoryWrite` | `Session::memory_write` returning `MemoryWriteAccess` |
| `RegisterRead` / `RegisterWrite` | `Session::cpu` returning `CpuAccess` |
| `Control` | `Session::control` returning `ControlAccess` |
| `Events` | `Session::events` returning `EventAccess` |
| `MemoryView` | `Session::views` returning `ViewAccess` |
| `Acquisition` | `Session::acquisition` returning `AcquisitionAccess` |

Every new provider must add a support-contract entry, focused positive and
negative tests, documentation of permissions and side effects, and appropriate
CI or lab verification. Capability claims are not complete until
`scripts/verify-support-matrix.py` and the generated matrix both pass.

Sparse-memory implementations must define deterministic partial-operation
semantics. The testkit contract covers empty and unaligned operations,
contiguous segment boundaries, holes, the final physical address, arithmetic
overflow, and partial read/write failure. Providers must also reject overlapping
or address-space-overflowing ranges before a session is exposed.

Acquisition output must be assembled and synchronized in a same-directory
temporary file, then atomically published. A failed acquisition must preserve
an existing destination and remove every temporary artifact; the fake provider
serves as the portable reference implementation for this contract.

External command transports must use a total operation deadline, cap durations
before computing an `Instant` deadline so overflow cannot disable enforcement, kill and reap
timed-out children, and drain stdout and stderr concurrently through bounded
readers. Checking output size only after `Command::output` returns does not meet
the resource-limit contract because allocation has already occurred.
Failure paths must also avoid waiting indefinitely for pipe EOF: a descendant
may retain an inherited handle after the direct child has been killed and reaped.

## Offline Example

The [`inspect_raw`](../crates/vmi/examples/inspect_raw.rs) example turns a flat
file into an immutable dump provider and reads physical memory through the same
public session API used by live providers:

```console
cargo run -p vmi --example inspect_raw -- memory.bin 0x1000 16
```
