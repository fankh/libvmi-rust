# Operations Qualification

This runbook defines the minimum operational contract for a v1 deployment. The
gate is complete for the narrowed supported-provider scope. Post-v1 promotions
require their own retained load, fault, and recovery evidence.

## Health and Budgets

Treat attach plus a bounded status/read operation as the provider health check.
Do not use an unbounded memory acquisition as a liveness probe. Operators must set
an application deadline and cancellation token around work that can outlive a
single provider command.

For the QEMU release qualification, a one-hour repeated control/read loop must:

- complete without a command or semantic-data failure;
- grow QEMU resident memory by no more than 32 MiB;
- grow the QEMU file-descriptor count by no more than 16;
- preserve equality between live reads and range/core acquisitions; and
- return a typed backend error after abrupt transport loss.

Artifact providers must remain bounded by their documented input-size, segment,
decompression, traversal, and allocation ceilings. Same-machine core benchmark
results use a 10% default regression limit; shared-runner timing is diagnostic and
must not be compared across unrelated hosts.

## Diagnosis

Record the library version and commit, provider ID and capability set, host and
guest versions, requested consistency, operation and deadline, `VmiErrorKind`, and
the complete diagnostic error text. For live providers also record the vendor tool
version, endpoint type, target lifecycle state, and whether the failure followed a
pause, resume, migration, snapshot, or disconnect.

Preserve generated JSON qualification evidence and its SHA-256 digest. Never put
guest secrets, raw memory, authentication material, or unrestricted vendor command
output in routine CI artifacts.

## Recovery

1. Stop issuing new operations and cancel cooperative in-flight work.
2. If lifecycle generation changed or the transport disconnected, discard the
   session; do not reuse cached translations or provider handles.
3. Restore the target to its intended running/paused state with the hypervisor's
   native management plane.
4. Reattach, renegotiate required capabilities, and repeat the bounded health read.
5. Resume workload processing only after semantic validation succeeds. Repeated
   timeout, permission, corruption, or capability failures require operator action;
   automatic retry must be bounded and use backoff.

Artifact corruption and profile mismatch are not transient. Quarantine the input,
verify provenance and digest, and reacquire it instead of retrying indefinitely.

## Gate Exit Criteria

The operations gate becomes complete after supported QEMU and artifact workflows
have retained resource-budget/load/fault results, preview providers have documented
best-effort recovery evidence on their real-host matrices, and an RC exercise has
demonstrated the diagnosis and recovery sequence above. Any unresolved resource
growth, state-restoration failure, silent partial inventory, or untyped timeout is
release blocking.
