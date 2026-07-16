# KVM-Based Virtual Machine Introspection (VMI) — Comprehensive Research

## Table of Contents
1. [All Known KVM Introspection Approaches](#1-all-known-kvm-introspection-approaches)
2. [KVMi Technical Details](#2-kvmi-technical-details)
3. [KVM VMI Tools and Projects](#3-kvm-vmi-tools-and-projects)
4. [KVM VMI Limitations vs Xen VMI](#4-kvm-vmi-limitations-vs-xen-vmi)
5. [Practical Setup Guide](#5-practical-setup-kvm-vmi-from-scratch)

---

## 1. All Known KVM Introspection Approaches

### 1.1 KVMi (KVM Introspection Subsystem) — Bitdefender Kernel Patches

**Status**: NOT merged upstream. Still in RFC/patch stage after 5+ years of effort.

**Maintainer**: Adalbert Lazar (alazar@bitdefender.com), Bitdefender. Later RFC variants by Mickael Salaun (mic@digikod.net) for "Hypervisor-Enforced Kernel Integrity."

**Patch History**:
| Version | Date | Patches | Notes |
|---------|------|---------|-------|
| v9 | July 2020 | 84 | Fixed non-x86 build issues |
| v10 | November 2020 | 81 | |
| v11 | December 2020 | 81 | |
| v12 | October 2021 | 77 | Last full series from Bitdefender |
| RFC v1 (EPT Views) | July 2020 | 34 | Alternative approach |
| RFC v2 (HEKI) | November 2023 | 19 | Mickael Salaun's reworked approach |
| RFC v3 (CR pinning) | May 2024 | 5 | Minimal subset |

**How it works**: KVMi adds a socket-based introspection API between a modified QEMU and an external introspection application. The introspection app connects via a Unix domain socket. QEMU is launched with special arguments:
```text
-chardev socket,path=/tmp/introspector,id=chardev0,reconnect=10
-object introspection,id=kvmi,chardev=chardev0
```

**Kernel config required**: `CONFIG_KVM_INTROSPECTION=y`

**API**: Message-based protocol over Unix socket. Each message has a header (`struct kvmi_msg_hdr` with id, size, seq fields). Max message size = 8192 - header bytes.

### 1.2 QEMU Monitor Protocol (QMP) for Memory Access

QMP provides several commands for memory access from the host:

| QMP Command | Purpose |
|-------------|---------|
| `memsave` | Save virtual memory region to file |
| `pmemsave` | Save physical memory region to file |
| `dump-guest-memory` | Full guest memory dump (ELF/kdump format) |
| `human-monitor-command` | Execute HMP commands including `xp` (examine physical memory) |
| `query-memory-devices` | List memory devices |
| `query-memory-size-summary` | Get total memory config |

**Usage via libvirt**: `virDomainQemuMonitorCommand()` sends QMP commands programmatically.

**Limitations**: QMP is designed for management, not real-time introspection. No event-based monitoring. Memory reads via `pmemsave` write to files (slow). The `xp` command (via `human-monitor-command`) returns formatted text that must be parsed.

LibVMI's legacy KVM driver uses this as a fallback ("native" mode) — it calls `exec_xp()` to read memory word-by-word via QMP, which is extremely slow.

### 1.3 /dev/kvm ioctl Interface

The standard KVM ioctl API provides register and some memory access:

**Register Access**:
- `KVM_GET_REGS` / `KVM_SET_REGS` — general-purpose registers (vcpu ioctl)
- `KVM_GET_SREGS` / `KVM_SET_SREGS` — segment registers, CR0-CR8 (vcpu ioctl)
- `KVM_GET_ONE_REG` / `KVM_SET_ONE_REG` — individual register access
- `KVM_GET_MSRS` / `KVM_SET_MSRS` — MSR access

**Memory Operations**:
- `KVM_SET_USER_MEMORY_REGION` — create/modify/delete guest physical memory slots (vm ioctl)
- `KVM_TRANSLATE` — translate GVA to GPA using current VCPU paging mode (vcpu ioctl, x86)
- `KVM_GET_DIRTY_LOG` — get bitmap of dirtied pages

**Limitations**: These ioctls are designed for the VMM (QEMU) process that owns the VM. An external process cannot use them directly — you'd need to be the QEMU process or inject code into it. No event-based introspection (no CR write notifications, no EPT violation callbacks, etc.).

### 1.4 libkvmi — Bitdefender's Userspace Library

**Repo**: https://github.com/bitdefender/libkvmi
**License**: LGPL-3.0
**Language**: C (98.6%)
**Latest Release**: v1.1.0 (March 2023)
**Stars**: 48

libkvmi wraps the low-level KVMi socket protocol into a C library API. It requires:
- Patched kernel with `CONFIG_KVM_INTROSPECTION`
- Patched QEMU with VMI support (kvmi-v7 branch)

**Build**:
```bash
git clone https://github.com/bitdefender/libkvmi --branch kvmi-v7
./bootstrap && ./configure && make && sudo make install
```

**Test**: `./examples/hookguest-libkvmi /tmp/introspector`

The library connects to the QEMU introspection socket and provides C functions for all KVMI commands/events. LibVMI wraps this library for its KVM driver (non-legacy mode).

### 1.5 kvm-vmi Project (KVM Fork with VMI Patches)

**Repo**: https://github.com/KVM-VMI/kvm-vmi
**Maintainer**: @Wenzel (Mathieu Tarral)
**Stars**: 362 | **Forks**: 64 | **Commits**: 488
**License**: GPLv3

An integrated project providing:
- **kvm**: Patched Linux kernel (5.4.24 base, kvmi-v7 branch)
- **qemu**: Modified QEMU with introspection socket support
- **nitro**: Legacy userland introspection library (syscall tracing)
- **libvmi**: Unified API working across Xen and KVM

Includes Vagrant setup for automated provisioning. Presented at KVM Forum 2017 and 2019.

### 1.6 memflow-kvm Connector

**Repo**: https://github.com/memflow/memflow-kvm
**Stars**: 49 | **Releases**: 13 (latest v0.2.1, October 2024)
**License**: MIT (connector) / GPL-2 (kernel module)

**Architecture**: Three components:
1. **memflow-kmod** — Linux kernel module that maps all KVM VM pages into the introspector's address space ("userspace-to-userspace DMA")
2. **memflow-kvm-ioctl** — Rust ioctl interface to the kernel module
3. **memflow-kvm** — Physical memory connector implementing the memflow protocol

**How it works**: The kernel module uses `CONFIG_KALLSYMS=y` and `CONFIG_KALLSYMS_ALL=y` to find KVM internal structures. It maps guest physical pages directly into the introspector process, enabling zero-copy memory reads at near-native speed.

**Setup**:
```bash
# Install via DKMS (Debian/Ubuntu)
sudo dpkg -i memflow-dkms_*.deb
sudo modprobe memflow
# Create memflow group and udev rules
sudo groupadd memflow
# Copy udev rules to /etc/udev/rules.d/
```

**Key advantage**: No kernel patches needed (just a loadable module). Works with unmodified KVM/QEMU. Very fast memory reads.

**Limitation**: Read-only memory access. No event interception (no CR/MSR/PF monitoring). No register access. Pure memory introspection only.

### 1.7 Direct /proc/pid/mem Access to QEMU Process

QEMU runs as a regular Linux process. Guest physical memory is mapped into QEMU's virtual address space via `mmap()`. You can read it directly:

```bash
# Find QEMU PID
QEMU_PID=$(pgrep -f "qemu-system.*vm-name")

# Read guest physical memory via /proc
cat /proc/$QEMU_PID/maps  # Find memory regions
dd if=/proc/$QEMU_PID/mem bs=1 skip=$OFFSET count=$LENGTH
```

**memflow's QEMU connector** uses exactly this approach — it reads `/proc/pid/mem` of the QEMU process with `CAP_SYS_PTRACE` capability.

**How to map GPA to QEMU VA**: Parse QEMU's memory slot layout from `/proc/pid/maps` or use QMP to query `query-memory-devices`. The guest RAM typically appears as a large anonymous mapping.

**Advantages**: No kernel patches. No kernel module. Works with any KVM/QEMU version.

**Limitations**: Requires root or `CAP_SYS_PTRACE`. No write access guarantees (race conditions). No event interception. Must reverse-engineer QEMU's memory layout. Slow for random access patterns (page fault per uncached page).

### 1.8 GDB Stub Approach

QEMU has a built-in GDB server:
```bash
qemu-system-x86_64 ... -gdb tcp::1234 -S
# Or attach to running VM via QMP:
# {"execute": "human-monitor-command", "arguments": {"command-line": "gdbserver tcp::1234"}}
```

Then connect with GDB:
```bash
gdb -ex "target remote :1234"
```

**Capabilities**: Read/write registers, read/write memory (virtual and physical via `monitor` command), set breakpoints, single-step.

**LibVMI legacy approach**: Earlier LibVMI KVM drivers used a patched QEMU that exposed a custom Unix domain socket. The `struct request` protocol was simple:
```c
struct request {
    uint64_t type;    // 0=quit, 1=read, 2=write
    uint64_t address; // physical address
    uint64_t length;  // byte count
};
```

**Limitations**: Pauses the entire VM during debugging. Single-threaded. Very slow for bulk memory reads. No selective event monitoring.

---

## 2. KVMi Technical Details

### 2.1 Kernel Patches Required

The KVMi patches modify the following kernel subsystems:
- `virt/kvm/` — Core KVM introspection infrastructure
- `arch/x86/kvm/` — x86-specific VMI hooks (EPT violations, CR writes, MSR intercepts)
- `include/uapi/linux/kvmi.h` — Userspace API header
- `arch/x86/include/uapi/asm/kvmi.h` — x86-specific structures

**Kernel config options**:
```text
CONFIG_KVM=m
CONFIG_KVM_INTEL=m (or CONFIG_KVM_AMD=m)
CONFIG_KSM=n                    # Kernel Same-page Merging must be disabled
CONFIG_REMOTE_MAPPING=y         # New option added by patches
CONFIG_KVM_INTROSPECTION=y      # The introspection subsystem itself
```

### 2.2 Supported Kernel Versions

The kvm-vmi project's primary branch is based on **Linux 5.4.24** (kvmi-v7). The upstream patch submissions targeted:
- v9-v11: Linux 5.x series
- v12 (October 2021): Linux 5.x
- RFC v2/v3 (2023-2024): More recent kernels but with reduced scope

No patches exist for kernels >= 6.x from Bitdefender. The community kvm-vmi project has not been updated to recent kernels.

### 2.3 Complete Command Set (from kvmi.h, kvmi-v7 branch)

```c
enum {
    KVMI_VERSION = 0x00000001,

    // Protocol
    KVMI_EVENT_REPLY              = 0,
    KVMI_EVENT                    = 1,
    KVMI_GET_VERSION              = 2,

    // VM-level commands
    KVMI_VM_CHECK_COMMAND         = 3,   // Check if command is supported
    KVMI_VM_CHECK_EVENT           = 4,   // Check if event is supported
    KVMI_VM_GET_INFO              = 5,   // Get VM info (vcpu count)
    KVMI_VM_CONTROL_EVENTS        = 8,   // Enable/disable VM events
    KVMI_VM_READ_PHYSICAL         = 17,  // Read guest physical memory
    KVMI_VM_WRITE_PHYSICAL        = 18,  // Write guest physical memory
    KVMI_VM_QUERY_PHYSICAL        = 39,  // Query physical page info
    KVMI_VM_SET_PAGE_ACCESS       = 21,  // Set R/W/X permissions on pages
    KVMI_VM_GET_MAX_GFN           = 29,  // Get maximum guest frame number
    KVMI_VM_GET_NEXT_AVAILABLE_GFN = 31, // Next available GFN
    KVMI_VM_GET_MAP_TOKEN         = 22,  // Get memory mapping token
    KVMI_VM_CONTROL_CMD_RESPONSE  = 27,  // Control command responses
    KVMI_VM_CONTROL_SPP           = 24,  // Sub-page protection control
    KVMI_VM_SET_PAGE_WRITE_BITMAP = 26,  // Sub-page write bitmap
    KVMI_VM_SET_PAGE_SVE          = 30,  // Suppress #VE

    // EPT View management
    KVMI_VM_CREATE_EPT_VIEW       = 43,
    KVMI_VM_DESTROY_EPT_VIEW      = 44,

    // vCPU-level commands
    KVMI_VCPU_GET_INFO            = 6,   // Get vCPU info (TSC speed)
    KVMI_VCPU_PAUSE               = 7,   // Pause a vCPU
    KVMI_VCPU_CONTROL_EVENTS      = 9,   // Enable/disable vCPU events
    KVMI_VCPU_CONTROL_CR          = 10,  // Enable CR interception
    KVMI_VCPU_CONTROL_MSR         = 11,  // Enable MSR interception
    KVMI_VCPU_GET_REGISTERS       = 13,  // Read registers + selected MSRs
    KVMI_VCPU_SET_REGISTERS       = 14,  // Write registers
    KVMI_VCPU_GET_CPUID           = 15,  // Execute CPUID
    KVMI_VCPU_GET_XSAVE           = 16,  // Get XSAVE state
    KVMI_VCPU_SET_XSAVE           = 38,  // Set XSAVE state
    KVMI_VCPU_INJECT_EXCEPTION    = 19,  // Inject exception into guest
    KVMI_VCPU_GET_MTRR_TYPE       = 23,  // Get MTRR type for GPA
    KVMI_VCPU_TRANSLATE_GVA       = 35,  // GVA -> GPA translation
    KVMI_VCPU_GET_XCR             = 37,  // Get extended control register
    KVMI_VCPU_CONTROL_SINGLESTEP  = 63,  // Enable/disable single-stepping
    KVMI_VCPU_GET_EPT_VIEW        = 34,  // Get current EPT view
    KVMI_VCPU_SET_EPT_VIEW        = 32,  // Switch EPT view
    KVMI_VCPU_CONTROL_EPT_VIEW    = 36,  // Control EPT view visibility
    KVMI_VCPU_SET_VE_INFO         = 28,  // Set #VE info page
    KVMI_VCPU_DISABLE_VE          = 33,  // Disable #VE
    KVMI_VCPU_CHANGE_GFN          = 60,  // Remap GFN in EPT view
    KVMI_VCPU_ALLOC_GFN           = 41,  // Allocate guest frame
    KVMI_VCPU_FREE_GFN            = 42,  // Free guest frame
};
```

### 2.4 Complete Event Set

```c
enum {
    KVMI_EVENT_UNHOOK       = 0,   // Introspection disconnecting
    KVMI_EVENT_CR           = 1,   // Control register write (CR0/CR3/CR4)
    KVMI_EVENT_MSR          = 2,   // MSR write
    KVMI_EVENT_XSETBV       = 3,   // XSETBV instruction
    KVMI_EVENT_BREAKPOINT   = 4,   // INT3 breakpoint hit
    KVMI_EVENT_HYPERCALL    = 5,   // VMCALL/VMMCALL
    KVMI_EVENT_PF           = 6,   // EPT violation (page fault)
    KVMI_EVENT_TRAP         = 7,   // Debug trap (after single-step)
    KVMI_EVENT_DESCRIPTOR   = 8,   // IDTR/GDTR/LDTR/TR modification
    KVMI_EVENT_CREATE_VCPU  = 9,   // New vCPU created
    KVMI_EVENT_PAUSE_VCPU   = 10,  // vCPU paused by introspector
    KVMI_EVENT_SINGLESTEP   = 11,  // Single-step completed
    KVMI_EVENT_CMD_ERROR    = 12,  // Command error notification
    KVMI_EVENT_CPUID        = 13,  // CPUID instruction intercepted
};
```

**Event Actions** (reply from introspector):
```c
enum {
    KVMI_EVENT_ACTION_CONTINUE = 0,  // Let guest continue normally
    KVMI_EVENT_ACTION_RETRY    = 1,  // Retry the instruction
    KVMI_EVENT_ACTION_CRASH    = 2,  // Crash the guest
};
```

### 2.5 Key Data Structures

**Message Header** (every message starts with this):
```c
struct kvmi_msg_hdr {
    __u16 id;    // Command/event ID
    __u16 size;  // Payload size
    __u32 seq;   // Sequence number for matching replies
};
```

**Event structure** (delivered with every event):
```c
struct kvmi_event {
    __u16 size;
    __u16 vcpu;
    __u8  event;       // KVMI_EVENT_* value
    __u8  padding[3];
    struct kvmi_event_arch arch;  // Full register state
};
```

**x86 event architecture state** (included in every event):
```c
struct kvmi_event_arch {
    __u8 mode;          // CPU mode: 2 (16-bit), 4 (32-bit), 8 (64-bit)
    __u8 padding1;
    __u16 view;         // Current EPT view
    __u32 padding2;
    struct kvm_regs regs;    // GP registers (RAX-R15, RIP, RFLAGS, RSP)
    struct kvm_sregs sregs;  // Segment registers, CR0-CR4, IDT, GDT
    struct {                 // Selected MSRs
        __u64 sysenter_cs, sysenter_esp, sysenter_eip;
        __u64 efer, star, lstar, cstar, pat, shadow_gs;
    } msrs;
};
```

**Page access control**:
```c
enum {
    KVMI_PAGE_ACCESS_R = 1 << 0,  // Read
    KVMI_PAGE_ACCESS_W = 1 << 1,  // Write
    KVMI_PAGE_ACCESS_X = 1 << 2,  // Execute
    KVMI_PAGE_SVE      = 1 << 3,  // Suppress Virtualization Exceptions
};

struct kvmi_page_access_entry {
    __u64 gpa;        // Guest physical address
    __u8  access;     // KVMI_PAGE_ACCESS_* flags
    ...
};
```

**EPT violation (page fault) event**:
```c
struct kvmi_event_pf {
    __u64 gva;     // Guest virtual address that faulted
    __u64 gpa;     // Guest physical address
    __u8  access;  // Which access triggered (R/W/X)
    ...
};
```

**CR write event**:
```c
struct kvmi_event_cr {
    __u16 cr;           // Which CR (0, 3, or 4)
    __u64 old_value;
    __u64 new_value;
};
struct kvmi_event_cr_reply {
    __u64 new_val;      // Introspector can modify the value
};
```

**Memory mapping ioctls** (for direct guest memory mapping):
```c
#define KVM_GUEST_MEM_START   _IOW('i', 0x01, void *)
#define KVM_GUEST_MEM_MAP     _IOWR('i', 0x02, struct kvmi_guest_mem_map)
#define KVM_GUEST_MEM_UNMAP   _IOW('i', 0x03, unsigned long)
```

**Hypercall codes** (for in-guest cooperation):
```c
#define KVMI_HC_START   0x01
#define KVMI_HC_MAP     0x02
#define KVMI_HC_UNMAP   0x03
#define KVMI_HC_END     0x04
```

### 2.6 Advanced Features

**EPT Views**: Multiple EPT (Extended Page Table) mappings for the same guest. Each view can have different R/W/X permissions. Used for:
- Stealthy hook pages (execute from one view, read from another)
- Memory shadowing for rootkit detection
- Code integrity enforcement

```c
struct kvmi_features {
    __u8 spp;         // Sub-Page Protection support
    __u8 vmfunc;      // VMFUNC support
    __u8 eptp;        // EPT pointer switching
    __u8 ve;          // Virtualization Exception (#VE)
    __u8 singlestep;  // Single-step support
};
```

**Sub-Page Protection (SPP)**: Intel SPP allows write-protect at 128-byte granularity within a 4KB page:
```c
struct kvmi_page_write_bitmap_entry {
    __u64 gpa;
    __u32 bitmap;   // 32 bits = 32 x 128-byte sub-pages
};
```

**Virtualization Exception (#VE)**: Instead of VM exits on EPT violations, the CPU delivers a #VE exception to the guest. The introspector can set this up for lower-overhead monitoring.

### 2.7 Why KVMi Hasn't Been Merged Upstream

Based on mailing list analysis, the key reasons include:

1. **Massive patch surface**: 77-84 patches across multiple subsystems. The kernel community prefers incremental merging, but KVMi is hard to split into independently useful pieces.

2. **Single consumer concern**: KVM maintainers questioned whether the API serves only Bitdefender's commercial product. Without multiple consumers, there's resistance to adding a large new kernel API.

3. **Maintenance burden**: The KVM maintainers (Paolo Bonzini et al.) expressed concern about the long-term maintenance cost of a complex introspection API that few developers would understand.

4. **Alternative approaches**: Some maintainers suggested simpler approaches (e.g., eBPF-based, or leveraging existing KVM infrastructure) rather than a dedicated VMI subsystem.

5. **Security surface**: Adding a socket-based API from QEMU to external processes expands the attack surface. Questions about privilege separation and sandboxing.

6. **Shift to HEKI**: After v12 stalled, Mickael Salaun (Landlock author) proposed "Hypervisor-Enforced Kernel Integrity" (HEKI) as a narrower, more upstreamable approach — focusing on CR pinning and kernel integrity rather than full-blown VMI.

7. **EPT views complexity**: The advanced features (multiple EPT views, SPP, #VE) are Intel-specific and add significant complexity.

### 2.8 Performance Characteristics

No formal benchmarks have been published in the patch series. Expected characteristics:

- **Event delivery latency**: Each intercepted event causes a VM exit + context switch to QEMU + socket IPC to introspector + reply. Expected latency: ~10-50 microseconds per event.
- **Memory read/write**: Via `KVMI_VM_READ_PHYSICAL` / `KVMI_VM_WRITE_PHYSICAL`, each operation involves socket round-trip. For bulk reads, the `KVM_GUEST_MEM_MAP` ioctl provides direct mapping (zero-copy).
- **EPT violation overhead**: Removing W permission on a page and catching writes adds ~2-5 microseconds per write to that page.
- **Impact on unmonitored workloads**: Minimal when no events are subscribed. The patches add checks in VM exit paths but these are branch-predicted and nearly free.
- **Comparison**: Xen's `vm_event` mechanism has similar latency characteristics but benefits from ring-buffer-based event delivery rather than socket IPC.

---

## 3. KVM VMI Tools and Projects

### 3.1 LibVMI on KVM

**Repo**: https://github.com/libvmi/libvmi

LibVMI provides a unified C API for VM introspection across Xen and KVM. Two KVM modes:

**Modern mode** (default, uses KVMi):
```bash
cmake .. -DENABLE_KVM=ON -DENABLE_XEN=OFF
```
Requires: patched kernel + patched QEMU + libkvmi installed. Connects via `libkvmi_wrapper.c`.

**Legacy mode** (patched QEMU socket or QMP):
```bash
cmake .. -DENABLE_KVM_LEGACY=ON
```
Two sub-modes:
- **Patch mode**: Custom QEMU patch adds a Unix domain socket. Uses `struct request` protocol (type=1 for read, type=2 for write). Fast.
- **Native mode** (fallback): Uses QMP `xp` command to read memory word-by-word. Extremely slow. No QEMU patches needed.

QEMU patches available for versions: 0.14.0, 1.2.0, 1.5.1, 1.6.0, 2.4.0.1, 2.8, 2.10, 2.12, 3.x, 4.0.0, 4.1.0.

### 3.2 DRAKVUF on KVM

**DRAKVUF does NOT support KVM.** It exclusively requires Xen with Intel VT-x + EPT. The build explicitly disables KVM:
```bash
./configure --disable-kvm --disable-bareflank --disable-file
```
There are no known plans to add KVM support. DRAKVUF depends deeply on Xen's altp2m (alternate p2m) API which has no KVM equivalent.

### 3.3 memflow Ecosystem

**memflow** (https://github.com/memflow/memflow) is a Rust-based physical memory introspection framework.

Connectors:
| Connector | Method | Write? | Events? | Kernel Patches? |
|-----------|--------|--------|---------|-----------------|
| memflow-kvm | Kernel module maps VM pages | Read-only | No | Module only |
| memflow-qemu | /proc/pid/mem of QEMU | Read-only | No | None |
| memflow-pcileech | DMA hardware | R/W | No | None |
| memflow-coredump | File analysis | Read-only | No | None |

Performance: Can walk an entire process virtual address space in under 1 second via batched operations and caching.

### 3.4 kvmi-rs (Rust Bindings)

**Repo**: https://github.com/kylerky/kvmi-rs
**Language**: Rust (91.7%), C (7.9%)
**Stars**: 3 | **Last updated**: January 2023

Provides Rust bindings over KVMi. Contains:
- `kvmi` — Core introspection bindings
- `kvmi-semantic` — Semantic analysis layer (OS-aware introspection)
- `observer` — Monitoring functionality

Minimally documented and maintained. Not widely adopted.

### 3.5 Nitro (Legacy)

Part of the kvm-vmi project. An early KVM introspection tool that intercepts system calls by monitoring SYSENTER/SYSCALL. Replaced by the KVMi approach.

### 3.6 pyvmidbg

**Repo**: https://github.com/Wenzel/pyvmidbg (archived November 2021)

Python GDB stub using LibVMI for hypervisor-level debugging. Supports KVM (via kvm-vmi/libvmi). Implements GDB Remote Serial Protocol. Limited to 1 vCPU.

### 3.7 r2vmi

**Repo**: https://github.com/Wenzel/r2vmi (archived November 2021)

Radare2 + LibVMI for hypervisor-level reverse engineering. **Xen only**, does not support KVM. Successor: pyvmidbg.

### 3.8 Production KVM VMI Deployments

**Bitdefender HVI (Hypervisor Introspection)**: The primary commercial consumer of KVMi. Bitdefender's server security product uses VMI for:
- Anti-rootkit detection
- Kernel integrity monitoring
- Exploit prevention
- Memory-based malware detection

Deployed in enterprise environments on KVM-based clouds. Requires their custom kernel + QEMU builds.

**No known open-source production KVM VMI deployments** exist. All projects remain in research/experimental stage. This contrasts with Xen where DRAKVUF and Bitdefender HVI have production deployments.

---

## 4. KVM VMI Limitations vs Xen VMI

| Capability | Xen | KVM (with KVMi patches) | KVM (without patches) |
|-----------|-----|------------------------|----------------------|
| **Upstream support** | Yes (vm_event in mainline) | No (patches not merged) | N/A |
| **Memory R/W** | Yes (via xenaccess/libvmi) | Yes (KVMI_VM_READ/WRITE_PHYSICAL) | Limited (QMP/proc/mem) |
| **Register access** | Yes (vm_event) | Yes (KVMI_VCPU_GET/SET_REGISTERS) | Limited (GDB stub) |
| **CR interception** | Yes (CR0/CR3/CR4) | Yes (KVMI_EVENT_CR) | No |
| **MSR interception** | Yes | Yes (KVMI_EVENT_MSR) | No |
| **EPT violation events** | Yes (mem_access) | Yes (KVMI_EVENT_PF) | No |
| **Breakpoints** | Yes (INT3) | Yes (KVMI_EVENT_BREAKPOINT) | GDB only |
| **Single-stepping** | Yes (MTF) | Yes (KVMI_EVENT_SINGLESTEP) | GDB only |
| **Descriptor table events** | Limited | Yes (KVMI_EVENT_DESCRIPTOR) | No |
| **CPUID interception** | No | Yes (KVMI_EVENT_CPUID) | No |
| **Alternate page tables** | Yes (altp2m, mainline) | Yes (EPT views, patches only) | No |
| **Sub-page protection** | No | Yes (SPP, Intel only) | No |
| **#VE support** | No | Yes (patches only) | No |
| **Event delivery** | Ring buffer (shared memory) | Unix socket IPC | N/A |
| **Event latency** | ~5-20 us | ~10-50 us | N/A |
| **DRAKVUF support** | Yes | No | No |
| **LibVMI support** | Yes (mature) | Yes (work-in-progress) | Legacy mode only |
| **Kernel patches needed** | No | Yes | No |
| **Maintenance status** | Active (Xen community) | Stalled (since 2021) | N/A |

### Key Limitations of KVM VMI

1. **No upstream support**: The single biggest limitation. Every deployment requires custom kernel + QEMU builds. Kernel upgrades require re-porting patches.

2. **Stalled development**: Bitdefender's last full patch series (v12) was October 2021. The kvm-vmi project's kernel is stuck on 5.4.24.

3. **IPC overhead**: Xen uses a shared-memory ring buffer for event delivery (zero-copy, ~5us). KVMi uses Unix domain socket IPC through QEMU (~10-50us). This matters for high-frequency events.

4. **QEMU dependency**: KVMi routes everything through QEMU. The introspector connects to QEMU's socket, not directly to the kernel. This adds latency and complexity. Xen's vm_event goes directly from hypervisor to introspection domain.

5. **No DRAKVUF**: DRAKVUF is the most mature open-source VMI analysis platform, and it only works on Xen. There is no KVM equivalent.

6. **Intel-only advanced features**: EPT views, SPP, and #VE are Intel-specific. AMD SEV complicates memory access. Xen's altp2m also requires Intel but is upstream.

7. **Security model**: KVM's security model (QEMU as the VMM) means the introspector must trust QEMU. In Xen's model, the introspector runs in a separate domain with direct hypervisor access.

### Where KVM VMI is Better

1. **CPUID interception**: KVMi supports it; Xen vm_event does not.
2. **Sub-page protection**: KVMi supports Intel SPP for 128-byte granularity write protection.
3. **Wider hardware support**: KVM runs on more platforms (ARM, RISC-V, s390) though VMI patches are x86-only.
4. **Deployment**: KVM is far more common than Xen in modern cloud/enterprise (libvirt, OpenStack, Proxmox). If VMI were upstream, it would have much wider reach.

---

## 5. Practical Setup: KVM VMI from Scratch

### Option A: Full KVMi Setup (kvm-vmi project)

#### Prerequisites
- Ubuntu 18.04/20.04 (tested)
- Intel CPU with VT-x and EPT
- 8GB+ RAM, 50GB+ disk

#### Step 1: Clone and prepare
```bash
git clone https://github.com/KVM-VMI/kvm-vmi.git --recursive
cd kvm-vmi
git checkout master
git submodule update
```

#### Step 2: Build patched kernel
```bash
sudo apt-get install bc fakeroot flex bison libelf-dev libssl-dev ncurses-dev

cd kvm-vmi/kvm
make olddefconfig
make menuconfig
# Set: CONFIG_KVM=m, CONFIG_KVM_INTEL=m, CONFIG_KSM=n,
#       CONFIG_REMOTE_MAPPING=y, CONFIG_KVM_INTROSPECTION=y

make -j$(nproc) bzImage
make -j$(nproc) modules
sudo make modules_install
sudo make install
sudo reboot
# Verify: uname -r should show 5.4.24-kvmi
```

#### Step 3: Build patched QEMU
```bash
sudo apt-get install libpixman-1-dev pkg-config zlib1g-dev \
  libglib2.0-dev dh-autoreconf libspice-server-dev

cd kvm-vmi/qemu
./configure --target-list=x86_64-softmmu --enable-spice --prefix=/usr/local
make -j$(nproc)
sudo make install
```

#### Step 4: Install libkvmi
```bash
git clone https://github.com/bitdefender/libkvmi --branch kvmi-v6
cd libkvmi
./bootstrap && ./configure && make && sudo make install
```

#### Step 5: Create a VM with introspection socket
Add to libvirt domain XML:
```xml
<domain type='kvm' xmlns:qemu='http://libvirt.org/schemas/domain/qemu/1.0'>
  <qemu:commandline>
    <qemu:arg value='-chardev'/>
    <qemu:arg value='socket,path=/tmp/introspector,id=chardev0,reconnect=10'/>
    <qemu:arg value='-object'/>
    <qemu:arg value='introspection,id=kvmi,chardev=chardev0'/>
  </qemu:commandline>
  <devices>
    <emulator>/usr/local/bin/qemu-system-x86_64</emulator>
  </devices>
</domain>
```

#### Step 6: Build LibVMI
```bash
sudo apt-get install build-essential libtool cmake pkg-config check \
  libglib2.0-dev libvirt-dev flex bison libjson-c-dev

cd kvm-vmi/libvmi
mkdir build && cd build
cmake .. -DCMAKE_INSTALL_PREFIX=/usr/local -DENABLE_KVM=ON \
  -DENABLE_XEN=OFF -DENABLE_BAREFLANK=OFF
make -j$(nproc)
sudo make install
```

#### Step 7: Generate OS profile (for semantic analysis)
```bash
# Start the VM, then:
./examples/vmi-win-guid   # Note kernel filename + PDB GUID

# Generate Volatility3 profile:
git clone https://github.com/volatilityfoundation/volatility3
cd volatility3 && pip install -e .
python volatility/framework/symbols/windows/pdbconv.py \
  -o /etc/libvmi/profile.json -p <kernel_file> -g <pdb_guid>
```

#### Step 8: Test
```bash
# Terminal 1: Start introspection
cd libkvmi/examples
./hookguest-libkvmi /tmp/introspector

# Terminal 2: Start VM
virsh start <domain>

# Wait ~10 seconds for connection. Test LibVMI:
LIBVMI_DEBUG=1 ./build/examples/vmi-process-list -n <vm-name> \
  -j /etc/libvmi/profile.json
```

### Option B: memflow-kvm (No Kernel Patches)

For read-only memory introspection without kernel patches:

```bash
# Install memflow
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
cargo install memflowup

# Or manually:
git clone https://github.com/memflow/memflow-kvm
cd memflow-kvm
cargo build --release --all-features

# Install kernel module
cd memflow-kmod
sudo dkms install .
sudo modprobe memflow

# Setup permissions
sudo groupadd memflow
sudo usermod -aG memflow $USER
# Add udev rules for /dev/memflow

# Use with memflow tools
cp target/release/libmemflow_kvm.so ~/.local/lib/memflow/
```

### Option C: /proc/pid/mem (Zero Setup)

For quick-and-dirty memory reads:

```bash
# Find QEMU process
QEMU_PID=$(pgrep -f "qemu-system.*your-vm")

# Find guest RAM mapping (look for large anonymous mapping)
grep -E "^[0-9a-f]+-[0-9a-f]+ rw-p.*\(deleted\)" /proc/$QEMU_PID/maps

# Read guest physical memory at offset
sudo python3 -c "
import os
pid = $QEMU_PID
# Guest RAM base address from maps output
base = 0x7f0000000000  # ADJUST THIS
gpa = 0x1000  # Guest physical address to read
fd = os.open(f'/proc/{pid}/mem', os.O_RDONLY)
os.lseek(fd, base + gpa, os.SEEK_SET)
data = os.read(fd, 4096)
os.close(fd)
print(data[:64].hex())
"
```

### Option D: QEMU GDB Stub (Zero Setup)

```bash
# Launch VM with GDB server
qemu-system-x86_64 ... -gdb tcp::1234

# Connect
gdb -ex "set architecture i386:x86-64" \
    -ex "target remote :1234"

# In GDB:
# (gdb) info registers
# (gdb) x/10x 0xfffff80000000000   # Read virtual memory
# (gdb) monitor xp /10x 0x1000     # Read physical memory
```

---

## Appendix: Key URLs and References

| Resource | URL |
|----------|-----|
| kvm-vmi project | https://github.com/KVM-VMI/kvm-vmi |
| kvm-vmi setup guide | https://kvm-vmi.github.io/kvm-vmi/kvmi-v7/setup.html |
| libkvmi | https://github.com/bitdefender/libkvmi |
| LibVMI | https://github.com/libvmi/libvmi |
| memflow-kvm | https://github.com/memflow/memflow-kvm |
| memflow | https://github.com/memflow/memflow |
| kvmi-rs | https://github.com/kylerky/kvmi-rs |
| KVMi v12 patches | https://patchwork.kernel.org/project/kvm/list/?q=KVMI |
| KVMi kernel header | KVM-VMI/kvm @ kvmi-v7: include/uapi/linux/kvmi.h |
| KVMi x86 header | KVM-VMI/kvm @ kvmi-v7: arch/x86/include/uapi/asm/kvmi.h |
| KVM ioctl API docs | https://www.kernel.org/doc/html/latest/virt/kvm/api.html |
| DRAKVUF (Xen only) | https://drakvuf.com/ |
| KVM Forum 2019 VMI talk | Mihai Dontu, "Advanced VMI on KVM: A Progress Report" |
| awesome-virtualization | https://github.com/Wenzel/awesome-virtualization |
