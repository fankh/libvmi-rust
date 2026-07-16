# Production Readiness for v1.0

The authoritative provider contract is [`support-matrix.toml`](../support-matrix.toml).
It separates current maturity from the maximum tier intended for v1.0. The
authoritative gate ledger is [`release-readiness.toml`](../release-readiness.toml).

## Release Tiers

- **Supported**: stable capability contract, documented platform/version range,
  real-environment integration evidence, security review, and release-blocking CI.
- **Preview**: implemented and tested with typed failure behavior, but with a
  narrower validation matrix or known upstream/vendor limitations. Preview APIs
  retain normal semver protection, while operational support is best effort.
- **Experimental**: opt-in or offline integration with no production availability
  commitment. It must fail closed and cannot block v1 unless it affects shared code.
- **Internal**: deterministic test infrastructure that is not a production provider.

Current maturity never inherits from an upstream hypervisor's reputation. A
provider reaches its v1 target only after repository evidence satisfies that tier.

## Global v1 Exit Criteria

Every critical gate in `release-readiness.toml` must be `complete`. In addition:

1. Supported providers pass unit, property, corruption, concurrency, and real-host
   integration tests on every claimed platform or an explicitly documented subset.
2. Preview providers advertise only verified capabilities and document version,
   platform, consistency, and vendor limitations.
3. Debug and release tests, MSRV, no-default-feature, 32-bit, documentation,
   sanitizer, Miri, fuzz-build, C ABI, CLI, packaging, dependency, and policy gates pass.
4. The public facade is frozen and intentional behavior changes have migration notes.
5. Critical/high security defects and correctness defects are closed; accepted lower
   risks have owners, rationale, and release notes.
6. Release artifacts are reproducible, include an SBOM, and are signed after an RC soak.

## v1 Provider Scope

- **Supported target**: raw dump, manifest snapshots, converted/offline cores,
  VirtualBox cores, and QEMU.
- **Preview target**: Xen.
- **Experimental target**: live VirtualBox, Firecracker, Cloud Hypervisor,
  Hyper-V live/saved-state, VMware live artifacts, and bhyve integrations.
- **Internal**: the deterministic fake provider.

KVMi and a custom kernel module are post-v1 research unless a separate release
decision adds them to both the provider matrix and readiness ledger.

Guest OS adapters and profile parsers ship as tested APIs, but broad kernel-build
compatibility is not a v1 production claim. The real-guest matrix remains post-v1
qualification work and cannot expand the v1 support contract without retained
artifact evidence.

## Status Changes

A gate may move to `complete` only when every listed evidence path exists and its
acceptance criteria pass. A blocked gate records the blocker in the relevant design
or provider document. Changes to provider tiers, platforms, or criticality require
review as release-scope changes.
