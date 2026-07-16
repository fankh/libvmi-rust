# Guest OS and Profile Qualification

The `os-profiles` release gate separates parser/adapter safety from broad guest
compatibility. Parser and adapter qualification is complete for v1; the real-
artifact matrix below is explicitly deferred and is not a v1 production claim.

## Current Contract

- Linux symbol ingestion supports bounded, strictly validated `System.map` files.
- Windows symbol ingestion supports bounded native PDB public symbols and type
  member offsets, including OMAP relocation and continued field lists.
- Normalized JSON profiles require explicit unsigned symbol addresses and offsets.
- Linux and Windows object walkers use checked addresses, bounded traversal, and
  fail closed on corrupt lists, strings, ranges, or missing profile fields.
- Profile parsing and both guest adapters are release-tested on Linux and Windows
  hosts in the dedicated `OS/profile qualification` CI job. Windows additionally
  exercises parsing of the native PDB produced for the test binary.

These statements describe parser and adapter behavior. They do not yet claim that
every kernel build in a distribution or Windows servicing channel is compatible.

## Required Real-Artifact Matrix

| Guest family | Minimum v1 evidence | Status |
|---|---|---|
| Linux LTS | Ubuntu 22.04 and 24.04 x86-64 kernels, matching `System.map`, process and module enumeration | Pending |
| Linux enterprise | One current RHEL-compatible 9.x x86-64 kernel, process and module enumeration | Pending |
| Windows client | Windows 11 x64 current servicing release, matching Microsoft PDB, process and module enumeration | Pending |
| Windows server | Windows Server 2022 x64, matching Microsoft PDB, process and module enumeration | Pending |

Each retained result must identify the exact guest build, profile provenance and
digest, acquisition provider, commands exercised, expected object counts, and any
accepted limitation. Guest images and vendor symbols must not be committed when
their licenses prohibit redistribution; store only metadata, digests, and test
results.

## Post-v1 Promotion Criteria

Broad guest compatibility may be claimed only after all four rows pass on immutable
memory artifacts, corruption variants demonstrate bounded failure, and the evidence
is retained by a release workflow. A kernel/profile mismatch, absent required symbol,
invalid pointer, traversal cycle, or unsupported layout must return a typed error
without partial success being presented as a complete inventory.
