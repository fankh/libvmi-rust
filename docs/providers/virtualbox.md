# VirtualBox Provider Qualification

## v1 Contract

The `virtualbox` live provider remains Experimental for v1 on Linux, Windows, and
macOS hosts. It advertises physical-memory read, register read,
control, and acquisition. Register write is optional and must be explicitly
enabled only with a transport whose vendor build implements it.

The default physical-memory path acquires a temporary VM core with `dumpvmcore`
and reads the requested range from normalized ELF `PT_LOAD` segments. It is not a
zero-copy live read, and its effective consistency depends on VM state during
acquisition. `virtualbox-core` separately targets Supported as an immutable
offline artifact provider.

## Capability Evidence

| Capability | Mechanism | Evidence |
| --- | --- | --- |
| Memory read | Temporary `dumpvmcore` plus ELF normalization | Sparse/range reads, cleanup precedence, size bounds, malformed core handling, and real 7.2.12 acquisition/re-read |
| Register read | `VBoxManage debugvm getregisters` | Alias/number parsing, vCPU selection, bounded output, and malformed text properties |
| Control | `VBoxManage showvminfo/controlvm` | State mapping and deterministic pause/resume command tests |
| Acquisition | `dumpvmcore` and atomic range publication | Destination preflight, symlink handling, synchronization, collision, cleanup, and no-clobber tests |
| Register write | Optional vendor transport | Capability builder isolation, exact assignment formatting, and write/read transport tests |

All vendor subprocesses have total deadlines, bounded stdout/stderr readers,
kill/reap behavior, fallible argument ownership, and fail-closed target validation.
Acquisition uses create-new same-directory temporary files, synchronization, and
hard-link publication so a concurrently created destination is never overwritten.

## Known Limits and Release Evidence

- Oracle VirtualBox 7.2.12 returned `E_NOTIMPL` from
  `IMachineDebugger::readPhysicalMemory` during real-host validation. The provider
  therefore retains its tested direct-memory injection seam but defaults to the
  slower core-acquisition path.
- Register write depends on vendor implementation of
  `IMachineDebugger::setRegister`; it is not advertised by the default connector.
- The provider exposes management events only through portable mechanisms it can
  verify; it does not claim guest memory-access events or alternate views.
- Post-v1 Preview promotion requires the same real-host status/register/control/core suite
  on each claimed host OS, plus permission, disk-full, process termination, and
  repeated-acquisition resource tests. Current retained evidence covers a real
  VirtualBox 7.2.12 host and deterministic cross-platform transport contracts, but
  not the complete three-host matrix.
