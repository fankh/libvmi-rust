# VMware Artifact Evidence

`synthetic-qualification.json` is deterministic connector evidence generated on
Windows 11. It covers a 4 MiB contiguous `.vmem` fixture and a 4 MiB ELF core
representative of `vmss2core` output.

Both paths passed 25 repeated 4 KiB reads. The `.vmem` path also passed an
explicit out-of-range failure check. Mean complete CLI invocation latency was
7.679 ms for `.vmem` and 16.465 ms for the converted core. These timings include
process startup, artifact opening and validation, provider attachment, reading,
hex formatting, and clean shutdown.

This evidence is synthetic and does not promote direct `.vmem` support beyond
Experimental. A vendor-captured qualification file must set `source` to
`vendor-captured` and retain the artifact hashes produced by the qualifier.
Guest memory bytes are never copied into the report.
