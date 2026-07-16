# Supply-Chain Security

The locked dependency graph is checked by RustSec and the committed cargo-deny
policy. CI rejects known advisories, yanked dependencies, wildcard requirements,
unapproved licenses, duplicate versions, and dependencies from unknown registries
or Git sources. Persistent fuzz inputs have exact hashes and provenance, portable
unsafe code is forbidden, and the two approved unsafe boundary crates have an
explicit reviewed inventory.

Tag releases generate CycloneDX JSON SBOMs with pinned `cargo-cyclonedx` 0.5.9.
The CLI and C ABI are packaged separately so each archive is paired with the BOM
for the actual product it contains. Archives use sorted paths, a fixed timestamp,
numeric ownership, and SHA-256 checksums.

GitHub Actions signs the archive subject digests and their SBOM claims through
Sigstore-backed artifact attestations. Verify a downloaded archive before use:

```console
sha256sum --check SHA256SUMS
gh attestation verify libvmi-rust-cli-vVERSION-x86_64-unknown-linux-gnu.tar.gz --repo fankh/libvmi-rust
gh attestation verify libvmi-rust-ffi-vVERSION-x86_64-unknown-linux-gnu.tar.gz --repo fankh/libvmi-rust
```

An attestation proves source and workflow provenance; it does not by itself prove
that an artifact is defect-free. Consumers must also enforce the expected tag,
repository, and signer workflow and review advisories applicable to their use.

The gate becomes complete only after the final locked graph passes audit/deny,
scheduled fuzzing and sanitizer runs are green, both unsafe boundaries are reviewed
at the release commit, and an RC artifact's checksum, SBOM, and attestation have
been independently verified.
