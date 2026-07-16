# Unsafe Code Inventory

The workspace compiler policy sets `unsafe_code = "forbid"` for all crates
except `vmi-driver-xen` and `vmi-ffi`. `scripts/verify-unsafe-policy.py` checks
that this allowlist and the source-level unsafe syntax inventory stay exact.

Updated: 2026-07-14

Production unsafe code is confined to two crates. Every block relies on an
explicit caller or vendor-ABI invariant and is covered by strict Clippy and
cross-platform release builds.

## `vmi-ffi`

| Boundary | Purpose | Safety invariant |
| --- | --- | --- |
| Exported C functions | Read C strings and write caller-owned outputs | Public `# Safety` contracts require valid pointers; null and slice-length checks run before constructing references or slices |
| Opaque snapshot tokens | Identify registry entries without exposing Rust objects | Tokens are never dereferenced, are validated against the locked live-handle registry, and use a non-wrapping allocator |
| Last-error copying | Copy thread-local error bytes to C storage | Copy occurs only when the caller-provided buffer is non-null and large enough, including the trailing NUL |

All exported operations catch Rust panics before they can cross the C ABI.
Foreign, stale, and double-closed handles are rejected without dereferencing
their values.

## `vmi-driver-xen`

| Boundary | Purpose | Safety invariant |
| --- | --- | --- |
| `libloading` symbol resolution | Load supported `libxenctrl` entry points | Symbols use the published xenctrl signatures and cannot outlive the retained `Library` |
| xenctrl interface handle | Open, serialize, and close the native interface | The raw handle is checked for null, retained behind a mutex, and closed before the library is dropped |
| Foreign-page mapping | Read or write one mapped Xen guest page | GFN conversion is checked, mapping failures are rejected, copies remain within one 4 KiB mapping, and every successful map is unmapped |
| `Send`/`Sync` implementation | Permit provider sessions to share the backend | Every use of the xenctrl handle is serialized by its mutex; the loaded library remains owned by the backend |

## Review Checklist

- Keep new unsafe code inside the narrowest FFI/provider crate.
- Add a `SAFETY` explanation immediately around each unsafe operation.
- Reject null pointers, invalid lengths, failed conversions, and vendor error
  sentinels before memory access.
- Add focused boundary tests and run the complete Windows and Docker Linux
  verification matrices.
- Update this inventory whenever an unsafe boundary changes.
