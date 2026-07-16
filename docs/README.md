# LibVMI-Rust Documentation

This directory is the entry point for implementation documentation. Documents are
grouped by purpose so current behavior is not confused with research or planned
functionality.

## Start Here

| Document | Purpose |
| --- | --- |
| [Current implementation](current-implementation.md) | What is implemented, tested, and still missing |
| [Production readiness](production-readiness.md) | v1 provider tiers, release gates, and exit criteria |
| [QEMU provider qualification](providers/qemu.md) | Supported-tier contract, evidence, and remaining real-host gates |
| [Xen provider qualification](providers/xen.md) | Preview-tier capability activation, evidence, and native-host limits |
| [VirtualBox provider qualification](providers/virtualbox.md) | Preview-tier transport evidence and vendor limitations |
| [Artifact and secondary providers](providers/artifacts-and-secondary.md) | Supported artifact evidence and explicit v1 deferrals |
| [Development guide](development.md) | Build, test, lint, and documentation commands |
| [Provider authoring guide](provider-authoring.md) | Implement and validate a capability-accurate provider |
| [Compatibility policy](compatibility-policy.md) | Semver rules and public facade drift enforcement |
| [Migration guide](migration.md) | Release-to-release application migration notes |
| [Performance baselines](performance-baselines.md) | Reproducible core benchmarks and regression comparison |
| [Fixture policy](fixture-policy.md) | Integrity, provenance, licensing, and inventory rules for persistent test inputs |
| [Unsafe code inventory](unsafe-code-inventory.md) | Audited FFI/native boundaries and their invariants |
| [Implementation plan](../implementation-plan.md) | Authoritative roadmap and proposed workspace |
| [Provider support matrix](support-matrix.md) | Generated human-readable capability and maturity claims |
| [Provider support contract](../support-matrix.toml) | Authoritative machine-readable provider claims |
| [Release readiness ledger](../release-readiness.toml) | Authoritative machine-readable v1 gate status and evidence |

## Architecture Decisions

1. [Native Rust API and deferred provider ABI](adr/0001-native-rust-api-and-deferred-c-abi.md)
2. [Capability and consistency model](adr/0002-capability-and-consistency-model.md)
3. [Reuse, licensing, MSRV, and unsafe policy](adr/0003-reuse-licensing-msrv-and-unsafe-policy.md)

## Research Background

These documents record investigation and design inputs. They do not claim that
the described functionality is currently implemented.

| Document | Subject |
| --- | --- |
| [LibVMI overview](../libvmi-overview.md) | C LibVMI architecture and limitations |
| [Rust VMI ecosystem](../rust-vmi-ecosystem.md) | Related Rust projects and reusable components |
| [KVM VMI methods](../kvm-vmi-methods.md) | Available KVM introspection approaches |
| [Xen VMI methods](../xen-vmi-methods.md) | Xen event and memory introspection |
| [Experimental KVM kernel module](../kvm-rust-kernel-module.md) | Research proposal, not a supported provider |
| [Research mind map](../mindmap.md) | Visual map of the investigation |

## Historical Material

[Legacy project structure](../project-structure.md) is retained as background for
the earlier Xen/KVM design. It is not the authoritative workspace definition.
