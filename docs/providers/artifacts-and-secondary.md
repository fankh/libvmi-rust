# Artifact and Secondary Provider Qualification

## Supported Artifact Providers

The v1 Supported artifact surface is immutable and read-only:

- `raw-dump` for flat physical-memory images;
- `snapshot-manifest` for versioned, confined multi-file sparse snapshots;
- `virtualbox-core` for VirtualBox ELF VM cores;
- `hyperv-core` for user-converted ELF or Windows KDMP output;
- `vmware-core` for user-converted ELF or Windows KDMP output.

The converted-core identifiers are provenance aliases over the shared ELF/KDMP
normalization path; they do not imply native parsing of proprietary saved-state
containers. Every provider advertises only physical-memory read with immutable
snapshot consistency.

Qualification includes bounded metadata reads, checked 64-bit offsets/ranges,
sparse holes, adjacent segments, final-address handling, zero-filled ELF tails,
manifest path and symlink confinement, compressed-stream limits, malformed and
overlapping records, property tests, persistent fuzz seeds, differential reads
against an independent Python oracle, C ABI reads, and Windows/macOS/Linux CI.

## Explicit v1 Deferrals

The following remain Experimental and cannot be presented as operationally
supported in v1:

| Provider | v1 boundary |
| --- | --- |
| Firecracker | Versioned memory snapshot manifest only; no REST lifecycle orchestration |
| Cloud Hypervisor | Versioned memory snapshot manifest only; no REST lifecycle orchestration |
| Hyper-V | Versioned saved-state manifest only; no native proprietary container decoder |
| VMware | Flat `.vmem` or user-converted core only; no automated snapshot orchestration |
| bhyve | Compile-only saved-state manifest; converted cores remain Experimental |

KVMi and the custom kernel module remain outside v1. No Experimental provider may
block the release unless it exposes a shared parser or core correctness defect.
Promotion requires a separate support-matrix change backed by native-host fixtures,
version/platform policy, operational soak, fault recovery, and security review.
