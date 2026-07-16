# Compatibility Policy

The `vmi` facade is the supported application import surface. Provider crates
remain independently versioned workspace components, but applications should
prefer facade exports unless they require provider-specific configuration.

Before 1.0, minor releases may contain breaking API changes when the migration
is documented. Patch releases must remain source compatible. At 1.0, removals,
renames, signature changes, and stricter trait requirements require a major
version; additive APIs and new providers may ship in minor versions.

The committed [`public-api.txt`](public-api.txt) snapshot records the simplified
facade surface. CI rejects any drift. An intentional change requires:

1. Review the generated diff and classify its semver impact.
2. Update [migration guidance](migration.md) for breaking or behavior-changing changes.
3. Run `VMI_UPDATE_PUBLIC_API=1 bash scripts/test-public-api.sh`.
4. Commit the reviewed snapshot with the implementation.

The snapshot detects structural API drift; it does not replace behavioral
contract tests, capability validation, or human semver review.

## Error and cancellation contract

Callers should branch on `VmiError::kind()` or match typed variants, never parse
display text. `Timeout` means the operation exceeded its configured deadline;
`Cancelled` means a caller-provided cancellation signal was observed. Backend
messages remain diagnostic text and are not a stable machine-readable interface.

`CancellationToken` is cooperative. Default cancellable driver methods observe
it immediately before and after the underlying operation. Providers may override
those methods to add safe cancellation points inside long-running work; cancelling
a token does not imply that an operating-system call can be interrupted instantly.

Lifecycle generations are monotonically increasing within a session. A reconnect,
reboot, or memory-topology-change notification invalidates target-derived caches;
after `Destroyed`, callers must stop using the session. Lifecycle support is an
explicit capability and providers without a reliable native signal fail closed.
