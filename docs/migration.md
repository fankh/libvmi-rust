# Migration Guide

When constructing raw artifacts from untrusted or externally supplied mapping
metadata, prefer `SnapshotBundle::try_from_raw`. It rejects a base/length pair
that extends beyond the 64-bit guest-physical address space. The existing
`SnapshotBundle::from_raw` remains available for source compatibility and for
already-validated in-memory mappings.

## Unreleased to 1.0.0

The initial release establishes:

- `vmi` as the curated facade;
- explicit physical and virtual address types;
- attach-time capability negotiation;
- explicit guest byte order for scalar reads;
- immutable snapshot consistency metadata;
- Rust 1.85 as the minimum supported Rust version;
- C ABI version 1 for immutable artifact reads.
- stable error categories through `VmiError::kind()`, including typed timeout
  and cancellation results;
- cooperative cancellation through `driver::CancellationToken` and the
  `*_cancellable` driver methods.

There is no earlier released Rust API to migrate from. Applications prototyped
against individual workspace crates should move common imports to `vmi` or
`vmi::prelude`, retain provider-specific imports only when configuring a
backend, and request every required capability in `AttachRequest`.

Future release sections must include mechanical before/after examples for every
breaking public API change and note any capability, consistency, configuration,
MSRV, artifact-format, or C ABI behavior change.
