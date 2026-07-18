# libvirt QEMU/KVM Real-VM Qualification — 2026-07-18

## Result

**Passed for the provider's advertised control and acquisition capabilities.**
Commit `7dbba6f` was built in release mode and exercised through the actual
`vmi-cli` against a real libvirt-managed QEMU process.

This run used QEMU TCG because nested KVM was not exposed to the qualification
host. The measurements are valid for this host and topology but are not a KVM
hardware-acceleration baseline.

## Environment

| Property | Value |
|---|---|
| Host | `scuxvsa-524` |
| Kernel | Linux `6.8.0-90-generic` x86_64 |
| Host CPU exposed by hypervisor | Intel Xeon Processor (Skylake) |
| libvirt | 10.0.0 |
| QEMU | 8.2.2, Ubuntu package `1:8.2.2+ds-0ubuntu1.17` |
| Acceleration | TCG (nested KVM unavailable) |
| Domain | `libvmi-qual-20260718`, disposable and diskless |
| Guest allocation | 1 vCPU, 128 MiB RAM, no network |
| Connection | `qemu:///system` |
| Domain XML SHA-256 | `21e2520f4397f3908edb47944354cf077434c4da45c20e66a3b93723dd09ca3c` |

## Feature results

| Feature | Result | Evidence |
|---|---|---|
| QEMU domain attachment | Pass | XML type accepted and status returned `Running` |
| State query | Pass | `libvirt-status` returned `Running` |
| Suspend | Pass | `libvirt-pause` returned `Paused` |
| Resume | Pass | `libvirt-resume` returned `Running` |
| Full memory acquisition | Pass | 134,480,867-byte ELF core produced |
| ELF parsing | Pass | GPA 0 returned `53 ff 00 f0 53 ff 00 f0 c3 e2 00 f0 53 ff 00 f0` |
| Physical-range acquisition | Pass | 4,096-byte output produced from GPA 0 |
| Existing-output protection | Pass | Second acquisition failed with the expected refusal |

The feature run is also recorded in
[`libvirt-qemu-real-vm-features-2026-07-18.json`](libvirt-qemu-real-vm-features-2026-07-18.json).

## Performance results

All timings include process startup, `virsh`, libvirt communication, and the
provider operation. Full raw samples are in
[`libvirt-qemu-real-vm-2026-07-18.json`](libvirt-qemu-real-vm-2026-07-18.json).

| Operation | Samples | Median | p95 | Throughput |
|---|---:|---:|---:|---:|
| Status | 30 | 119.00 ms | 124.89 ms | n/a |
| Pause | 10 | 146.13 ms | 152.03 ms | n/a |
| Resume | 10 | 144.10 ms | 149.49 ms | n/a |
| Full 128 MiB dump | 5 | 1,041.66 ms | 1,085.00 ms | 123.12 MiB/s |
| 4 KiB range | 3 | 1,571.10 ms | 1,571.10 ms | 0.0025 MiB/s |
| 1 MiB range | 3 | 2,434.56 ms | 2,434.56 ms | 0.411 MiB/s |
| 64 MiB range | 3 | 3,969.26 ms | 3,969.26 ms | 16.12 MiB/s |

Range acquisition currently creates and parses a full temporary core for each
request, so small-range latency and effective throughput are expected to be
poor. The 64 MiB samples also showed substantial variance (2.77–8.65 seconds),
which should be revisited on a dedicated KVM host with controlled storage.

## Cleanup

The domain was destroyed and undefined. All temporary cores, extracted ranges,
benchmark scripts, and remote result files were removed. The final libvirt
domain list was empty. Installed libvirt/QEMU and Rust build dependencies remain
on the host.

