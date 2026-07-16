# ADR 0002: Capability and Consistency Model

Status: Accepted

Decision:

- Capability support is explicit and provider-specific.
- Read, write, register, control, view, acquisition, and event behavior are modeled as separate facets.
- Consistency is reported as one of: live best-effort, paused, or immutable snapshot.
- Unsupported operations fail closed at attach time when they are requested explicitly.

Consequences:

- Providers do not need to fake unsupported behavior.
- Contract tests can verify capability claims directly.
