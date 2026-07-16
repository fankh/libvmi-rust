# Development Guide

Use a stable Rust toolchain with Rustfmt and Clippy. Run commands from the
`libvmi-rust` directory.

The declared MSRV is Rust 1.85. CI checks the complete all-target, all-feature
workspace with that exact toolchain in addition to current stable.

## Build and Test

```console
cargo build --workspace --all-targets --all-features
cargo +1.85.0 check --workspace --all-targets --all-features
cargo build --workspace --all-targets --all-features --release
cargo test --workspace --all-targets
cargo test --workspace --all-targets --release
cargo test --workspace --doc
cargo run -p vmi --example custom_provider
MIRIFLAGS=-Zmiri-disable-isolation cargo +nightly miri test -p vmi-types -p vmi-driver-api -p vmi-core -p vmi-events -p vmi-views
bash scripts/test-fuzz-targets.sh
VMI_RUN_FUZZ=1 VMI_FUZZ_SECONDS=30 bash scripts/test-fuzz-targets.sh
bash scripts/test-sanitizers.sh
bash scripts/test-public-api.sh
bash scripts/test-coverage.sh
bash scripts/run-benchmarks.sh
```

The workspace includes the `vmi-cli` binary for artifact, profile, OS, QEMU,
VirtualBox, and Xen inspection workflows. See
[current implementation](current-implementation.md) for its command matrix.

## Required Local Checks

```console
cargo fmt --all --check
cargo check --workspace --all-targets --no-default-features
python scripts/verify-support-matrix.py
python -m unittest scripts/test_support_matrix.py
python scripts/verify-release-readiness.py
python -m unittest scripts/test_release_readiness.py
python scripts/generate-support-matrix.py --check
python -m unittest scripts/test_generate_support_matrix.py
python scripts/verify-unsafe-policy.py
python -m unittest scripts/test_unsafe_policy.py
python scripts/verify-fixtures.py
python -m unittest scripts/test_fixtures.py
python scripts/verify-package-metadata.py
python -m unittest scripts/test_package_metadata.py
python scripts/verify-markdown.py
python -m unittest scripts/test_markdown.py
python scripts/verify-panic-policy.py
python -m unittest scripts/test_panic_policy.py
python -m unittest scripts/test_differential_artifacts.py
cargo clippy --workspace --all-targets --all-features -- -D warnings -D unsafe-op-in-unsafe-fn -D clippy::undocumented-unsafe-blocks
cargo clippy --workspace --lib --bins --examples --all-features -- -D warnings -D clippy::as_conversions -D clippy::cast_possible_truncation -D clippy::cast_sign_loss -D clippy::cast_possible_wrap -D clippy::cast_lossless -D clippy::string_slice
cargo clippy --workspace --lib -- -D warnings -D clippy::indexing_slicing

cargo clippy --workspace --lib --bins --examples --all-features -- -D warnings -D clippy::arithmetic_side_effects
cargo test --workspace --all-targets
cargo test --workspace --all-targets --release
cargo test --workspace --doc
cargo check --workspace --all-targets --all-features --target i686-pc-windows-msvc
cargo doc --workspace --no-deps
cargo audit
cargo deny check
cargo package --workspace --allow-dirty
bash scripts/test-c-abi.sh
bash scripts/test-cli.sh
bash scripts/test-qemu-integration.sh
```

Install the 32-bit Windows target once with
`rustup target add i686-pc-windows-msvc`. The cross-target check exercises
pointer-width conversions and the C ABI without requiring a 32-bit test host.

Workspace path dependencies use exact package versions as well as local paths.
This prevents an unrelated newer crate with the same name on crates.io from
being selected while verifying or publishing the package set. Remove
`--allow-dirty` for release candidates.

`scripts/verify-package-metadata.py` also validates every internal dependency
edge and prints a deterministic dependency-first publication order. Release
automation must preserve that order because crates.io must know each internal
dependency before a dependent crate can be published. A dependency cycle, a
non-exact internal version, or a missing local path fails the check.

CI runs the portable workspace tests natively on Linux, Windows, and macOS,
plus a separate 32-bit Windows all-feature compile check. It also packages and
verifies every workspace crate without `--allow-dirty` on a clean checkout.
Nightly CI also executes the portable type, driver-contract, core, event, and
view crates under Miri.

Install `cargo-fuzz` with `cargo install cargo-fuzz --locked`. The default fuzz
script compiles both harnesses; opt-in execution bounds each campaign by time
and limits generated inputs to 64 KiB. Preserve any crash artifact as a
regression seed before fixing it.

Every checked-in fuzz seed is inventoried in `fuzz/fixtures.toml`. The fixture
validator verifies hashes, sizes, provenance fields, path confinement, and an
exact manifest-to-corpus inventory. See [fixture policy](fixture-policy.md).

The differential artifact suite constructs ELF64 files with Python's standard
library and compares randomized CLI reads against an independent byte-map
oracle. This covers raw slicing, ELF load segments, zero-filled memory tails,
and reads spanning adjacent segments without sharing Rust parser code.
The scheduled fuzz workflow runs both targets for 60 seconds daily and retains
crash artifacts for 14 days; pull-request CI compiles the harnesses only.

`scripts/test-sanitizers.sh` requires nightly Rust on x86-64 Linux. It runs all
workspace targets under AddressSanitizer with leak detection and aborts on the
first memory error. The suite includes the Rust side of the C ABI boundary.

Install `cargo-public-api 0.52.0` and run the public API script with nightly
Rust. It compares the simplified `vmi` facade against the committed snapshot.
Set `VMI_UPDATE_PUBLIC_API=1` only after reviewing the semver and migration
impact of an intentional API change.

Install `cargo-llvm-cov 0.6.21` and the `llvm-tools-preview` Rust component to
run the coverage gate. The all-target workspace baseline measured on Windows
on 2026-07-14 is 75.87% lines; CI enforces a platform-tolerant 70% floor.
Override `VMI_MINIMUM_LINE_COVERAGE` only when diagnosing a proposed threshold
change, and commit threshold changes only with a newly recorded measurement.

The `vmi-types` rustdoc suite includes compile-fail examples that enforce the
type separation between guest-physical addresses, guest-virtual addresses,
and translation roots. `cargo test --workspace --doc` executes these API
contracts on stable and MSRV-compatible Rust.

Core address/range and AMD64 translation invariants also run under generated
property tests. Failing cases are minimized by `proptest`; preserve a minimized
failure as a deterministic regression test before changing the implementation.
Private QEMU, VirtualBox, and Xen command/protocol parsers are subjected to the
same generated Unicode and malformed-text inputs without exposing those parser
functions as public API.

The release benchmark harness writes JSON suitable for same-machine comparison.
See [performance baselines](performance-baselines.md). Pull-request CI compiles
the harness and tests its comparator, but deliberately does not gate on timing
from variable shared runners.

Nineteen portable crates inherit `unsafe_code = "forbid"`. Only
`vmi-driver-xen` and `vmi-ffi` are reviewed unsafe boundary crates; the unsafe
policy validator rejects new exemptions, unsafe syntax in portable crates, or
safe crates that stop inheriting workspace lints.

## Provider Contract Rules

1. Advertise only capabilities the provider implements.
2. Reject required but unavailable capabilities during attachment.
3. Return typed errors for unsupported optional facets.
4. State the consistency mode accurately.
5. Test every advertised capability and failure path.
6. Update `support-matrix.toml` only after corresponding tests pass.

## C ABI

The public header is `crates/vmi-ffi/include/vmi.h`. Raw, ELF vmcore, Xen core,
legacy AMD64 KDMP, LiME, and manifest artifacts use the same opaque handle. Handles are
monotonic tokens backed by a synchronized Rust-owned registry, not dereferenceable allocation
addresses. Release them with `vmi_snapshot_close`; stale, foreign, and repeated closes are
rejected safely and recorded in the calling thread's last error. Errors are copied with
`vmi_last_error`; no Rust panic is allowed to cross the ABI boundary.

Build shared and static libraries with:

```console
cargo build --release -p vmi-ffi
```

Define `VMI_STATIC` when statically linking on Windows.

On Linux, a static C consumer links the compression libraries used by the artifact crate:

```console
cc app.c target/release/libvmi_ffi.a -ldl -lpthread -lm -llzma -lbz2 -lz -lzstd
```

`scripts/test-c-abi.sh` compiles the public header as strict C11, tests both
dynamic and static linkage, compiles the same consumer as strict C++17, and
validates ABI versioning, artifact open, segment enumeration, physical-memory
inspection, error access, cleanup, and the exact shared-library export set.

`scripts/verify-support-matrix.py` uses Python 3.11 or newer from the standard
library only. It rejects schema drift, duplicate or malformed providers,
unknown maturities/capabilities, empty claims, and inventory mismatches.

`scripts/verify-release-readiness.py` validates the v1 gate ledger, status
vocabulary, unique gate IDs, criticality flags, repository-confined evidence
paths, and evidence existence. A critical gate can be marked complete only
after its documented exit criteria pass.

`scripts/generate-support-matrix.py` converts the authoritative TOML contract
to `docs/support-matrix.md`. Run it without `--check` after changing provider
claims; CI rejects stale generated output.

`scripts/test-cli.sh` performs real raw-memory inspection through the compiled
binary, verifies uppercase hexadecimal input, and checks fail-closed malformed
number and unknown-command paths.

`scripts/test-qemu-integration.sh` requires `qemu-system-x86_64` and Python 3.
It launches an isolated two-vCPU TCG guest, exercises the public CLI over real
QMP/GDB transports, re-reads acquired range/core artifacts, and writes a JSON
qualification transcript under `target/`. It also performs a configurable soak,
enforces QEMU RSS/file-descriptor growth budgets, and verifies abrupt-disconnect
failure behavior. Set `VMI_QEMU_SOAK_SECONDS=3600` for release qualification.
## Documentation Rules

- Update [current implementation](current-implementation.md) when behavior lands.
- Update the [implementation plan](../implementation-plan.md) when sequencing or
  acceptance criteria change.
- Add an ADR for decisions that constrain multiple crates or providers.
- Keep research claims separate from current support claims.
