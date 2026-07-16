# LibVMI-Rust — Project Structure & File Reference

> **Planning status:** This is the legacy Xen/KVM-oriented structure sketch. Do not use it to scaffold the implementation. The authoritative multi-hypervisor crate plan and capability model are in [implementation-plan.md](implementation-plan.md#proposed-workspace).

## Workspace Layout

```text
libvmi-rust/
├── Cargo.toml                         # Workspace root
├── crates/
│   ├── vmi-core/                      # Core types, traits, address translation
│   ├── vmi-kmod/                      # Rust kernel module (KVM hooks, EPT, MTF, ring buffer)
│   ├── vmi-driver-xen/                # Xen hypervisor driver
│   ├── vmi-driver-kvm/                # KVM userspace driver (uses vmi-kmod)
│   ├── vmi-driver-dump/               # Offline memory dump driver
│   ├── vmi-os-windows/                # Windows OS profile
│   └── vmi-os-linux/                  # Linux OS profile
├── examples/                          # Usage examples
└── tests/                             # Integration tests & fixtures
```

---

## Crate: `vmi-core` — Core Engine

Foundation crate. All other crates depend on this.

| File | Purpose | Key Types / Traits |
|------|---------|-------------------|
| `lib.rs` | Crate root, re-exports public API | `VmiCore<D>` — main entry point |
| `address.rs` | Type-safe address wrappers (compile-time safety) | `Va`, `Pa`, `Gfn`, `Dtb` — prevent mixing virtual/physical addresses |
| `memory.rs` | Memory access trait and high-level read/write API | `MemoryAccess` trait, `read_va()`, `read_pa()`, `read_string_va()`, `write_va()` |
| `registers.rs` | CPU register types and access | `Register` enum (RAX-R15, CR0-CR4, MSR, etc.), `RegisterSet` |
| `events.rs` | Event types and dispatch system | `VmiEvent` enum, `MemAccess` flags, `EventResponse`, `EventCallbackId` |
| `cache.rs` | LRU caches for V2P, symbols, pages, PIDs | `CacheManager` — `v2p_lookup()`, `v2p_insert()`, `flush_all()` |
| `paging.rs` | Page table walker (4-level IA-32e + 5-level LA57) | `walk_ia32e()`, `walk_la57()`, `PageMode` enum |
| `error.rs` | Error types for the entire framework | `VmiError` enum — `TranslationFailed`, `PageNotPresent`, `SymbolNotFound`, etc. |

### Core Traits Defined in `vmi-core`

| Trait | File | Purpose | Implementors |
|-------|------|---------|-------------|
| `VmiDriver` | `memory.rs` | Raw hypervisor memory/register operations | `XenDriver`, `KvmDriver`, `DumpDriver` |
| `VmiEventDriver` | `events.rs` | Event registration and listening (extends `VmiDriver`) | `XenDriver`, QEMU lifecycle events |
| `OsProfile` | `lib.rs` | OS-aware introspection (processes, modules, symbols) | `WindowsOs`, `LinuxOs` |

---

## Crate: `vmi-kmod` — Rust Kernel Module (KVM Hooks)

Custom kernel module that hooks KVM internals via ftrace. Provides full VMI event capabilities on KVM without kernel patches.

| File | Purpose | Key Functions |
|------|---------|--------------|
| `lib.rs` | Module init, `/dev/vmi-kvm` ioctl dispatch | `init()`, `ioctl()` — 13 ioctl commands |
| `ept_hooks.rs` | ftrace hook on `handle_ept_violation()` | Capture GPA, GVA, access type (R/W/X) on every EPT violation |
| `ept_access.rs` | EPT page table walk and permission control | `set_page_access()`, `ept_walk_to_pte()`, `invept_single_context()` |
| `cr_hooks.rs` | ftrace hook on `handle_cr()` | Capture CR0/CR3/CR4 writes with old/new values |
| `singlestep.rs` | Monitor Trap Flag (MTF) control and hook | `enable()`, `disable()` — set/clear MTF bit in VMCS |
| `shadow_pages.rs` | Stealthy breakpoints via EPT shadow page swapping | `install_breakpoint()` — dual-view: execute INT3 page, read clean page |
| `ring_buffer.rs` | Lock-free shared memory ring buffer + eventfd | `push()` — zero-copy event delivery to userspace (~3-10 us) |
| `vmcs.rs` | VMCS field read/write via `vmread`/`vmwrite` instructions | `vmcs_read64()`, `vmcs_write64()`, `invept_single_context()` |

### Key Design Decisions

| Decision | Why |
|----------|-----|
| ftrace hooks (not kprobes) | More stable, lower overhead, can replace function entry |
| Shared memory ring buffer (not socket) | ~3-10 us latency vs KVMi's ~10-50 us socket IPC |
| Bypass QEMU entirely | Events go KVM → module → userspace (1 context switch vs 3) |
| Loadable module (not patches) | Zero kernel source modifications, works on stock kernels |

---

## Crate: `vmi-driver-xen` — Xen Hypervisor Driver

Implements `VmiDriver` + `VmiEventDriver` for Xen via the `xen` crate (rust-vmm).

| File | Purpose | Key Functions |
|------|---------|--------------|
| `lib.rs` | Driver initialization, `XenDriver` struct | `XenDriver::new(domain_id)`, `XenDriver::connect()` |
| `memory.rs` | Xen memory operations via xenctrl | `read_physical()` → `xc_map_foreign_range()`, `write_physical()` |
| `events.rs` | vm_event ring buffer + altp2m integration | `register_event()` → `xc_monitor_*`, `listen()` → `xc_vm_event_get_request()` |
| `registers.rs` | vCPU register access via Xen hypercalls | `read_register()` → `xc_vcpu_getcontext()`, `write_register()` |

### Xen-Specific Features

| Feature | API | Description |
|---------|-----|-------------|
| vm_event ring buffer | `xc_vm_event_get_request/put_response` | Shared memory event delivery (~5-20 us) |
| altp2m views | `xc_altp2m_create/switch/set_mem_access` | Multiple EPT views for stealthy breakpoints |
| Domain forking | `xc_memc_fork` | Clone VM state for parallel analysis |
| Memory sharing | `xc_sharing_*` | Share pages between domains |

---

## Crate: `vmi-driver-kvm` — KVM Hypervisor Driver

Implements `VmiDriver` for KVM via `kvm-ioctls` (rust-vmm). Events not supported without KVMi patches.

| File | Purpose | Key Functions |
|------|---------|--------------|
| `lib.rs` | Driver initialization, `KvmDriver` struct | `KvmDriver::new(vm_name)`, connection via libkvmi socket or memflow |
| `memory.rs` | KVM memory access (multiple backends) | `read_physical()` via memflow-kvm (zero-copy) or KVMI_VM_READ_PHYSICAL |
| `registers.rs` | vCPU register access via KVM ioctls | `read_register()` → `KVM_GET_REGS`, `write_register()` → `KVM_SET_REGS` |

### KVM Backend Options

| Backend | File Interaction | Patches? | Memory | Registers | Events |
|---------|-----------------|----------|--------|-----------|--------|
| memflow-kvm | Kernel module | Module only | Read | No | No |
| libkvmi | Socket to QEMU | Kernel + QEMU | R/W | R/W | 14 types |
| /proc/pid/mem | Direct read | None | Read | No | No |

---

## Crate: `vmi-driver-dump` — Offline Memory Dump Driver

Implements `VmiDriver` (read-only) for forensic analysis of memory dumps.

| File | Purpose | Key Functions |
|------|---------|--------------|
| `lib.rs` | Driver initialization, `DumpDriver` struct | `DumpDriver::open(path, format)` |
| `lime.rs` | LiME format parser (Linux memory dumps) | `LimeReader::new()`, parse LiME header, address range mapping |
| `raw.rs` | Raw/flat memory dump reader | `RawReader::new()`, direct offset-based access |
| `kdump.rs` | Windows crash dump (DMP) parser | `KdumpReader::new()`, parse DUMP_HEADER64, physical page mapping |

### Supported Dump Formats

| Format | Extension | Source | Crate Dependency |
|--------|-----------|--------|-----------------|
| LiME | `.lime` | AVML, LiME kernel module | `memmap2` |
| Raw | `.raw`, `.bin` | dd, QEMU `pmemsave` | `memmap2` |
| Windows DMP | `.dmp` | BSOD, WinDbg, LiveKd | `memmap2`, `scroll` |
| Xen Core | `.core` | `xl dump-core` | `memmap2` |

---

## Crate: `vmi-os-windows` — Windows OS Profile

Implements `OsProfile` for Windows 7/8/10/11/Server.

| File | Purpose | Key Functions |
|------|---------|--------------|
| `lib.rs` | `WindowsOs` struct, `OsProfile` impl | `WindowsOs::init()`, `processes()`, `kernel_modules()` |
| `eprocess.rs` | Process enumeration via EPROCESS linked list | Walk `ActiveProcessLinks`, read PID/PPID/name/DTB/ImageFileName |
| `modules.rs` | Module listing via PEB → LDR_DATA_TABLE_ENTRY | Enumerate DLLs per process, get base address/size/path |
| `registry.rs` | Registry hive parsing from memory | Read CMHIVE, parse key/value structures |
| `network.rs` | Network connection enumeration | Parse TCP/UDP endpoint structures (TcpE, UdpA pool tags) |
| `profiles.rs` | OS profile loading (struct offsets) | Load IST JSON (Volatility3) or parse PDB symbols for EPROCESS offsets |

### Windows Kernel Structures Parsed

| Structure | Offset Source | Used For |
|-----------|--------------|----------|
| `EPROCESS` | PDB / IST JSON | Process enumeration |
| `PEB` | PDB / IST JSON | Process environment, loaded modules |
| `LDR_DATA_TABLE_ENTRY` | PDB / IST JSON | DLL/module listing |
| `KPCR` / `KPRCB` | PDB / IST JSON | Per-CPU data, current thread |
| `OBJECT_HEADER` | PDB / IST JSON | Kernel object metadata |
| `CMHIVE` | PDB / IST JSON | Registry hive roots |
| `FILE_OBJECT` | PDB / IST JSON | Open file handles |

---

## Crate: `vmi-os-linux` — Linux OS Profile

Implements `OsProfile` for Linux 2.6–6.x.

| File | Purpose | Key Functions |
|------|---------|--------------|
| `lib.rs` | `LinuxOs` struct, `OsProfile` impl | `LinuxOs::init()`, `processes()`, `kernel_modules()` |
| `task_struct.rs` | Process enumeration via `task_struct` linked list | Walk `tasks` list, read pid/tgid/comm/mm→pgd |
| `modules.rs` | Kernel module listing via `modules` list | Enumerate `struct module`, get name/base/size |
| `network.rs` | Network socket/connection parsing | Parse `inet_hashtable`, TCP/UDP sockets |
| `profiles.rs` | OS profile loading (struct offsets) | Parse `System.map` for symbol addresses, DWARF for struct offsets |

### Linux Kernel Structures Parsed

| Structure | Offset Source | Used For |
|-----------|--------------|----------|
| `task_struct` | DWARF / System.map | Process enumeration |
| `mm_struct` | DWARF | Memory management, pgd (page directory) |
| `vm_area_struct` | DWARF | Process memory mappings (VMAs) |
| `module` | DWARF / System.map | Kernel module listing |
| `dentry` / `inode` | DWARF | File system objects |
| `sock` / `inet_sock` | DWARF | Network connections |
| `cred` | DWARF | Process credentials (uid/gid) |

---

## Examples

| File | Purpose | Demonstrates |
|------|---------|-------------|
| `process_list.rs` | List all processes running in a VM | `OsProfile::processes()`, basic VmiCore usage |
| `memory_dump.rs` | Dump a memory region from a VM to file | `VmiDriver::read_physical()`, address ranges |
| `event_monitor.rs` | Monitor memory/register events in real-time | `VmiEventDriver::register_event()`, `listen()` loop |
| `rootkit_detector.rs` | Detect hidden processes (DKOM rootkits) | EPROCESS list walk vs physical memory pool tag scan |

---

## Dependency Summary

| Crate | External Dependencies | Purpose |
|-------|----------------------|---------|
| `vmi-core` | `nix`, `lru`, `zerocopy`, `bitflags`, `thiserror`, `tracing` | System calls, caching, binary parsing, errors |
| `vmi-driver-xen` | `xen` (rust-vmm), `vmi-core` | Xen hypercalls and vm_event |
| `vmi-driver-kvm` | `kvm-ioctls` (rust-vmm), `vmi-core` | KVM ioctl wrappers |
| `vmi-driver-dump` | `memmap2`, `scroll`, `vmi-core` | Memory-mapped file I/O |
| `vmi-os-windows` | `pdb`, `goblin`, `vmi-core` | PDB symbol parsing, PE binary parsing |
| `vmi-os-linux` | `goblin`, `vmi-core` | ELF binary parsing |

---

## File Count Summary

| Crate | `.rs` Files | Lines (est.) | Phase |
|-------|-------------|-------------|-------|
| `vmi-core` | 8 | ~1,500 | Phase 2 |
| `vmi-driver-xen` | 4 | ~800 | Phase 2 |
| `vmi-driver-kvm` | 3 | ~500 | Phase 2 |
| `vmi-driver-dump` | 4 | ~600 | Phase 2 |
| `vmi-os-windows` | 6 | ~1,200 | Phase 3 |
| `vmi-os-linux` | 5 | ~800 | Phase 3 |
| `examples` | 4 | ~400 | Phase 1-3 |
| **Total** | **34** | **~5,800** | |
