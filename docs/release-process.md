# Release Process

The tag workflow is intentionally fail closed. A release tag must exactly match
the workspace package version, and every critical gate in
`release-readiness.toml` must be complete. Development CI validates ledger shape
without requiring completion; the tag workflow invokes the validator's strict
`--require-complete` mode.

## Candidate Procedure

1. Freeze the public facade and review `docs/public-api.txt` plus migration notes.
2. Set one workspace version, update exact internal dependency requirements, and
   set the readiness ledger release to the same version.
3. Complete all real-host matrices and retain their immutable qualification JSON.
4. Run the full CI suite, scheduled fuzz campaign, and release QEMU soak from the
   exact candidate commit.
5. Review dependency/unsafe/security results and record the go/no-go decision.
6. Create the matching annotated `vVERSION` tag. The tag workflow reruns strict
   readiness, audit, deny, release tests, build, SBOM, deterministic archive,
   checksum, and attestation steps before creating the GitHub release.
7. Independently download and verify both archives and attestations, then publish
   crates in the dependency-first order printed by the metadata validator.

Never retag a published version. If verification fails, fix forward, repeat the RC
qualification, and use a new version. Crates.io publication remains a deliberate
operator step because partial publication is irreversible; the validator provides
the required deterministic order and rejects dependency cycles or non-exact
workspace requirements.

## Current Blockers

The v1 release remains blocked on QEMU soak completion, real Xen/VirtualBox host
matrices, real Linux/Windows guest compatibility artifacts, final operations and
security exercises, and synchronization of the current `0.1.0` workspace version
with the planned release ledger. The workflow is present now so release mechanics
are reviewed before those external qualification gates close.
