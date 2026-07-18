# Libvirt Linux Process Inspection — 2026-07-18

## Result

**Passed for Linux process enumeration.** The release build at commit `951d9a5`
enumerated 114 tasks from a real Ubuntu 24.04 guest memory image acquired through
the `libvirt-qemu` provider.

## Guest and profile

| Property | Value |
|---|---|
| Guest | Ubuntu Minimal 24.04 LTS x86-64 |
| Cloud image SHA-256 | `484a0b828d2bf0d7a3f778d1f27cce2f8a1e6e2bf3d3c5a6adae57cd47fdc6e3` |
| Kernel | `6.8.0-136-generic` |
| VM | 2 vCPUs, 1 GiB RAM, QEMU 8.2.2 TCG, OVMF UEFI |
| Acquisition | libvirt memory-only ELF dump, 1,073,743,939 bytes |
| Kernel CR3 | `0x074a0000`, captured from kernel-mode vCPU 1 |
| `init_task` | `0xffffffff8360fb40`, matching `nokaslr` System.map |
| `task_struct.tasks` | `0x8e8`, derived from matching kernel BTF |
| `task_struct.pid` | `0x9b8`, derived from matching kernel BTF |
| `task_struct.comm` | `0xbd8`, 16 bytes, derived from matching kernel BTF |

The official minimal cloud image required OVMF and an explicit netplan DHCP
configuration. KASLR was disabled for the final capture so the retained
`System.map` symbol had unambiguous runtime provenance.

## Command

```console
vmi-cli linux-processes-elf memory.core init-task.map \
  0x074a0000 0x8e8 0x9b8 0xbd8 16 4096
```

## Observed applications and services

The walker returned 114 tasks. Representative memory-derived results included:

| PID | Command |
|---:|---|
| 0 | `swapper/0` |
| 1 | `systemd` |
| 124, 191 | `psimon` |
| 129 | `systemd-journal` |
| 195 | `systemd-network` |
| 240 | `systemd-resolve` |
| 362 | `dbus-daemon` |
| 382 | `snapd` |
| 383 | `systemd-logind` |
| 421 | `unattended-upgr` |

Kernel workers and storage, journaling, console, and device-management tasks were
also traversed. The task-list walk terminated normally rather than at its 4,096
entry safety limit.

## Negative evidence and limitations

- A first attempt used a runtime `init_task` symbol retained from a different
  KASLR boot. The walker failed closed on an unmapped page and returned no partial
  inventory.
- Ubuntu's full `System.map` contains duplicate symbol names and was rejected by
  the strict profile parser. A minimal profile containing the single required
  `init_task` symbol was used for the successful run.
- Two custom marker processes were configured, but their service stopped during
  an earlier failed in-guest BTF export and they were not present in the final
  memory inventory. They are not claimed as passing evidence.
- Module enumeration and comparison with a simultaneous in-guest `ps` snapshot
  were not performed. The broader Ubuntu LTS OS/profile matrix therefore remains
  pending.

## Cleanup

The guest was destroyed and undefined. Its overlay, seed image, NVRAM, memory
cores, extracted profiles, and temporary scripts were removed. The final libvirt
domain list was empty. The cached upstream Ubuntu base image remains on the host.

