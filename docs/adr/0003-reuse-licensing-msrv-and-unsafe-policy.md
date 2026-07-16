# ADR 0003: Reuse, Licensing, MSRV, and Unsafe Policy

Status: Accepted

Decision:

- Reuse `vmi-rs` and `memflow` through adapters where they fit the provider boundary.
- Avoid taking a hard dependency on the C LibVMI core.
- Keep provider-specific unsafe and FFI code isolated to the narrowest possible crate.
- Declare Rust 1.85 as the minimum supported Rust version (MSRV) and test the
  complete all-feature workspace against it in CI. Raising it requires an ADR
  update and a compatibility-policy note.

Consequences:

- The first provider crates can stay small and reviewable.
- License-sensitive dependencies remain an explicit decision point rather than an accident.
- Rust 1.82 cannot build the locked dependency graph because Cargo predates the
  stabilized Edition 2024 manifest format used by `getrandom 0.4`; Rust 1.85 is
  the first evidence-backed compatible stable release.
