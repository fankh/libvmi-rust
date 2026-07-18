# Current Implementation

Updated: 2026-07-14

The repository provides a contract foundation and an offline vertical slice for
a native Rust VMI framework. It can inspect raw physical-memory artifacts,
including pages acquired from QEMU, and provides capability-limited direct QEMU,
libvirt-managed QEMU/KVM, VirtualBox, and Xen `xl` attachment.

## Implemented Crates

| Crate | Current responsibility |
| --- | --- |
| `vmi-types` | Addresses, ranges, capabilities, errors, descriptors, consistency, architecture, and scalar decoding |
| `vmi` | Curated public facade, namespaces, and application prelude |
| `vmi-driver-api` | Connector/session boundaries, memory access, optional facets, and typed read helpers |
| `vmi-testkit` | Deterministic sparse-memory fake provider and contract tests |
| `vmi-artifact` | Raw, ELF64, Xen core, legacy/bitmap/RDMP and gzip/zlib/bzip2/xz/zstd-wrapped AMD64 KDMP, LiME, and manifest snapshots |
| `vmi-driver-dump` | Read-only raw physical-memory provider |
| `vmi-core` | Provider attachment and high-level byte/scalar reads |
| `vmi-cli` | Executable raw-dump physical-memory inspection |
| `vmi-arch-api` | Architecture-neutral address-translation contract |
| `vmi-arch-amd64` | Four/five-level AMD64 translation with 4 KiB, 2 MiB, and 1 GiB pages |
| `vmi-arch-aarch64` | Configurable AArch64 stage-1 translation for 4/16/64 KiB granules and block mappings |
| `vmi-driver-qemu` | Live QMP physical/register reads, status, pause/resume, asynchronous events, and physical-range acquisition over TCP or Unix sockets |
| `vmi-driver-libvirt` | Managed QEMU/KVM domain validation, control, memory-only ELF acquisition, and physical-range extraction through virsh |
| `vmi-driver-snapshot` | Generic, VirtualBox, microVM, VMware, Hyper-V, and bhyve snapshots |
| `vmi-driver-virtualbox` | VBoxManage registers, state/control, snapshot-backed live memory reads, core acquisition, and range extraction |
| `vmi-driver-xen` | Live xl control/acquisition plus optional libxenctrl memory, xenctx registers, and vm_event transport integration |
| `vmi-ffi` | Versioned C ABI for opening, describing, and reading immutable memory artifacts |
| `vmi-profile` | Linux `System.map`, native PDB public symbols, and normalized JSON profiles |
| `vmi-os-linux` | Configurable process, module, file, path, and socket inspection |
| `vmi-os-windows` | Configurable process/module traversal and `FILE_OBJECT` inspection |
| `vmi-events` | Bounded thread-safe event queue implementing `EventAccess` |
| `vmi-views` | Thread-safe memory-view lifecycle and active-view management |

## Enforced Behavior

- Required capabilities are checked before a session is returned.
- Unsupported optional facets fail with `VmiError::CapabilityMissing`.
- Physical-memory reads use typed guest physical addresses and ranges.
- Artifact reads span adjacent normalized segments while still failing on sparse-memory holes.
- Scalar reads require an explicit byte order.
- Provider metadata and target consistency are exposed through the session.

## Verified Coverage

- The simplified public `vmi` facade has a committed API snapshot; CI rejects
  unreviewed additions, removals, renames, and signature drift.
- The complete x86-64 Linux workspace test matrix passes under AddressSanitizer
  with leak detection, including parser, provider, example, and C ABI tests.
- Buildable libFuzzer targets exercise KDMP/LiME artifact parsers and normalized
  JSON/System.map profile parsers from committed format-aware seed corpora.
- Daily bounded fuzz campaigns upload crash artifacts on failure, while pull
  requests compile both harnesses to prevent target drift.
- Miri executes 15 portable-core tests covering translation caches, provider
  registration, event synchronization, scalar decoding, and memory-view lifecycles.
- Public offline-inspection and custom-provider examples compile with all
  targets; the custom provider executes in CI as an end-to-end contract smoke test.
- Every crate declares Rust 1.85 as its MSRV, and CI compiles the complete
  all-target, all-feature workspace with that exact toolchain.
- The compiler forbids unsafe Rust in all 19 portable crates; a tested policy
  validator confines unsafe syntax to the reviewed Xen-native and C ABI boundaries.
- Provider capability documentation is generated deterministically from the
  validated TOML contract, with CI rejecting stale human-readable tables.
- Portable workspace tests run natively on Linux, Windows, and macOS in CI;
  release packaging verifies each of the 21 crates from a clean checkout.
- All 143 Windows workspace tests pass in both debug and release profiles, exercising
  the same behavior with overflow checks enabled and disabled.
- The complete all-feature workspace, including native compression dependencies
  and the C ABI, compiles for 32-bit Windows (`i686-pc-windows-msvc`) in CI.
- Attach-time rejection when a required capability is absent.
- Successful little-endian memory reads through the fake provider.
- Fail-closed CPU, control, event, view, and acquisition facets.
- Deterministic capability iteration.
- Explicit-endian scalar decoding.
- Raw artifact range and boundary validation.
- QEMU `pmemsave` output inspected through the Rust CLI and matched against QMP.
- AMD64 canonical-address validation and normal/large-page translation.
- AMD64 LA57 five-level translation and 57-bit canonical-address validation.
- AArch64 4/16/64 KiB-granule stage-1 translation, configurable address sizes, and block mappings.
- Linux `System.map` validation, aliases, exact lookup, and symbol-plus-offset resolution.
- Normalized JSON symbol/structure-offset profiles with strict numeric validation.
- Text profile files are bounded to 64 MiB, and `System.map`, normalized symbol, and offset collections are limited to one million entries.
- Native Microsoft PDB public symbols plus `Type.Member` field offsets, including OMAP relocation and continued field lists.
- PDB files are bounded to 8 GiB, with one-million-entry ceilings for public symbols and decoded field offsets.
- Linux `init_task` traversal with PID/command extraction, loop detection, and limits.
- Linux `modules` traversal with names, core ranges, loop detection, and limits.
- Linux per-task `files_struct`/`fdtable` traversal with descriptor, file, dentry, and basename extraction.
- Linux `d_parent` ancestry reconstruction for absolute dentry paths with cycle and component limits.
- Linux `mnt_root`/`mnt_mountpoint`/`mnt_parent` traversal across nested mount boundaries.
- Linux `file`/`socket`/`sock` inspection with IPv4/IPv6 endpoints, ports, protocol, and state.
- Profile-configured circular socket-list enumeration with corruption and traversal limits.
- Profile-configured bucket/hlist socket-table enumeration with cross-bucket duplicate detection.
- Windows `PsActiveProcessHead` traversal with PID/image/DTB extraction and corruption protection.
- Windows `PsLoadedModuleList` traversal with UTF-16 names, image ranges, and corruption protection.
- Windows `FILE_OBJECT` decoding with NT path, device pointer, flags, and access booleans.
- Configurable Windows level-0/1/2 handle-table enumeration with object decoding and granted access.
- Windows handle-table address resolution rejects unknown levels with a typed backend error; no production panic macros remain in the workspace.
- Portable event ordering, timeout, capacity, closure, and producer/consumer synchronization.
- Capability-gated physical and cross-page virtual writes with cache invalidation.
- Portable memory-view creation, deletion, switching, and lifecycle protection.
- Stable application import surface across core, providers, artifacts, architectures, and OS layers.
- Thread-safe provider registration, descriptor enumeration, lookup, removal, and attachment by ID. Provider IDs and descriptor strings are copied with fallible allocation. Descriptor enumeration snapshots connectors under the registry lock and invokes connector callbacks only after releasing it, permitting reentrant registry use.
- Bounded translation caching with explicit invalidation.
- Translation-cache poisoning returns a typed backend error instead of panicking, including during write-triggered invalidation.
- Translation physical-address offset arithmetic is checked and fails closed on overflow.
- Translation caches are invalidated after every attempted physical or virtual write, including partial-write failures.
- Translation-cache keys include an explicit stable translator tag, preventing mappings from different architecture/configuration translators from aliasing under the same root and virtual page without relying on object addresses.
- Cross-page virtual reads with page-granular translation caching.
- Live QEMU physical reads matched against an acquired dump from the same state.
- Live QEMU vCPU general/control register reads with validated register names.
- Configurable QMP timeouts with correlated replies across asynchronous events and protocol-error tests.
- Platform-gated QMP Unix-domain socket attachment for native Linux hypervisor deployments.
- Real Docker Linux QEMU validation over a Unix QMP socket: status, reset-mode `RIP`/`RFLAGS`, physical reads, 4 KiB `pmemsave`, and a 33.8 MB ELF VM core were acquired and re-read successfully.
- Real QEMU 11 event validation: same-session pause/resume operations delivered ordered `STOP` and `RESUME` events through the generic event facet.
- QMP events use a bounded 1,024-entry pending queue and fail closed on overflow. Event-kind ownership and any queue growth are fallible, so allocation pressure is returned as a backend error at the untrusted protocol boundary.
- QMP JSON frames are limited to 16 MiB and GDB RSP replies to 1 MiB to prevent unbounded allocations from malformed endpoints. Successful QMP `return` subtrees are moved out of the owned response rather than cloned, avoiding a second frame-sized allocation.
- QEMU HMP register aliases normalize reset/legacy-mode `EIP` and `EFL` output to canonical `RIP` and `RFLAGS` requests.
- Opt-in QEMU GDB RSP register writes with checksums, acknowledgements, mapping, and read-back verification.
- Real QEMU 11 GDB validation covers four-byte `RFLAGS` writes and explicit RSP thread selection with verified vCPU 1 `RIP` writes on a two-vCPU guest.
- Full QEMU ELF VM-core acquisition and generic `PT_LOAD` normalization.
- ELF truncation, range, overlap, and zero-fill validation.
- Xen ELF core physical-range normalization with format-specific provenance.
- Legacy AMD64 `PAGE`/`DU64` KDMP physical-run normalization and strict bounds validation.
- Modern AMD64 `SDMP`/`FDMP` bitmap KDMP sparse-page normalization and strict count/bounds validation.
- Modern AMD64 uncompressed `RDMP` active/kernel-memory range normalization and strict bounds validation.
- Transparent gzip/zlib KDMP wrapper decoding followed by the same strict legacy, bitmap, or RDMP validation.
- Transparent bzip2/xz/zstd KDMP wrapper decoding with magic-based detection and strict inner-format validation.
- Compressed KDMP decoding is bounded to 64 GiB by default, with a caller-configurable lower ceiling applied consistently to gzip, zlib, bzip2, xz, and zstd wrappers. Decoded output grows fallibly in 64 KiB chunks rather than relying on `read_to_end`, while preserving limit-plus-one bomb detection.
- Converted-core detection reads only the eight-byte signature, and standalone KDMP metadata inspection reads only the fixed 8 KiB header rather than allocating the complete dump.
- ELF core loading enforces configurable file and aggregate decoded-memory ceilings, checks declared `PT_LOAD` memory before allocation, and uses fallible segment reservation.
- Raw, LiME, Xen core, and encoded/uncompressed KDMP file loading now applies metadata-based size ceilings before reading; raw/LiME/Xen expose configurable limit variants for large trusted captures.
- Bounded artifact-file reads continue with fallible 64 KiB chunk growth if an already-open file changes after metadata inspection, so a metadata/read race cannot reintroduce infallible `read_to_end` allocation.
- Limited artifact reads use the same open file handle for metadata and input, bound the read itself to `limit + 1`, recheck the resulting length, and reserve buffers fallibly to close file-growth races.
- The public raw-inspection example uses the same bounded artifact loader instead of an unbounded whole-file allocation.
- Fixed-width artifact field decoding uses checked offset arithmetic and panic-free array copies, returning typed truncation/overflow errors for hostile offsets.
- Manifest segment buffers use fallible exact reservation before reading, so allocation pressure is reported as a backend error rather than an infallible allocation path.
- Generic physical/scalar reads, translated virtual reads, and VirtualBox/Xen range acquisition use fallible buffers; Xen now also rejects zero-length acquisition before invoking the hypervisor.
- Physical ranges and raw snapshot reads use widened half-open arithmetic, including correct access to `u64::MAX` without wraparound. File-backed raw artifacts use the checked constructor and reject mappings that extend beyond the physical address space; in-memory callers can opt into the same validation with `SnapshotBundle::try_from_raw`.
- Snapshot reads preflight complete contiguous segment coverage before copying, so a request that crosses a hole fails without partially modifying the caller's Rust or C output buffer.
- Text profiles and PDBs inspect the same open handle they subsequently parse; text reads are bounded to `limit + 1` and use fallible pre-reservation, closing metadata/open races.
- Text-profile reads continue with fallible 64 KiB chunk growth when the open file changes after metadata inspection, then convert the owned buffer to strictly validated UTF-8 without copying.
- Profile symbol-name maps, address-alias vectors, JSON/PDB offset maps, and PDB field-list cycle detectors grow fallibly and surface allocation failures as backend errors. Symbol insertion allocates both index keys before mutating either index, preventing recoverable allocation errors from leaving partial entries. PDB compound field names are built fallibly, and conflicting duplicate offsets are rejected without replacing the original value.
- QEMU acquisition paths reject control characters before monitor submission, and HMP range dumps escape both quotes and backslashes to prevent command injection through filenames.
- QEMU range and full-core acquisition reject an existing local destination—or an unexpected metadata inspection error—before submitting the filename to the monitor. Because QMP may address a remote host filesystem and exposes no atomic no-replace publication primitive, concurrent remote filename creation remains an environment-level limitation.
- Artifact prefix buffers and KDMP/manifest segment tables use fallible exact reservation in addition to their structural count limits. Prefix detection uses explicit bounded chunk reads, eliminating the last production `read_to_end` path and its possible full-buffer growth probe.
- Linux and Windows introspection result/path vectors grow fallibly, and Windows UTF-16 decoding pre-reserves its word buffer without an implicit infallible collect.
- Linux guest byte strings and Windows process image names preserve lossy UTF-8 compatibility through bounded decoders: valid buffers transfer ownership without copying, while corrupt sequences use checked worst-case sizing and fallible allocation before replacement-character recovery.
- Linux absolute guest paths are sized with checked arithmetic and assembled once into fallibly reserved output. Windows UTF-16 module names likewise reserve a checked worst-case UTF-8 size before strict surrogate decoding, eliminating infallible final string construction.
- Offline snapshot connector target IDs and `source (format)` display names are copied and assembled with checked, fallible allocation before connector publication.
- Dump, offline snapshot, QEMU, VirtualBox, and Xen sessions share immutable provider metadata with their connectors (and immutable target metadata where targets are known before connection), avoiding attachment-time deep copies. Mutable capability builders use copy-on-write so cloned connectors remain isolated, and capability-rejection provider IDs are copied with fallible allocation.
- Live-provider runtime capability and unsupported-operation errors also copy provider IDs fallibly. The reusable fake provider follows the same shared-metadata and copy-on-write model so contract tests exercise equivalent ownership behavior.
- VirtualBox and Xen subprocess reader thread names are length-checked and allocated before child creation, so allocation failure cannot leave a newly spawned command behind. QEMU register normalization and VirtualBox/Xen bounded numeric command arguments reserve storage fallibly before formatting.
- QEMU GDB and HMP request strings use checked, pre-sized construction. Range-acquisition monitor paths are escaped in one fallible pass, and full-dump `file:` protocols are length-checked before QMP request construction.
- VirtualBox and Xen range-publication filenames are assembled with fallible native `OsString`/`PathBuf` reservations, preserving non-UTF-8 Unix filename bytes instead of applying lossy conversion. Temporary core extensions and VirtualBox `--filename` arguments are likewise pre-sized and fallible.
- QEMU acquisition destinations are assembled deterministically as absolute paths without canonicalizing the required-absent output; non-UTF-8 paths fail closed and Windows separators are normalized fallibly. Fake-provider atomic publication also preserves native filename bytes, uses a non-wrapping temporary ID allocator, and constructs runtime capability errors fallibly.
- QEMU TCP, optional GDB, and Unix socket endpoint identities must be non-empty and control-free; Unix target names must also be UTF-8. All configured endpoint identities are validated before either QMP or GDB transport creation.
- Default `Session` facet methods construct missing-capability provider IDs fallibly at the driver API boundary. Fake-provider overrides and the custom-provider example follow the same error behavior, while the example shares immutable provider/target metadata between connector and session.
- Bounded event queues preallocate their declared capacity fallibly. The complete 16-bit memory-view identifier space is tracked in a fixed inline bitmap with no per-view allocation; enumeration alone reserves its exact result size fallibly.
- Translation caches preallocate their bounded map/order storage fallibly during attach, cache insertion propagates typed errors, and provider descriptor enumeration reserves exactly before cloning.
- The C ABI snapshot registry reserves fallibly before allocating and publishing a new opaque handle, so recoverable registry growth failure does not consume a token; last-error size calculation saturates instead of using unchecked addition.
- C ABI snapshot tokens use a non-wrapping atomic allocator and fail closed permanently at address-space exhaustion instead of permitting stale-token reuse.
- Production unsafe code is inventoried explicitly, with pointer, native-library, mapping, synchronization, and cleanup invariants recorded for release review.
- The locked dependency graph is checked against RustSec advisories and a committed cargo-deny policy covering licenses, duplicate versions, wildcard requirements, yanked crates, and untrusted sources.
- All 21 workspace crates inherit release descriptions, use exact-version local dependency edges, and pass isolated `cargo package --workspace` verification without resolving similarly named crates from crates.io.
- Every publishable crate carries a responsibility-specific description plus inherited repository, README, keyword, category, license, and MSRV metadata. A tested validator rejects missing files, field drift, and incomplete metadata before packaging.
- The same package validator enforces exact-version, path-backed workspace dependency edges and computes a deterministic dependency-first publication order, rejecting internal cycles before a release begins.
- A dependency-free Markdown validator checks every root and documentation Markdown file for missing or workspace-escaping local links, stale GitHub-style heading fragments (including duplicate-heading suffixes), unlabeled code fences, and unclosed fences; focused mutation tests and CI preserve the policy.
- A production panic-policy validator scans every crate source before its trailing test module and rejects `unwrap`, `expect`, `panic!`, `unreachable!`, `todo!`, and `unimplemented!`; test and benchmark assertions remain outside the production policy boundary.
- Production libraries, binaries, and examples deny `as` conversions as well as truncating, sign-losing, wrapping, and avoidable lossless casts. Numeric and pointer conversions are explicit, canonical-address checks use unsigned masks, host-width conversions are checked, and page-offset arithmetic is pointer-width independent.
- Production libraries, binaries, and examples deny direct string slicing, keeping externally derived UTF-8 boundaries on checked string APIs.
- Every workspace library denies production indexing and slicing. Artifact signatures, bitmaps, ranges, and streaming buffers use checked access; FFI handles are revalidated at lookup; Linux and Windows guest strings fail closed; profile reads, virtual-memory chunks, and sparse test memory use checked ranges; and hypervisor protocol and subprocess buffers cannot panic on malformed boundaries. The CLI remains outside this library invariant because its positional argument access is guarded by explicit command arity checks.
- Every production library, binary, and example denies arithmetic with implicit overflow side effects under all features. Core virtual I/O, memory-range containment, artifact format offsets and segment progress, profile calculations, sparse test ranges, page-table translation, hypervisor drivers, protocol codecs, FFI, CLI paths, and Linux/Windows introspection use checked or explicitly saturating operations.
- PDB type-profile extraction excludes compiler-generated Rust closure environments, whose synthetic names can legitimately identify multiple incompatible capture layouts in MSVC debug information.
- CLI argument ingestion rejects more than 64 arguments or any argument larger than 32 KiB before command dispatch, bounding allocation from externally supplied process arguments.
- Every unsafe block, including C ABI boundary tests, carries an adjacent safety explanation enforced by strict Clippy.
- The C artifact opener bounds paths to 32 KiB and fallibly owns the validated UTF-8 path before writing its output handle, so intentionally overlapping foreign input/output storage cannot invalidate a live Rust string borrow.
- C snapshot validation clones a reference-counted live entry under the registry lock and releases the global mutex before reads or segment queries. Concurrent close removes future access immediately while already-validated operations retain a safe snapshot lifetime.
- C ABI errors are isolated per thread. Exact-size buffers receive the message and trailing NUL, undersized buffers remain untouched, and successful null close clears stale error state; a caught close panic records a boundary-specific error.
- C segment count/start/length outputs are zeroed before handle or index validation, and aliased start/length destinations are rejected, preventing stale or unrepresentable scalar results on failure.
- A strict C11 consumer is compiled and dynamically linked against the release `vmi-ffi` library, then performs byte-accurate raw-memory inspection through the public header in Docker Linux and CI.
- The same consumer is statically linked through the documented native compression-library set and compiled as strict C++17, verifying `extern "C"`, `VMI_STATIC`, archive completeness, and header compatibility.
- Linux `cdylib` linkage hides symbols from bundled native archives and CI verifies that exactly the seven versioned public `vmi_*` functions are exported; the previously leaked bzip2 implementation symbol is no longer visible.
- CLI dispatch and number parsing have focused tests, accept decimal plus `0x`/`0X` hexadecimal input consistently, publish complete usage for every implemented command, and run a real offline raw-memory smoke test in Docker and CI.
- CLI operating-system arguments are collected with fallible vector growth and explicit Unicode validation, avoiding `env::args` panics on non-Unicode Unix input.
- CLI hex-dump rendering checks line-offset multiplication and address addition, so an invalid successful provider response crossing the physical-address ceiling becomes an error rather than a formatting panic.
- The machine-readable support matrix has a dependency-free validator for schema, provider inventory, identifier syntax, maturity vocabulary, capability vocabulary, duplicate claims, and required metadata; CI also enforces RustSec and cargo-deny checks.
- QEMU GDB register widths are validated before slicing/multiplication, register encoding and response growth are fallible, and QEMU memory/VBoxManage adapter collections reserve before population.
- Linux and Windows guest-list cycle detectors reserve fallibly before every guest-derived set insertion.
- ELF, legacy/bitmap/RDMP KDMP, LiME, and manifest parsers verify segment-vector capacity fallibly before every insertion.
- The C ABI rejects read lengths above `isize::MAX` before constructing a Rust slice, preventing undefined behavior from hostile foreign lengths.
- QMP connections preallocate their bounded asynchronous-event queue fallibly. Request IDs use the complete `u64` space, including the final value, then enter a permanent explicit exhausted state without wrapping.
- QMP endpoint rendering is performed once with fallible ownership and reused for named-target comparison; the required target display-name copy is also allocated fallibly before opening protocol connections.
- QMP line framing checks cumulative length and reserves fallibly before copying each buffered socket chunk, avoiding `read_until`'s infallible growth while retaining the 16 MiB message ceiling.
- QMP framing has small-limit socket regressions for both oversized newline-terminated messages and unterminated frames closed at the boundary.
- Xen range acquisition uses collision-resistant process/time/atomic-sequence temporary core names, with concurrent uniqueness coverage.
- VirtualBox and Xen extracted ranges are synchronized to same-directory create-new temporary files before atomic publication; existing destinations are never clobbered and temporary outputs are removed on failure.
- VirtualBox, Xen, and synthetic range publication now uses an atomic same-filesystem hard-link create after synchronizing the temporary inode. Unlike Unix `rename`, this fails if a destination appears concurrently and therefore provides true no-clobber publication across platforms.
- Eight-writer contention tests for both native command providers prove exactly one publication succeeds, its complete payload becomes visible, and no temporary names remain.
- Native `VBoxManage`, `xl`, and `xenctx` subprocess transports enforce configurable nonzero end-to-end deadlines (30 seconds by default, capped to a portable 24-hour maximum), kill and reap timed-out or output-flooding children, and concurrently capture stdout/stderr through bounded readers so output limits apply during execution rather than after unbounded allocation. Oversized durations cannot overflow `Instant` and silently disable the deadline, and failure paths do not block on pipe readers that a surviving descendant may keep open.
- External-command capture rejects reader limits whose `limit + 1` sentinel cannot be represented, and never follows a failed process kill with an unbounded reap wait.
- Concurrent external-command readers share one atomic byte budget and grow their buffers fallibly in fixed chunks, so stdout plus stderr—not each stream independently—obeys the documented 16 MiB ceiling.
- Bounded artifact files, compressed KDMP streams, text profiles, and VirtualBox/Xen command pipes retry interrupted reads while preserving their existing byte ceilings and fallible-growth guarantees.
- Capture overflow is checked again after both readers finish, closing the fast-child-exit race that could otherwise accept deliberately truncated command output.
- External-command setup and polling failures clean up the spawned child; fallible named reader-thread creation replaces panic-prone thread spawning.
- Bitmap KDMP table ranges use checked `usize` arithmetic and PFN-to-index conversion, preserving fail-closed behavior on 32-bit hosts and hostile page counts.
- VBoxManage, `xl`, and `xenctx` responses enforce a 16 MiB combined stdout/stderr parsing ceiling with checked length arithmetic before UTF-8 conversion.
- VirtualBox/Xen temporary-core cleanup failures are surfaced when the primary operation succeeds, while primary read/parse errors retain precedence during cleanup failure.
- QEMU, VirtualBox, and Xen snapshot acquisition refuses existing destinations before invoking vendor tooling; VirtualBox and Xen preflight uses symlink-aware metadata so dangling links cannot bypass the overwrite policy.
- Nonexistent relative QEMU acquisition destinations are resolved against the client working directory before monitor submission, avoiding accidental writes in QEMU's working directory.
- QEMU, VirtualBox, and Xen range acquisition reject overflowing `start + length` requests before invoking the hypervisor.
- Xen register parsing streams adjacent tokens without per-line collection, xenctrl write staging allocates fallibly, and QEMU/VirtualBox/Xen accept both conventional hex-prefix cases.
- Native xenctrl memory access converts GFNs to host `c_ulong` with bounds checking and reports `munmap` failures instead of discarding them.
- AMD64 and AArch64 translators return typed invariant errors instead of ending traversal with production `unreachable!()` panics.
- Legacy KDMP bootstrap metadata: CR3, PFN database, process/module anchors, debugger block, CPU count, and bugcheck data.
- LiME multi-range parsing with inclusive-range, truncation, and overlap validation.
- Versioned multi-file snapshot manifests with relative paths, file slices, sparse GPAs, and overlap validation.
- Manifest segment files are canonicalized and confined to the manifest directory; absolute paths, parent traversal, and escaping symlinks fail closed.
- Manifest loading reads only declared file slices and enforces configurable segment-count and aggregate-byte budgets (65,536 segments and 64 GiB by default).
- Snapshot manifest JSON is bounded to 16 MiB before parsing.
- Snapshot manifests use the shared fallible bounded-file reader and convert their owned byte buffer to UTF-8 without a second allocation.
- Capability-accurate manifest snapshot connector with custom provider identity and target filtering.
- Dedicated Firecracker and Cloud Hypervisor immutable snapshot connectors with stable provider IDs.
- Dedicated VMware `.vmem`/`vmss2core`, Hyper-V/LiveKd, and bhyve user-supplied ELF/KDMP conversion connectors plus manifest connectors.
- Dedicated VirtualBox `dumpvmcore` ELF provider, separate from the planned live Main API backend.
- Capability-limited live VirtualBox provider with register reads, control, core acquisition, offline range extraction, and opt-in register writes for vendor builds that implement them.
- VirtualBox register writes are read back and verified; zero-length physical acquisitions fail before invoking the hypervisor.
- VirtualBox live-session physical reads through per-read `dumpvmcore` acquisition, ELF normalization, guaranteed temporary-file cleanup, and collision-resistant process/time/sequence temporary names for concurrent reads.
- Injectable VirtualBox Main API/XPCOM memory transport preferred over `dumpvmcore`, preserving the same `MemoryAccess` contract for zero-copy adapters.
- Real VirtualBox 7.2.12 validation: a running disposable VM produced a 79.6 MB `dumpvmcore` artifact and the Rust CLI successfully inspected GPA zero; a direct COM call to `IMachineDebugger::readPhysicalMemory` returned the vendor's explicit `E_NOTIMPL` error.
- Real VirtualBox 7.2.12 register validation: `getregisters` returned live `RIP`, while `setregisters` returned vendor `E_NOTIMPL`; register-write capability is therefore disabled by default and requires explicit opt-in.
- Capability-limited Xen xl provider with control, core acquisition, and offline range extraction.
- Optional Linux Xen backend with dynamically loaded `libxenctrl` GFN mappings for direct cross-page physical reads and writes.
- Optional Xen `xenctx` backend for coherent per-vCPU general/control register reads.
- Capability-accurate Xen CPU transport integration for validated register writes; read-only xenctx sessions continue to reject writes.
- Capability-gated Xen event transport integration with timeout propagation and typed `VmiEvent` delivery for native vm_event ring adapters.
- Panic-contained C ABI with opaque ownership, thread-local errors, segment metadata, and raw/ELF/LiME/manifest reads.
- The C ABI uses monotonic opaque handle tokens backed by a synchronized Rust-owned map, rejecting foreign, stale, and double-closed handles without pointer dereferences, allocator-address reuse, or close/read races.
- A real Docker Linux C11 consumer compiles with `-Wall -Wextra -Werror`, links the static archive and native compression libraries, and reads a raw artifact successfully.
- Workspace source coverage is measured across all targets with a recorded 75.87% Windows line baseline and a 70% cross-platform regression floor in CI.
- Compile-fail rustdoc contracts prove that guest-physical addresses, guest-virtual addresses, and translation roots cannot be accidentally interchanged.
- Generated property tests cover address newtype round-trips, half-open memory ranges at overflow boundaries, arbitrary canonical AMD64 4 KiB translations, and noncanonical-address rejection.
- A release-mode core benchmark emits versioned JSON for 4 KiB physical-read latency/throughput and cached-translation latency, with a fail-closed same-machine regression comparator and an initial dated Windows baseline.
- CI verifies that the complete workspace and every target compile with default features disabled.
- Every persistent fuzz seed has an exact SHA-256/size inventory plus license, provenance, generator, architecture, byte-order, page/range context, and expected behavior; CI rejects fixture drift and path escapes.
- The deterministic testkit implements every capability it is configured to advertise: sparse memory read/write, independent register read/write, VM control, queued events, memory-view switching, and range/snapshot acquisition.
- Generated AArch64 properties validate address-width rejection and page-offset preservation across 4, 16, and 64 KiB translation granules.
- Randomized raw and ELF reads are differentially checked against an independent Python generator/oracle, including ELF zero-fill tails and adjacent-segment boundaries.
- Generated malformed Unicode/text properties exercise private QEMU GDB/HMP, VirtualBox, and Xen command parsers without expanding the public API.
- QEMU GDB register decoding operates on validated ASCII byte pairs; a minimized non-ASCII input that previously triggered a UTF-8 boundary panic is retained as a regression seed.
- The synthetic testkit supports address-specific read/write fault injection with deterministic partial-operation behavior for failure and recovery tests. Its sparse-memory contracts cover empty and unaligned operations, contiguous segment boundaries, holes, the final physical address, arithmetic overflow, and partial writes; ambiguous overlapping or address-space-overflowing maps are rejected at attach time.
- Event queues and QEMU event waits treat deadlines beyond `Instant`'s representable range as bounded repeated waits instead of immediate timeouts; socket timeout values are capped to a portable 24-hour wait slice.
- QEMU connector command timeouts are normalized to that same nonzero 24-hour maximum, preventing an unrepresentable configured duration from disabling the total QMP/GDB command deadline.
- VirtualBox and Xen command argument vectors and every owned argument string use fallible allocation before invoking their process transports, including user-controlled VM/domain names and output paths. Attachment rejects empty, option-like, or control-character-bearing target names before any transport call.
- VirtualBox and Xen temporary-core and range-publication sequence counters fail closed at exhaustion instead of wrapping and reusing a filename token.
- Their concurrent stdout/stderr readers reserve from the shared output budget with checked compare/exchange loops, preserving the limit without version-sensitive atomic APIs or counter overflow.
- QMP request correlation and GDB remote packets enforce end-to-end command deadlines, so unrelated QMP replies or byte-at-a-time GDB responses cannot reset the timeout indefinitely.
- QEMU GDB socket expiry normalizes both platform timeout representations (`TimedOut` and `WouldBlock`) to a stable backend timeout error.
- QMP buffered reads likewise normalize `TimedOut` and `WouldBlock`, so event waits no longer depend on localized or platform-specific operating-system error text.
- QMP command calls normalize a socket read timeout to the command-level timeout contract; a 30-run optimized stress loop covers the former boundary race.
- QEMU HMP physical-memory decoding now requires exact colon-delimited byte fields and rejects malformed, missing, or surplus values without silently filtering tokens.
- Public errors expose stable categories through `VmiError::kind()`; QMP, GDB, Xen, and VirtualBox deadlines now return typed timeout errors rather than requiring diagnostic-string parsing.
- Driver memory reads, event waits, and acquisition expose additive cooperative-cancellation methods backed by a cloneable thread-safe token. Default implementations check operation boundaries and return the typed cancellation error.
- Synthetic acquisition uses synchronized same-directory temporary files and atomic hard-link publication, preserving existing output and cleaning temporary artifacts when preparation fails.

## CLI

```console
cargo run -p vmi-cli -- read-raw <dump-file> <guest-physical-address> <length>
cargo run -p vmi-cli -- read-elf <vmcore> <guest-physical-address> <length>
cargo run -p vmi-cli -- read-xen-core <domain.core> <guest-physical-address> <length>
cargo run -p vmi-cli -- read-kdmp <memory.dmp> <guest-physical-address> <length>
cargo run -p vmi-cli -- read-lime <capture.lime> <guest-physical-address> <length>
cargo run -p vmi-cli -- read-manifest <snapshot.json> <guest-physical-address> <length>
cargo run -p vmi-cli -- qemu-status <host:port>
cargo run -p vmi-cli -- qemu-pause <host:port>
cargo run -p vmi-cli -- qemu-resume <host:port>
cargo run -p vmi-cli -- qemu-read <host:port> <gpa> <length>
cargo run -p vmi-cli -- qemu-reg-read <host:port> <vcpu> <register>
cargo run -p vmi-cli -- qemu-event <host:port> <timeout-ms> [pause|resume]
cargo run -p vmi-cli -- qemu-gdb-reg-write <qmp-endpoint> <gdb-host:port> [vcpu] <register> <value>
cargo run -p vmi-cli -- qemu-acquire <host:port> <output> <gpa> <length>
cargo run -p vmi-cli -- qemu-dump <host:port> <output.elf>
cargo run -p vmi-cli -- vbox-status <vm>
cargo run -p vmi-cli -- vbox-reg-read <vm> <vcpu> <register>
cargo run -p vmi-cli -- vbox-reg-write <vm> <vcpu> <register> <value>
cargo run -p vmi-cli -- profile-symbol <System.map> <name>
cargo run -p vmi-cli -- profile-nearest <System.map> <address>
cargo run -p vmi-cli -- profile-json-symbol <profile.json> <name>
cargo run -p vmi-cli -- profile-json-offset <profile.json> <name>
cargo run -p vmi-cli -- profile-pdb-symbol <file.pdb> <image-base> <name>
cargo run -p vmi-cli -- profile-pdb-offset <file.pdb> <image-base> <Type.Member>
cargo run -p vmi-cli -- linux-processes-elf <vmcore> <System.map> <cr3> <tasks_off> <pid_off> <comm_off> <comm_len> <limit>
cargo run -p vmi-cli -- windows-processes-elf <vmcore> <symbols> <cr3> <links_off> <pid_off> <image_off> <image_len> <dtb_off> <limit>
```

On Unix hosts, every QEMU command also accepts `unix:/path/to/qmp.sock` in place of `<host:port>`.

Addresses and lengths accept decimal or `0x`-prefixed hexadecimal values.

## Not Yet Implemented

- Native Windows compressed-block KDMP variants and native Hyper-V/bhyve saved-state decoding without conversion.
- Bundled native vm_event event-channel waiting and a bundled VirtualBox COM/XPCOM memory adapter.
- Live-provider wiring for target lifecycle notifications. The portable API now
  defines a negotiated lifecycle facet, monotonic generations, and typed reconnect,
  reboot, memory-topology-change, and destruction events; providers advertise it
  only after their native transport can deliver those signals reliably.

## External Blockers

- Released VirtualBox Main API documentation exposes `IMachineDebugger::readPhysicalMemory`, but the operation remains unimplemented upstream; the driver therefore retains its tested injectable direct-memory boundary and `dumpvmcore` fallback.
- The maintained MIT Rust `libxen`/`libxen-sys` stack currently fails feature compilation against both Debian Xen headers and its pinned bindings because generated vm_event symbols and ring macros disagree. The driver retains its dynamically loaded xenctrl memory backend plus tested CPU/event transport boundaries until a compatible binding release is available.
- Native Windows compressed page/block records are not publicly specified and no maintained parser located during the audit implements them. File-level gzip, zlib, bzip2, xz, and zstd wrappers are supported and strictly validated.

See the [implementation plan](../implementation-plan.md) for sequencing and
acceptance criteria.
