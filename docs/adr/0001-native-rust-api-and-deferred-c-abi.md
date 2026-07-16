# ADR 0001: Native Rust API and Deferred Provider ABI

Status: Accepted, amended after the artifact C ABI was added

Decision:

- The portable API is implemented in Rust-first crates.
- No Rust ABI is exposed across dynamic library boundaries in v1.
- The versioned `vmi-ffi` C ABI exposes immutable artifact inspection to foreign-language consumers; it does not expose Rust provider traits or dynamically load providers.
- If dynamic provider extensibility becomes necessary later, it will be introduced behind a separately versioned C ABI or WIT boundary.

Consequences:

- Provider crates can evolve behind Cargo features and static linking.
- The provider API stays safe to refactor without committing to unstable plugin loading.
- The artifact C ABI has its own opaque ownership, error, compatibility, and C-consumer tests.
