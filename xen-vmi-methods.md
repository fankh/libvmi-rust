# Xen-Based Virtual Machine Introspection: Deep Technical Reference

## Table of Contents

1. [Xen VMI Architecture](#1-xen-vmi-architecture)
2. [Xen Event Types (Comprehensive)](#2-xen-event-types-comprehensive)
3. [Xen VMI Tools](#3-xen-vmi-tools)
4. [Xen Versions and VMI Features](#4-xen-versions-and-vmi-features)
5. [Practical Setup](#5-practical-setup)
6. [Xen VMI Advantages Over KVM](#6-xen-vmi-advantages-over-kvm)

---

## 1. Xen VMI Architecture

### 1.1 vm_event Subsystem

The vm_event subsystem is Xen's core mechanism for delivering hardware virtualization events from the hypervisor to a monitoring application running in a privileged domain (typically dom0 or a dedicated stub domain).

#### Evolution

- **Xen 4.4**: Initial "mem_event" subsystem (memory-focused only)
- **Xen 4.6**: Renamed to "vm_event" -- extended beyond memory to support all hardware events (interrupts, registers, descriptors, CPUID, etc.)
- **Current**: `VM_EVENT_INTERFACE_VERSION 0x00000007`

#### Architecture

```text
+------------------+       Shared Ring Buffer        +------------------+
|   Xen Hypervisor |  =============================> |  Monitor App     |
|   (vm_event      |  <============================= |  (dom0/stubdom)  |
|    producer)     |    Xen Event Channel (async)     |  (consumer)      |
+------------------+                                  +------------------+
        |                                                     |
  EPT violations                                        xenctrl API
  INT3 traps                                           (xc_monitor_*)
  CR writes                                            (xc_vm_event_*)
  MSR access                                           (xc_altp2m_*)
  CPUID intercept
  Descriptor access
```

#### Ring Buffer Mechanism

The vm_event system uses a **shared memory ring** (Xen ring protocol) for communication:

1. **Setup**: `vm_event_enable()` maps a guest-provided GFN as the ring buffer, initializes `FRONT_RING_INIT()`, and allocates an unbound Xen event channel via `alloc_unbound_xen_event_channel()`.

2. **Request Path (Hypervisor -> Monitor)**:
   - Caller reserves a ring slot via `__vm_event_claim_slot()`
   - Event copied into ring via `RING_GET_REQUEST()` + `memcpy()`
   - Producer index advanced, `RING_PUSH_REQUESTS()` makes entry visible
   - `notify_via_xen_event_channel()` signals the monitor
   - **Backpressure**: If ring utilization exceeds thresholds, the requesting vCPU is paused via `vm_event_mark_and_pause()`

3. **Response Path (Monitor -> Hypervisor)**:
   - `vm_event_get_response()` extracts response, updates consumer index
   - `vm_event_resume()` validates response version/vCPU ID, dispatches to handlers:
     - `p2m_mem_paging_resume()` for paging
     - `mem_sharing_fork_reset()` for fork reset
     - `vm_event_emulate_check()` for emulation control
     - `vm_event_register_write_resume()` for register writes
     - `vm_event_toggle_singlestep()` for single-step
     - `p2m_altp2m_check()` for alternate p2m switching
     - `vm_event_vcpu_unpause()` for vCPU resume

4. **Producer Accounting**: Tracks `target_producers` (monitored domain vCPUs) and `foreign_producers` (external domains). `vm_event_ring_available()` calculates free slots.

5. **Fairness**: Blocked vCPUs (ring full) tracked in `ved->blocked`, awakened round-robin. Queued vCPUs (failed slot claim) wait on `ved->wq` with priority.

#### Three Ring Types

| Ring Type | domctl Mode | Purpose |
|-----------|------------|---------|
| Paging | `XEN_DOMCTL_VM_EVENT_OP_PAGING (1)` | Memory paging/swapping events |
| Monitor | `XEN_DOMCTL_VM_EVENT_OP_MONITOR (2)` | Hardware event monitoring (primary VMI ring) |
| Sharing | `XEN_DOMCTL_VM_EVENT_OP_SHARING (3)` | Memory sharing/deduplication events |

#### Key domctl Operations

**`XEN_DOMCTL_vm_event_op` (cmd 56)**:
```c
struct xen_domctl_vm_event_op {
    uint32_t op;      /* XEN_VM_EVENT_ENABLE/DISABLE/RESUME/GET_VERSION */
    uint32_t mode;    /* PAGING(1) / MONITOR(2) / SHARING(3) */
    union {
        struct { uint32_t port; } enable;
        uint32_t version;
    } u;
};
```

**`XEN_DOMCTL_monitor_op` (cmd 77)**:
```c
struct xen_domctl_monitor_op {
    uint32_t op;    /* ENABLE(0) / DISABLE(1) / GET_CAPABILITIES(2) /
                       EMULATE_EACH_REP(3) / CONTROL_REGISTERS(4) */
    uint32_t event; /* XEN_DOMCTL_MONITOR_EVENT_* */
    union {
        struct { uint8_t index, sync, onchangeonly; uint64_t bitmask; } mov_to_cr;
        struct { uint32_t msr; uint8_t onchangeonly; } mov_to_msr;
        struct { uint8_t sync, allow_userspace; } guest_request;
        struct { uint8_t sync; } debug_exception;
        struct { uint8_t sync; } vmexit;
    } u;
};
```

#### xenctrl Monitor API (Complete)

```c
// Ring lifecycle
void *xc_monitor_enable(xc_interface *xch, uint32_t domain_id, uint32_t *port);
int   xc_monitor_disable(xc_interface *xch, uint32_t domain_id);
int   xc_monitor_resume(xc_interface *xch, uint32_t domain_id);
int   xc_monitor_get_capabilities(xc_interface *xch, uint32_t domain_id,
                                  uint32_t *capabilities);

// Control register monitoring
int xc_monitor_write_ctrlreg(xc_interface *xch, uint32_t domain_id,
                             uint16_t index, bool enable, bool sync,
                             uint64_t bitmask, bool onchangeonly);

// MSR monitoring
int xc_monitor_mov_to_msr(xc_interface *xch, uint32_t domain_id,
                          uint32_t msr, bool enable, bool onchangeonly);

// Execution control
int xc_monitor_singlestep(xc_interface *xch, uint32_t domain_id, bool enable);
int xc_monitor_software_breakpoint(xc_interface *xch, uint32_t domain_id, bool enable);

// Descriptor tables
int xc_monitor_descriptor_access(xc_interface *xch, uint32_t domain_id, bool enable);

// Guest request (vmcall/vmmcall)
int xc_monitor_guest_request(xc_interface *xch, uint32_t domain_id,
                             bool enable, bool sync, bool allow_userspace);

// Page faults
int xc_monitor_inguest_pagefault(xc_interface *xch, uint32_t domain_id, bool disable);

// Debug
int xc_monitor_debug_exceptions(xc_interface *xch, uint32_t domain_id,
                                bool enable, bool sync);

// CPUID interception
int xc_monitor_cpuid(xc_interface *xch, uint32_t domain_id, bool enable);

// Privileged calls
int xc_monitor_privileged_call(xc_interface *xch, uint32_t domain_id, bool enable);

// Emulation failures
int xc_monitor_emul_unimplemented(xc_interface *xch, uint32_t domain_id, bool enable);

// VM exits (raw VMEXIT interception)
int xc_monitor_vmexit(xc_interface *xch, uint32_t domain_id, bool enable, bool sync);

// I/O port monitoring
int xc_monitor_io(xc_interface *xch, uint32_t domain_id, bool enable);

// REP instruction emulation
int xc_monitor_emulate_each_rep(xc_interface *xch, uint32_t domain_id, bool enable);

// Version query
int xc_vm_event_get_version(xc_interface *xch);
```

---

### 1.2 altp2m (Alternate p2m) Subsystem

#### What It Is

altp2m (alternate physical-to-machine mapping) allows creation of **multiple EPT views** for a single guest domain. Each view can have different:
- Memory access permissions (read/write/execute) per page
- GFN-to-MFN remappings (page substitution)
- Suppress-VE (Virtualization Exception) settings

This is the key technology enabling **stealthy, invisible breakpoints** for VMI.

#### How Stealthy Breakpoints Work

Traditional INT3 breakpoints modify guest memory (replacing instruction bytes with `0xCC`), which is detectable by the guest. altp2m solves this:

```text
View 0 (Default/Clean):           View 1 (Instrumented):
+------------------+              +------------------+
| GFN 0x1000       |              | GFN 0x1000       |
| Original code    |              | INT3 (0xCC) +    |
| (executable, no  |              | remaining code   |
|  read allowed)   |              | (no-execute)     |
+------------------+              +------------------+

Guest executes in View 0:
  - Reads memory -> sees original code (clean)
  - Executes code -> EPT violation (X denied) -> switch to View 1

View 1:
  - Execute hits INT3 -> vm_event -> monitor handles breakpoint
  - Single-step past INT3 -> switch back to View 0
```

**Step-by-step stealthy breakpoint flow**:
1. Create View 0 (clean): original code pages, mark as **execute-only** (no read)
2. Create View 1 (instrumented): copy of code pages with INT3 patches, mark as **read-write only** (no execute)
3. Guest runs in View 0 normally
4. When guest tries to read the code page (e.g., integrity check), View 0 returns original unmodified bytes
5. When guest executes the code page, EPT execute violation triggers -> hypervisor switches to View 1
6. View 1 has INT3 at target address -> trap fires -> vm_event delivered to monitor
7. Monitor processes event, single-steps past INT3, switches back to View 0

This makes breakpoints **completely invisible** to the guest -- no memory modification is detectable.

#### altp2m Hypercall API

**`HVMOP_altp2m` (hypercall 25)**, `HVMOP_ALTP2M_INTERFACE_VERSION 0x00000001`

| Subcommand | ID | Purpose |
|-----------|-----|---------|
| `HVMOP_altp2m_get_domain_state` | 1 | Query altp2m enable state |
| `HVMOP_altp2m_set_domain_state` | 2 | Enable/disable altp2m for domain |
| `HVMOP_altp2m_vcpu_enable_notify` | 3 | Enable #VE notifications for vCPU |
| `HVMOP_altp2m_create_p2m` | 4 | Create new EPT view |
| `HVMOP_altp2m_destroy_p2m` | 5 | Destroy EPT view |
| `HVMOP_altp2m_switch_p2m` | 6 | Switch entire domain to a view |
| `HVMOP_altp2m_set_mem_access` | 7 | Set page access in a view |
| `HVMOP_altp2m_change_gfn` | 8 | Remap GFN->MFN in a view |
| `HVMOP_altp2m_set_mem_access_multi` | 9 | Bulk access configuration |
| `HVMOP_altp2m_set_suppress_ve` | 10 | Set VE suppression on page |
| `HVMOP_altp2m_get_suppress_ve` | 11 | Get VE suppression status |
| `HVMOP_altp2m_get_mem_access` | 12 | Query page access in view |
| `HVMOP_altp2m_vcpu_disable_notify` | 13 | Disable #VE for vCPU |
| `HVMOP_altp2m_get_p2m_idx` | 14 | Get active vCPU p2m index |
| `HVMOP_altp2m_set_suppress_ve_multi` | 15 | Bulk VE suppression |
| `HVMOP_altp2m_set_visibility` | 16 | Control view visibility |

#### xenctrl altp2m API (Complete)

```c
// Domain state
int xc_altp2m_get_domain_state(xc_interface *h, uint32_t dom, bool *state);
int xc_altp2m_set_domain_state(xc_interface *h, uint32_t dom, bool state);

// VE notifications
int xc_altp2m_set_vcpu_enable_notify(xc_interface *h, uint32_t domid,
                                     uint32_t vcpuid, xen_pfn_t gfn);
int xc_altp2m_set_vcpu_disable_notify(xc_interface *h, uint32_t domid,
                                      uint32_t vcpuid);

// View lifecycle
int xc_altp2m_create_view(xc_interface *h, uint32_t domid,
                          xenmem_access_t default_access, uint16_t *view_id);
int xc_altp2m_destroy_view(xc_interface *h, uint32_t domid, uint16_t view_id);
int xc_altp2m_switch_to_view(xc_interface *h, uint32_t domid, uint16_t view_id);

// Memory access per view
int xc_altp2m_set_mem_access(xc_interface *h, uint32_t domid, uint16_t view_id,
                             xen_pfn_t gfn, xenmem_access_t access);
int xc_altp2m_set_mem_access_multi(xc_interface *h, uint32_t domid,
                                   uint16_t view_id, uint8_t *access,
                                   uint64_t *gfns, uint32_t nr);
int xc_altp2m_get_mem_access(xc_interface *h, uint32_t domid, uint16_t view_id,
                             xen_pfn_t gfn, xenmem_access_t *access);

// GFN remapping
int xc_altp2m_change_gfn(xc_interface *h, uint32_t domid, uint16_t view_id,
                         xen_pfn_t old_gfn, xen_pfn_t new_gfn);

// VE suppression
int xc_altp2m_set_suppress_ve(xc_interface *h, uint32_t domid,
                              uint16_t view_id, xen_pfn_t gfn, bool sve);
int xc_altp2m_get_suppress_ve(xc_interface *h, uint32_t domid,
                              uint16_t view_id, xen_pfn_t gfn, bool *sve);
int xc_altp2m_set_supress_ve_multi(xc_interface *h, uint32_t domid,
                                   uint16_t view_id, xen_pfn_t first_gfn,
                                   xen_pfn_t last_gfn, bool sve,
                                   xen_pfn_t *error_gfn, int32_t *error_code);

// View query and visibility
int xc_altp2m_get_vcpu_p2m_idx(xc_interface *h, uint32_t domid,
                               uint32_t vcpuid, uint16_t *p2midx);
int xc_altp2m_set_visibility(xc_interface *h, uint32_t domid,
                             uint16_t view_id, bool visible);
```

#### Key Data Structures

```c
struct xen_hvm_altp2m_view {
    uint16_t view;
    uint16_t hvmmem_default_access;  /* xenmem_access_t */
};

struct xen_hvm_altp2m_mem_access {
    uint16_t view;
    uint16_t access;   /* xenmem_access_t */
    uint64_t gfn;
};

struct xen_hvm_altp2m_change_gfn {
    uint16_t view;
    uint64_t old_gfn;
    uint64_t new_gfn;  /* ~0UL to revert */
};

struct xen_hvm_altp2m_suppress_ve {
    uint16_t view;
    uint8_t  suppress_ve;
    uint64_t gfn;
};
```

#### Internal Implementation

- **View creation**: `p2m_init_next_altp2m()` allocates next available slot, calls `p2m_activate_altp2m()` which copies logdirty info from host p2m and initializes EPT via `p2m_init_altp2m_ept()`
- **View destruction**: `p2m_destroy_altp2m_by_id()` requires pausing all vCPUs, checks no vCPU uses the view (`active_vcpus` counter), calls `p2m_reset_altp2m()`, sets `altp2m_eptp[idx] = INVALID_MFN`
- **View switching**: `p2m_switch_vcpu_altp2m_by_id()` updates individual vCPU's p2m reference; `p2m_switch_domain_altp2m_by_id()` pauses all vCPUs and switches entire domain
- **GFN remapping**: `p2m_change_altp2m_gfn()` gets effective entry via `altp2m_get_effective_entry()` (checks altp2m, falls back to host p2m), creates new PTE for old_gfn pointing to MFN from new_gfn
- **Change propagation**: `p2m_altp2m_propagate_change()` syncs host p2m changes to alt views; if page removed, resets affected views

#### Configurable Number of Views (Xen 4.21+)

Xen 4.21 introduced `vm.cfg` support for configuring the number of altp2m tables per domain, allowing operators to tune memory vs. view capacity tradeoffs.

---

### 1.3 EPT (Extended Page Tables) Role in VMI

EPT (Intel) / NPT-RVI (AMD) provides **hardware-assisted second-level address translation** (Guest Physical Address -> Host Physical Address). For VMI, EPT serves three critical roles:

#### a) Memory Access Control

Each EPT entry contains **read/write/execute permission bits**. By clearing specific bits, the hypervisor can trap:
- **Read violations**: Guest attempts to read a protected page
- **Write violations**: Guest attempts to write to a protected page
- **Execute violations**: Guest attempts to execute code on a protected page

These violations generate **EPT violation VMEXITs**, which Xen converts to `VM_EVENT_REASON_MEM_ACCESS` events.

#### b) Multiple Memory Views (altp2m)

EPT supports `EPTP switching` (Intel VMFunc instruction, `VMFUNC_EPTP_SWITCHING`):
- Hardware feature on Intel Haswell+ and Atom Silvermont+
- Allows guest to switch between EPT views without VMEXIT
- Xen uses this for fast altp2m view switching

#### c) Virtualization Exceptions (#VE)

Intel Broadwell+ and Atom Goldmont+ support **Virtualization Exceptions** (#VE, vector 20):
- Instead of costly VMEXIT on EPT violation, CPU raises #VE in guest
- Guest handler can resolve the violation without hypervisor involvement
- `suppress_ve` bit per EPT entry controls whether #VE or VMEXIT occurs
- Used for performance optimization in VMI (handle known-safe violations in-guest)

#### EPT Entry Format (relevant bits)

```text
Bit 0:   Read access
Bit 1:   Write access
Bit 2:   Execute access (supervisor)
Bit 10:  Execute access (user-mode, if mode-based execute control enabled)
Bit 63:  Suppress VE (if EPT-violation #VE enabled)
```

#### Memory Access Types (xenmem_access_t)

```c
XENMEM_access_n      // No access (trap all R/W/X)
XENMEM_access_r      // Read-only
XENMEM_access_w      // Write-only
XENMEM_access_rw     // Read-write
XENMEM_access_x      // Execute-only
XENMEM_access_rx     // Read-execute
XENMEM_access_wx     // Write-execute
XENMEM_access_rwx    // Full access (default)
XENMEM_access_rx2rw  // Auto-change to r-w on write (trap once)
XENMEM_access_n2rwx  // Log access: starts as n, auto-goes to rwx, generates event
```

#### Memory Access API

```c
// Set access for page range
int xc_set_mem_access(xc_interface *xch, uint32_t domain_id,
                      xenmem_access_t access, uint64_t first_pfn, uint32_t nr);

// Batch set access for multiple pages
int xc_set_mem_access_multi(xc_interface *xch, uint32_t domain_id,
                            uint8_t *access, uint64_t *pages, uint32_t nr);

// Query access for a page
int xc_get_mem_access(xc_interface *xch, uint32_t domain_id,
                      uint64_t pfn, xenmem_access_t *access);
```

---

### 1.4 Xen Grant Tables and Memory Sharing

#### Grant Tables

Grant tables provide a **capability-based memory sharing mechanism** between domains:

- Each domain maintains a grant table (shared with Xen)
- **Grant references** are integer indices into the table, acting as capabilities
- Domains share pages without knowing real machine addresses
- Enables shared-memory communication between unprivileged domains

**Key Operations (`GNTTABOP_*`)**:

| Operation | Purpose |
|-----------|---------|
| `GNTTABOP_map_grant_ref` | Map a granted frame into a domain |
| `GNTTABOP_unmap_grant_ref` | Revoke mapping |
| `GNTTABOP_setup_table` | Initialize grant table pages |
| `GNTTABOP_transfer` | Transfer page ownership |
| `GNTTABOP_copy` | Hypervisor-mediated cross-domain copy |
| `GNTTABOP_query_size` | Get table dimensions |

**Grant Entry (v1)**:
```c
struct grant_entry_v1 {
    uint16_t flags;    /* GTF_readonly, GTF_reading, GTF_writing */
    domid_t  domid;    /* Target domain */
    uint32_t frame;    /* Page frame number */
};
```

**Grant Entry (v2)** adds sub-page grants (offset+length) and transitive grants (domain chains).

#### Memory Sharing for VMI

**`XENMEM_sharing_op` (opcode 22)** enables memory deduplication and domain forking:

| Operation | Purpose |
|-----------|---------|
| `XENMEM_sharing_op_nominate_gfn` | Nominate a guest frame for sharing |
| `XENMEM_sharing_op_share` | Establish sharing between pages |
| `XENMEM_sharing_op_fork` | Create memory-sharing based domain forks |
| `XENMEM_sharing_op_debug_gfn/mfn/gref` | Debug shared pages |

**Domain Forking**: Creates a copy-on-write clone of a running VM -- critical for malware analysis (DRAKVUF Sandbox uses this to fork VMs for each sample).

#### Memory Access Control

**`XENMEM_access_op` (opcode 21)** provides fine-grained page protection:

| Access Type | Behavior |
|------------|----------|
| `XENMEM_access_n` | No access (trap everything) |
| `XENMEM_access_r_pw` | Read + CPU page-table walk writes (A/D bits) |
| `XENMEM_access_rx2rw` | Auto-change to r-w on write |
| `XENMEM_access_n2rwx` | Log mode: starts as n, auto-goes to rwx + event |

---

### 1.5 xenctrl / xc_interface API for Memory Access

The `xenctrl` library (`libxenctrl`) is the primary C API for interacting with the Xen hypervisor from dom0/toolstack.

#### Opening a Connection

```c
xc_interface *xch = xc_interface_open(NULL, NULL, 0);
// ... use xch for all operations ...
xc_interface_close(xch);
```

#### Memory Mapping (Foreign Access)

```c
// Map a single page from a foreign domain
void *xc_map_foreign_range(xc_interface *xch, uint32_t dom,
                           int size, int prot, unsigned long mfn);

// Map multiple pages
void *xc_map_foreign_pages(xc_interface *xch, uint32_t dom,
                           int prot, const xen_pfn_t *arr, int num);

// Translate guest virtual address to physical
unsigned long xc_translate_foreign_address(xc_interface *xch, uint32_t dom,
                                          int vcpu, unsigned long long virt);
```

#### Domain Control

```c
int xc_domain_pause(xc_interface *xch, uint32_t domid);
int xc_domain_unpause(xc_interface *xch, uint32_t domid);
int xc_vcpu_getcontext(xc_interface *xch, uint32_t domid, uint32_t vcpu,
                       vcpu_guest_context_t *ctxt);
int xc_vcpu_setcontext(xc_interface *xch, uint32_t domid, uint32_t vcpu,
                       vcpu_guest_context_t *ctxt);
```

---

### 1.6 xen-access Example Tool

`xen-access` is Xen's built-in VMI example (in `tools/tests/xen-access/`). It demonstrates:

1. **Enabling the monitor ring**: `xc_monitor_enable()` -> returns event channel port
2. **Setting up event channel**: binds the port for notification
3. **Configuring events**: calls `xc_monitor_*()` to enable desired events
4. **Event loop**: polls the ring buffer, processes events, sends responses
5. **Handling responses**: sets flags like `VM_EVENT_FLAG_VCPU_PAUSED`, optionally emulates or denies

Events it can monitor:
- Memory access violations (via `xc_set_mem_access()`)
- CR0/CR3/CR4 writes (via `xc_monitor_write_ctrlreg()`)
- MSR writes (via `xc_monitor_mov_to_msr()`)
- INT3 software breakpoints (via `xc_monitor_software_breakpoint()`)
- CPUID instructions (via `xc_monitor_cpuid()`)
- Descriptor table access (via `xc_monitor_descriptor_access()`)
- Single-stepping (via `xc_monitor_singlestep()`)
- Guest requests (via `xc_monitor_guest_request()`)

---

## 2. Xen Event Types (Comprehensive)

### 2.1 Complete VM_EVENT_REASON List

| Reason | Value | Description |
|--------|-------|-------------|
| `VM_EVENT_REASON_UNKNOWN` | 0 | Unknown/unclassified event |
| `VM_EVENT_REASON_MEM_ACCESS` | 1 | EPT/NPT violation (memory R/W/X access) |
| `VM_EVENT_REASON_MEM_SHARING` | 2 | Memory sharing event |
| `VM_EVENT_REASON_MEM_PAGING` | 3 | Memory paging event |
| `VM_EVENT_REASON_WRITE_CTRLREG` | 4 | Write to control register (CR0/CR3/CR4/XCR0) |
| `VM_EVENT_REASON_MOV_TO_MSR` | 5 | Write to Model-Specific Register |
| `VM_EVENT_REASON_SOFTWARE_BREAKPOINT` | 6 | INT3 instruction executed |
| `VM_EVENT_REASON_SINGLESTEP` | 7 | Single-step completed |
| `VM_EVENT_REASON_GUEST_REQUEST` | 8 | Guest issued VMCALL/VMMCALL |
| `VM_EVENT_REASON_DEBUG_EXCEPTION` | 9 | Debug exception (DR registers, #DB) |
| `VM_EVENT_REASON_CPUID` | 10 | CPUID instruction intercepted |
| `VM_EVENT_REASON_PRIVILEGED_CALL` | 11 | Privileged instruction (ARM: SMC/HVC) |
| `VM_EVENT_REASON_INTERRUPT` | 12 | Interrupt injection |
| `VM_EVENT_REASON_DESCRIPTOR_ACCESS` | 13 | GDT/LDT/IDT/TR access |
| `VM_EVENT_REASON_EMUL_UNIMPLEMENTED` | 14 | Emulation failure |
| `VM_EVENT_REASON_VMEXIT` | 15 | Raw VMEXIT (VMX reason + qualification) |
| `VM_EVENT_REASON_IO_INSTRUCTION` | 16 | I/O port access (IN/OUT) |

### 2.2 Memory Access Events (VM_EVENT_REASON_MEM_ACCESS)

Triggered by EPT/NPT permission violations.

**Event Data Structure**:
```c
struct vm_event_mem_access {
    uint64_t gfn;       /* Guest frame number that was accessed */
    uint64_t offset;    /* Offset within the page */
    uint64_t gla;       /* Guest linear address (if valid) */
    uint32_t flags;     /* MEM_ACCESS_R/W/X + validity flags */
    uint32_t _pad;
};
```

**Access Flags**:
```c
MEM_ACCESS_R              (1 << 0)  /* Read access */
MEM_ACCESS_W              (1 << 1)  /* Write access */
MEM_ACCESS_X              (1 << 2)  /* Execute access */
MEM_ACCESS_RWX            (R|W|X)   /* All access */
MEM_ACCESS_RW             (R|W)
MEM_ACCESS_RX             (R|X)
MEM_ACCESS_WX             (W|X)
MEM_ACCESS_GLA_VALID      (1 << 3)  /* Guest linear address is valid */
MEM_ACCESS_FAULT_WITH_GLA (1 << 4)  /* Fault includes GLA */
MEM_ACCESS_FAULT_IN_GPT   (1 << 5)  /* Fault during guest page table walk */
```

### 2.3 Register Events (VM_EVENT_REASON_WRITE_CTRLREG)

Triggered when guest writes to control registers.

**Event Data Structure**:
```c
struct vm_event_write_ctrlreg {
    uint32_t index;     /* VM_EVENT_X86_CR0/CR3/CR4/XCR0 */
    uint32_t _pad;
    uint64_t new_value; /* Value being written */
    uint64_t old_value; /* Previous value */
};
```

**Control Register Indices**:
```c
VM_EVENT_X86_CR0   (0)  /* Protected mode, paging enable */
VM_EVENT_X86_CR3   (1)  /* Page directory base (process context switch) */
VM_EVENT_X86_CR4   (2)  /* Extensions enable (PAE, SMEP, etc.) */
VM_EVENT_X86_XCR0  (3)  /* Extended control register */
```

**CR3 monitoring** is the most important for VMI -- every process context switch writes CR3, allowing the monitor to track all process creation/termination.

### 2.4 MSR Events (VM_EVENT_REASON_MOV_TO_MSR)

```c
struct vm_event_mov_to_msr {
    uint64_t msr;       /* MSR index */
    uint64_t new_value; /* Value being written */
    uint64_t old_value; /* Previous value */
};
```

**MSR Bitmap Ranges** (for selective monitoring):
- **Low range**: 0x0000 - 0x1FFF (general-purpose MSRs)
- **Hypervisor range**: 0x40000000 - 0x40001FFF
- **High range**: 0xC0000000 - 0xC0001FFF (AMD extended)

Key MSRs for security monitoring:
- `MSR_LSTAR` (0xC0000082): Syscall entry point -- rootkits modify this
- `MSR_EFER` (0xC0000080): Extended Feature Enable Register
- `MSR_STAR` (0xC0000081): Legacy syscall target
- `MSR_IA32_SYSENTER_EIP` (0x176): 32-bit syscall entry

### 2.5 Interrupt Events (VM_EVENT_REASON_SOFTWARE_BREAKPOINT / INTERRUPT)

**Software Breakpoint (INT3)**:
```c
/* VM_EVENT_REASON_SOFTWARE_BREAKPOINT (6) */
/* No additional data -- uses register state from vm_event_regs_x86 */
/* The RIP in the register state points to the INT3 instruction */
```

**Debug Exception (#DB)**:
```c
struct vm_event_debug {
    uint64_t gfn;              /* Guest frame number */
    uint64_t pending_dbg;      /* Pending debug info */
    uint32_t insn_length;      /* Instruction length */
    uint32_t type;             /* Trap type */
};
```

**Interrupt Injection**:
```c
struct vm_event_interrupt_x86 {
    uint32_t vector;       /* Interrupt vector (0-255) */
    uint32_t type;         /* Interrupt type */
    uint32_t error_code;   /* Error code (if applicable) */
    uint64_t cr2;          /* CR2 value (for page faults) */
};
```

### 2.6 CPUID Interception (VM_EVENT_REASON_CPUID)

```c
struct vm_event_cpuid {
    uint32_t insn_length;  /* Instruction length */
    uint32_t leaf;         /* CPUID leaf (EAX input) */
    uint32_t subleaf;      /* CPUID subleaf (ECX input) */
};
```

Useful for detecting VM-aware malware that checks CPUID for hypervisor presence.

### 2.7 Descriptor Table Access (VM_EVENT_REASON_DESCRIPTOR_ACCESS)

```c
struct vm_event_desc_access {
    union {
        struct {
            uint32_t instr_info;         /* VMX instruction info */
            uint32_t _pad;
            uint64_t exit_qualification; /* VMX exit qualification */
        } vmx;
    } arch;
    uint8_t descriptor;   /* VM_EVENT_DESC_IDTR/GDTR/LDTR/TR */
    uint8_t is_write;     /* 1 = write, 0 = read */
};
```

**Descriptor Types**:
```c
VM_EVENT_DESC_IDTR  (1)  /* Interrupt Descriptor Table Register */
VM_EVENT_DESC_GDTR  (2)  /* Global Descriptor Table Register */
VM_EVENT_DESC_LDTR  (3)  /* Local Descriptor Table Register */
VM_EVENT_DESC_TR    (4)  /* Task Register */
```

Monitoring IDTR/GDTR writes detects rootkits that modify interrupt/syscall dispatch tables.

### 2.8 Guest Request (VM_EVENT_REASON_GUEST_REQUEST)

Triggered when the guest deliberately executes `VMCALL` (Intel) or `VMMCALL` (AMD). Allows cooperative VMI where an in-guest agent signals the monitor.

### 2.9 VMEXIT Events (VM_EVENT_REASON_VMEXIT)

```c
struct vm_event_vmexit {
    struct {
        uint64_t reason;          /* VMX exit reason */
        uint64_t qualification;   /* Exit qualification */
    } arch.vmx;
};
```

Raw VMEXIT interception -- gives the monitor access to every VMEXIT event with full VMX reason and qualification. Very powerful but very high overhead.

### 2.10 I/O Events (VM_EVENT_REASON_IO_INSTRUCTION)

```c
struct vm_event_io {
    uint32_t bytes;     /* Size of I/O (1, 2, or 4) */
    uint16_t port;      /* I/O port number */
    uint8_t  in;        /* 1 = IN, 0 = OUT */
    uint8_t  str;       /* 1 = string instruction (INS/OUTS) */
};
```

### 2.11 Response Flags

When the monitor sends a response, it sets flags to control hypervisor behavior:

```c
VM_EVENT_FLAG_VCPU_PAUSED       (1 << 0)   /* vCPU is paused, needs unpause */
VM_EVENT_FLAG_FOREIGN           (1 << 1)   /* Event from foreign domain */
VM_EVENT_FLAG_EMULATE           (1 << 2)   /* Emulate the faulting instruction */
VM_EVENT_FLAG_EMULATE_NOWRITE   (1 << 3)   /* Emulate but suppress writes */
VM_EVENT_FLAG_TOGGLE_SINGLESTEP (1 << 4)   /* Toggle single-step mode */
VM_EVENT_FLAG_SET_EMUL_READ_DATA (1 << 5)  /* Provide custom read data for emulation */
VM_EVENT_FLAG_DENY              (1 << 6)   /* Deny the operation (block CR/MSR write) */
VM_EVENT_FLAG_ALTERNATE_P2M     (1 << 7)   /* Switch to alternate p2m view */
VM_EVENT_FLAG_SET_REGISTERS     (1 << 8)   /* Modify guest registers */
VM_EVENT_FLAG_SET_EMUL_INSN_DATA (1 << 9)  /* Provide instruction data for emulation */
VM_EVENT_FLAG_GET_NEXT_INTERRUPT (1 << 10) /* Request next interrupt event */
VM_EVENT_FLAG_FAST_SINGLESTEP   (1 << 11)  /* Fast single-step (no event) */
VM_EVENT_FLAG_NESTED_P2M        (1 << 12)  /* Nested p2m related */
VM_EVENT_FLAG_RESET_VMTRACE     (1 << 13)  /* Reset VM trace buffer */
VM_EVENT_FLAG_RESET_FORK_STATE  (1 << 14)  /* Reset fork state */
VM_EVENT_FLAG_RESET_FORK_MEMORY (1 << 15)  /* Reset fork memory */
```

### 2.12 Register State in Events

Every event includes full register context:

```c
struct vm_event_regs_x86 {
    uint64_t rax, rcx, rdx, rbx, rsp, rbp, rsi, rdi;
    uint64_t r8, r9, r10, r11, r12, r13, r14, r15;
    uint64_t rflags, dr6, dr7;
    uint64_t rip;
    uint64_t cr0, cr2, cr3, cr4;
    uint64_t sysenter_cs, sysenter_esp, sysenter_eip;
    uint64_t msr_efer, msr_star, msr_lstar;
    uint64_t gdtr_base;
    /* Segment registers with selector, base, limit, attributes */
    /* fs_base, gs_base, shadow_gs */
};

struct vm_event_regs_arm {
    uint64_t ttbr0, ttbr1;
    uint64_t ttbcr;
    uint64_t pc;
    uint32_t cpsr;
};
```

### 2.13 Main Event Structure

```c
typedef struct vm_event_st {
    uint32_t version;     /* VM_EVENT_INTERFACE_VERSION */
    uint32_t flags;       /* VM_EVENT_FLAG_* */
    uint32_t reason;      /* VM_EVENT_REASON_* */
    uint32_t vcpu_id;
    uint16_t altp2m_idx;  /* Active altp2m view index */

    union {
        vm_event_mem_access_t         mem_access;
        vm_event_write_ctrlreg_t      write_ctrlreg;
        vm_event_mov_to_msr_t         mov_to_msr;
        vm_event_desc_access_t        desc_access;
        vm_event_singlestep_t         singlestep;
        vm_event_fast_singlestep_t    fast_singlestep;
        vm_event_debug_t              debug;
        vm_event_cpuid_t              cpuid;
        vm_event_interrupt_x86_t      interrupt;
        vm_event_paging_t             paging;
        vm_event_sharing_t            sharing;
        vm_event_emul_read_data_t     emul_read_data;
        vm_event_emul_insn_data_t     emul_insn_data;
        vm_event_vmexit_t             vmexit;
        vm_event_io_t                 io;
    } u;

    union {
        union {
            vm_event_regs_x86_t x86;
            vm_event_regs_arm_t arm;
        } regs;
        vm_event_emul_read_data_t emul_read_data;
        vm_event_emul_insn_data_t emul_insn_data;
    } data;
} vm_event_request_t, vm_event_response_t;
```

---

## 3. Xen VMI Tools

### 3.1 DRAKVUF

**Repository**: https://github.com/tklengyel/drakvuf

DRAKVUF is a **virtualization-based agentless black-box binary analysis system** -- the most advanced open-source VMI tool for Xen.

#### Architecture

```text
+-------------------+
|   DRAKVUF Engine  |
|   (C/C++)         |
+--------+----------+
         |
+--------v----------+     +------------------+
|     LibVMI        |     |   Volatility3    |
| (Memory access +  |     | (Symbol/offset   |
|  event handling)  |     |  generation)     |
+--------+----------+     +------------------+
         |
+--------v----------+
|   Xen Hypervisor  |
| - vm_event ring   |
| - altp2m (EPT)    |
| - memory sharing  |
+--------+----------+
         |
+--------v----------+
|   Guest VM        |
| (Windows/Linux)   |
| (No agent needed) |
+-------------------+
```

#### How DRAKVUF Uses LibVMI + Xen

1. **Memory Reading**: LibVMI calls `xc_map_foreign_range()` to map guest physical pages into DRAKVUF's address space, translates virtual addresses using guest page tables
2. **Breakpoint Hooking**: Uses altp2m to create dual EPT views:
   - **View 0 (clean)**: Original code, execute-only
   - **View 1 (shadow)**: INT3-patched code, read-write only
3. **Event Processing**: LibVMI registers callbacks on Xen's vm_event ring; DRAKVUF dispatches to plugins
4. **Symbol Resolution**: Uses Volatility3's `pdbconv` (Windows PDB) or `dwarf2json` (Linux DWARF) to generate JSON profiles with kernel struct offsets

#### Requirements

- Intel CPU with VT-x and EPT (AMD not supported)
- Xen 4.17+ with altp2m enabled
- Boot params: `altp2m=1 hap_1gb=0 hap_2mb=0`

#### Plugin System

DRAKVUF plugins hook specific kernel functions to monitor:

| Plugin Category | Capabilities |
|----------------|-------------|
| **Syscall tracing** | Hook Windows/Linux syscall entry points |
| **File I/O** | Monitor NtCreateFile, NtReadFile, NtWriteFile |
| **Network** | Track TCP/UDP connections |
| **Process** | Monitor NtCreateProcess, fork/exec |
| **Registry** | Windows registry access monitoring |
| **Memory** | Heap allocation tracking, VirtualAlloc |
| **Injection** | Detect code injection (NtWriteVirtualMemory) |
| **Extraction** | Extract memory-mapped files from guest |

#### Supported Guest OS

- Windows 7-10 (32/64-bit)
- Linux 2.6.x through 6.x (32/64-bit)

#### DRAKVUF Sandbox

A separate project providing automated malware analysis: forks a clean VM snapshot, runs malware, collects DRAKVUF traces, destroys fork. Uses `XENMEM_sharing_op_fork` for instant VM cloning.

---

### 3.2 HVMI (Bitdefender Hypervisor Memory Introspection)

**Repository**: https://github.com/bitdefender/hvmi

#### Architecture Differences from DRAKVUF

| Aspect | DRAKVUF | HVMI |
|--------|---------|------|
| **Purpose** | Malware analysis / forensics | Real-time endpoint protection |
| **Approach** | Passive monitoring + tracing | Active prevention (blocks attacks) |
| **Hypervisor** | Xen only | Xen, KVM, Napoca (Bitdefender's own) |
| **Agent** | Agentless | Agentless |
| **LibVMI** | Yes (uses LibVMI) | No (defines own hypervisor interface) |
| **Focus** | System call tracing, behavioral analysis | Exploit prevention, rootkit detection |

#### HVMI Protection Capabilities

- Binary exploit prevention in protected processes
- Code/data injection detection and blocking
- Function hook detection in system DLLs (IAT/EAT hooks)
- Rootkit detection: inline kernel hooks, SSDT hooks, driver-object hooks
- Kernel exploit and privilege escalation prevention
- Credential theft detection
- Fileless malware detection (PowerShell command scanning)
- Compromised parent process detection

#### HVMI Hypervisor Interface

HVMI defines a **hypervisor-agnostic API** that must be implemented by each hypervisor:
- Memory read/write primitives
- EPT permission management
- Register access
- Event notification (memory violations, CR writes, MSR writes, etc.)

#### Guest Support (CAMI Database)

HVMI uses a **CAMI (Computer-Aided Machine Introspection)** database containing:
- Kernel structure offsets for each OS version
- Function signatures for hooking
- Known-legitimate access patterns (exception system to prevent false positives)

Pre-built support: Windows 7 SP1/SP2, Windows 10 1809, Ubuntu 18.04, CentOS 8.

---

### 3.3 pyvmidbg

**Repository**: https://github.com/pyvmidbg/pyvmidbg (Note: may be archived/moved)

A **VMI-based debugger** that implements a GDB stub on top of LibVMI:

- Presents a standard GDB remote protocol interface
- Uses LibVMI to read/write guest memory and registers
- Sets breakpoints via vm_event (INT3 or mem_access)
- Allows debugging a guest OS kernel without any in-guest debug agent
- Supports both Windows and Linux guests
- Can attach to a running VM without guest awareness

#### Key Advantage

Traditional kernel debugging requires:
- Serial/network debug connection to guest
- Debug symbols and agent in guest
- Guest must be in debug mode

pyvmidbg requires **none of these** -- it debugs from outside the VM using VMI, making it useful for:
- Malware debugging (malware cannot detect the debugger)
- Kernel debugging of production VMs
- Forensic analysis of compromised systems

---

### 3.4 xen-access (Built-in Example)

**Location**: `xen/tools/tests/xen-access/xen-access.c`

The reference implementation showing how to use Xen's VMI APIs:

#### Typical Usage Flow

```c
// 1. Open xenctrl interface
xc_interface *xch = xc_interface_open(NULL, NULL, 0);

// 2. Enable monitor ring (returns shared ring + event channel port)
void *ring_page = xc_monitor_enable(xch, domid, &port);

// 3. Initialize ring buffer
SHARED_RING_INIT((vm_event_sring_t *)ring_page);
FRONT_RING_INIT(&front_ring, (vm_event_sring_t *)ring_page, PAGE_SIZE);

// 4. Bind event channel
xc_evtchn *xce = xc_evtchn_open(NULL, 0);
xc_evtchn_bind_interdomain(xce, domid, port);

// 5. Enable desired events
xc_monitor_write_ctrlreg(xch, domid, VM_EVENT_X86_CR3, 1, 1, 0, 1);
xc_monitor_software_breakpoint(xch, domid, 1);
xc_set_mem_access(xch, domid, XENMEM_access_rw, target_gfn, 1);

// 6. Event loop
while (running) {
    rc = xc_evtchn_pending(xce);
    while (RING_HAS_UNCONSUMED_REQUESTS(&front_ring)) {
        RING_GET_REQUEST(&front_ring, req_cons, &req);
        req_cons++;

        // Process event based on req.reason
        memcpy(&rsp, &req, sizeof(rsp));
        rsp.flags = VM_EVENT_FLAG_VCPU_PAUSED;

        // Put response
        RING_PUT_RESPONSE(&front_ring, rsp_prod, &rsp);
        rsp_prod++;
    }
    RING_PUSH_RESPONSES(&front_ring);
    xc_evtchn_unmask(xce, port);
}

// 7. Cleanup
xc_monitor_disable(xch, domid);
```

---

## 4. Xen Versions and VMI Features

### 4.1 Version Timeline

| Version | Year | VMI Features |
|---------|------|-------------|
| **Xen 4.4** | 2014 | Initial VMI support: mem_event subsystem, basic memory access trapping (x86 only) |
| **Xen 4.5** | 2015 | Enhanced mem_event: more event types, better CR monitoring, INT3 interception. x86 only. |
| **Xen 4.6** | 2015 | **Major rewrite**: mem_event renamed to **vm_event**. Added: altp2m (x86), monitor subsystem separated from vm_event, descriptor access events, CPUID interception, debug exceptions. ARM support added. |
| **Xen 4.7** | 2016 | altp2m improvements, GFN remapping, batch memory access operations (`set_mem_access_multi`), emulation control flags |
| **Xen 4.8** | 2016 | Guest request events, improved single-step, `VM_EVENT_FLAG_SET_REGISTERS` for modifying guest state from monitor |
| **Xen 4.9** | 2017 | `VM_EVENT_FLAG_FAST_SINGLESTEP` (single-step without generating event), `VM_EVENT_FLAG_SET_EMUL_INSN_DATA`, improved emulation support |
| **Xen 4.10** | 2017 | **altp2m ARM support** added. VE (Virtualization Exception) support, suppress-VE per page. |
| **Xen 4.11** | 2018 | I/O port monitoring (`VM_EVENT_REASON_IO_INSTRUCTION`), improved MSR bitmap handling |
| **Xen 4.12** | 2019 | VMEXIT interception (`VM_EVENT_REASON_VMEXIT`), monitor capabilities query |
| **Xen 4.13** | 2019 | Domain forking via memory sharing (`XENMEM_sharing_op_fork`), used by DRAKVUF Sandbox |
| **Xen 4.14** | 2020 | Fork improvements with IOMMU/interrupt flags, `VM_EVENT_FLAG_RESET_FORK_STATE/MEMORY` |
| **Xen 4.15** | 2021 | `VM_EVENT_FLAG_RESET_VMTRACE` (Intel Processor Trace integration), altp2m view visibility control |
| **Xen 4.16** | 2021 | suppress-VE multi-page operations, altp2m vCPU p2m index query |
| **Xen 4.17** | 2023 | Stability improvements, CONFIG_VM_EVENT modularization begins |
| **Xen 4.18** | 2023 | Continued refinement of vm_event/monitor subsystems |
| **Xen 4.19** | 2024 | New hypercalls for vCPU runstate/time mapping by physical address |
| **Xen 4.20** | 2024 | CONFIG_VM_EVENT consolidation patches (wrapping mem_access, monitor, XSM under single config) |
| **Xen 4.21** | 2025 | Configurable number of altp2m tables per domain via `vm.cfg` |

### 4.2 Monitor API Evolution

**Pre-4.6 (mem_event era)**:
```c
/* Deprecated HVM params -- replaced by monitor subsystem */
HVM_PARAM_MEMORY_EVENT_CR0          (20)  /* Deprecated */
HVM_PARAM_MEMORY_EVENT_CR3          (21)  /* Deprecated */
HVM_PARAM_MEMORY_EVENT_CR4          (22)  /* Deprecated */
HVM_PARAM_MEMORY_EVENT_INT3         (23)  /* Deprecated */
HVM_PARAM_MEMORY_EVENT_SINGLE_STEP  (25)  /* Deprecated */
HVM_PARAM_MEMORY_EVENT_MSR          (30)  /* Deprecated */
```

**Post-4.6 (vm_event/monitor era)**:
```c
/* Active ring parameters */
HVM_PARAM_PAGING_RING_PFN   (27)  /* Paging ring GFN */
HVM_PARAM_MONITOR_RING_PFN  (28)  /* Monitor ring GFN */
HVM_PARAM_SHARING_RING_PFN  (29)  /* Sharing ring GFN */

/* Altp2m mode */
HVM_PARAM_ALTP2M            (35)  /* disabled/mixed/external/limited */
```

### 4.3 Key Feature Dependencies

| Feature | Requires |
|---------|---------|
| Memory access trapping | HAP (EPT/NPT) |
| altp2m | Intel EPT + `altp2m=1` boot param |
| VMFUNC EPTP switching | Intel Haswell+ |
| Virtualization Exceptions (#VE) | Intel Broadwell+ |
| Domain forking | Memory sharing enabled |
| VMEXIT interception | Xen 4.12+ |
| I/O port monitoring | Xen 4.11+ |
| Fast single-step | Xen 4.9+ |
| View visibility control | Xen 4.15+ |
| Configurable altp2m count | Xen 4.21+ |

---

## 5. Practical Setup

### 5.1 Hardware Requirements

- **CPU**: Intel with VT-x and EPT (mandatory for full VMI)
  - AMD (NPT/RVI) works for basic mem_access but **no altp2m support**
  - Haswell+ recommended for VMFUNC EPTP switching
  - Broadwell+ recommended for #VE support
- **RAM**: Minimum 8GB, 16GB+ recommended (dom0 + guest + monitor overhead)
- **Storage**: SSD recommended for VM images

### 5.2 Dom0 Configuration

#### Install Xen

```bash
# Debian/Ubuntu
apt-get install xen-hypervisor-amd64 xen-utils xen-tools libxen-dev

# Or build from source for latest VMI features
git clone https://xenbits.xen.org/git-http/xen.git
cd xen
./configure --enable-systemd
make -j$(nproc)
make install
```

#### GRUB Boot Configuration

```bash
# /etc/default/grub.d/xen.cfg
GRUB_CMDLINE_XEN="dom0_mem=4096M,max:4096M altp2m=1 hap_1gb=0 hap_2mb=0"
```

**Critical boot parameters**:
- `altp2m=1` -- Enable alternate p2m support (required for stealthy breakpoints)
- `hap_1gb=0 hap_2mb=0` -- Disable large pages in HAP (required for fine-grained page-level access control; without this, setting access on a 4KB page within a 2MB large page would require splitting)
- `dom0_mem=4096M` -- Reserve adequate memory for dom0

```bash
update-grub
reboot  # Boot into Xen hypervisor
```

#### Verify Xen is Running

```bash
xl info          # Should show Xen version, capabilities
xl list          # Should show Domain-0
xl dmesg | grep -i altp2m   # Verify altp2m enabled
```

### 5.3 Guest VM Configuration

#### Guest Config (`/etc/xen/guest.cfg`)

```python
type = "hvm"           # Must be HVM (not PV) for VMI
name = "target-vm"
memory = 4096
vcpus = 2
disk = ['file:/path/to/disk.img,hda,w']
vif = ['bridge=xenbr0']

# VMI-critical settings
altp2m = "external"    # Enable altp2m for this guest
                       # Options: "disabled", "mixed", "external", "limited"
hap = 1                # Hardware Assisted Paging (required)
```

**altp2m modes**:
- `disabled` -- No altp2m
- `mixed` -- Both guest and external tools can manage views
- `external` -- Only external tools (dom0/stubdom) can manage views (most secure for VMI)
- `limited` -- Guest can switch views but not create/modify them

#### Create and Start Guest

```bash
xl create /etc/xen/guest.cfg
xl list   # Verify guest is running
```

### 5.4 LibVMI Setup

```bash
# Install dependencies
apt-get install libglib2.0-dev libxen-dev flex bison libjson-c-dev

# Build LibVMI
git clone https://github.com/libvmi/libvmi.git
cd libvmi
mkdir build && cd build
cmake .. -DENABLE_XEN=ON -DENABLE_KVM=OFF
make -j$(nproc)
sudo make install
sudo ldconfig
```

#### LibVMI Configuration

Create `/etc/libvmi.conf`:

```text
target-vm {
    ostype = "Windows";
    win_tasks   = 0x188;    /* EPROCESS.ActiveProcessLinks offset */
    win_pdbase  = 0x028;    /* EPROCESS.DirectoryTableBase offset */
    win_pid     = 0x2e0;    /* EPROCESS.UniqueProcessId offset */
    win_pname   = 0x2e8;    /* EPROCESS.ImageFileName offset */
}
```

For Linux:
```text
target-vm {
    ostype = "Linux";
    sysmap = "/path/to/System.map";
    linux_tasks = 0x298;    /* task_struct.tasks offset */
    linux_mm    = 0x308;    /* task_struct.mm offset */
    linux_pid   = 0x2e0;    /* task_struct.pid offset */
    linux_pgd   = 0x050;    /* mm_struct.pgd offset */
}
```

Use `dwarf2json` (Linux) or Volatility3's `pdbconv.py` (Windows) to generate JSON profiles with exact offsets.

### 5.5 DRAKVUF Setup

```bash
# Build DRAKVUF with all dependencies
git clone --recursive https://github.com/tklengyel/drakvuf.git
cd drakvuf
# Xen, LibVMI, Volatility3, dwarf2json are submodules

# Build Xen (if not installed)
cd xen && ./configure && make -j$(nproc) && sudo make install && cd ..

# Build LibVMI
cd libvmi && mkdir build && cd build
cmake .. -DENABLE_XEN=ON && make -j$(nproc) && sudo make install && cd ../..

# Generate kernel profile (Windows example)
python3 volatility3/volatility3/framework/symbols/windows/pdbconv.py \
    --guid <GUID> --output /path/to/profile.json

# Build DRAKVUF
mkdir build && cd build
cmake .. -DENABLE_LINUX=ON -DENABLE_WINDOWS=ON
make -j$(nproc)
sudo make install
```

### 5.6 Running VMI

```bash
# Simple xen-access test
/usr/lib/xen/bin/xen-access <domid> [breakpoint|cr3|memaccess|...]

# DRAKVUF trace
drakvuf -r /path/to/profile.json -d <domid> -t <timeout>

# LibVMI example (process list)
vmi-process-list target-vm
```

---

## 6. Xen VMI Advantages Over KVM

### 6.1 Architecture Comparison

| Aspect | Xen | KVM |
|--------|-----|-----|
| **Hypervisor type** | Type-1 (bare-metal) | Type-2 (hosted, Linux kernel module) |
| **VMI maturity** | 10+ years of VMI development (since 2014) | KVMi patch set, not yet mainlined |
| **API stability** | Stable vm_event/monitor API | KVMi API still evolving |
| **altp2m** | Full support (16+ views, GFN remapping) | No equivalent (EPT view switching not exposed) |
| **Memory access** | First-class xenctrl API | Requires KVMi patches or /dev/mem hacks |
| **Event channel** | Dedicated Xen event channel (low latency) | KVMi uses kernel socket |
| **Domain forking** | Native support (copy-on-write VM clones) | No equivalent |
| **Isolation** | Monitor runs in separate domain (dom0) | Monitor runs as userspace process on same kernel |

### 6.2 Why Xen Is Preferred for VMI

#### a) Stronger Isolation

Xen's Type-1 architecture means the hypervisor runs directly on hardware. The monitor application runs in dom0, which is a separate domain from the guest. Even if the guest is compromised, it cannot reach dom0 without a hypervisor escape.

KVM's Type-2 architecture means the monitor runs as a Linux process on the same kernel as the hypervisor. A kernel exploit in the host could compromise the monitor.

#### b) Mature, Stable VMI API

Xen has had a dedicated VMI API (vm_event/monitor) since 2014, with:
- 17 distinct event types
- altp2m with 16+ EPT views
- Memory sharing and domain forking
- Comprehensive xenctrl library
- Active maintenance (CONFIG_VM_EVENT modularization in 2026)

KVM's VMI support (KVMi) has been in patch form since ~2017 but has **never been merged into mainline Linux**. The API is unstable and requires custom kernel builds.

#### c) altp2m (No KVM Equivalent)

Xen's altp2m is the killer feature for stealthy VMI:
- Multiple EPT views with per-view access permissions
- GFN-to-MFN remapping per view
- VMFUNC-based fast switching
- Enables invisible breakpoints, shadow page monitoring

KVM has no equivalent mechanism. Stealthy breakpoints on KVM require workarounds that are slower and less robust.

#### d) Domain Forking

Xen supports instant copy-on-write VM cloning via memory sharing. This is critical for:
- Malware sandbox scaling (DRAKVUF Sandbox processes many samples in parallel)
- Snapshot-based analysis (fork, analyze, destroy -- no disk I/O)

KVM has no native VM forking capability.

#### e) Dedicated Event Channel

Xen's event channel provides low-latency asynchronous notification between hypervisor and monitor. The shared ring buffer with backpressure handling is purpose-built for high-throughput event delivery.

KVM relies on standard Linux IPC mechanisms which add overhead and are not optimized for VMI event volumes.

#### f) LibVMI Event Support

LibVMI's event handling (memory events, register events, interrupt events) is **only fully supported on Xen**. The KVM driver in LibVMI provides basic memory read/write but event support requires the unmerged KVMi patches.

#### g) Ecosystem

The entire VMI ecosystem is Xen-centric:
- **DRAKVUF**: Xen only
- **LibVMI**: Full support for Xen, limited KVM
- **HVMI**: Supports Xen natively
- **xen-access**: Xen reference implementation
- **Academic research**: Vast majority of VMI papers use Xen

### 6.3 KVM's Advantages (for completeness)

- **Deployment**: KVM is built into Linux kernel -- no separate hypervisor install
- **Cloud adoption**: Most clouds use KVM (AWS Nitro, GCP, etc.)
- **Performance**: Lower overhead for non-VMI workloads
- **Development velocity**: Linux kernel development pace is faster

### 6.4 When to Use Each

| Use Case | Recommended |
|----------|-------------|
| Malware analysis sandbox | Xen (DRAKVUF) |
| Real-time endpoint protection | Xen (HVMI) |
| Security research / VMI development | Xen |
| Cloud workload monitoring | KVM (if basic), Xen (if deep VMI needed) |
| Forensic analysis | Xen |
| Production server monitoring | KVM (simpler deployment) |

---

## References

### Source Code
- `xen/include/public/vm_event.h` -- Event types, structures, flags
- `xen/include/public/hvm/hvm_op.h` -- altp2m hypercall definitions
- `xen/include/public/domctl.h` -- Monitor and vm_event domctl operations
- `xen/include/public/memory.h` -- Memory access and sharing operations
- `xen/include/public/grant_table.h` -- Grant table operations
- `xen/common/vm_event.c` -- Ring buffer implementation
- `xen/arch/x86/monitor.c` -- Monitor subsystem (event enable/disable)
- `xen/arch/x86/mm/altp2m.c` -- altp2m implementation
- `xen/arch/x86/mm/p2m.c` -- p2m memory mapping
- `tools/include/xenctrl.h` -- xenctrl API (xc_monitor_*, xc_altp2m_*, xc_mem_access_*)
- `tools/tests/xen-access/xen-access.c` -- Reference VMI implementation

### Tools
- DRAKVUF: https://github.com/tklengyel/drakvuf
- LibVMI: https://github.com/libvmi/libvmi
- HVMI: https://github.com/bitdefender/hvmi
- Xen Project: https://xenproject.org/

### Documentation
- Xen VMI Wiki: https://wiki.xenproject.org/wiki/Virtual_Machine_Introspection
- LibVMI API: https://libvmi.com/api/
- DRAKVUF: https://drakvuf.com/
