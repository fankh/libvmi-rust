# VMware Artifact Qualification

The VMware scope is deliberately offline. `vmware` reads a contiguous flat
`.vmem` file as guest physical RAM, while `vmware-core` reads an ELF or Windows
KDMP produced by `vmss2core`. Neither provider controls a running VMware VM,
reads live vCPU registers, or delivers hypervisor events.

Broadcom documents `.vmem` as the RAM of a running VM or the RAM retained with a
snapshot. A checkpoint normally also includes `.vmsn` or `.vmss` state. For
guest-aware conversion, use the `vmss2core` version bundled with a current
VMware Workstation or Fusion installation and retain every checkpoint component.

The direct `.vmem` provider requires a contiguous physical layout and an
operator-supplied physical base, normally zero. Some ESXi configurations omit
untouched pages and produce a sparse memory artifact. Such captures must not be
treated as flat GPA space; convert them with VMware tooling or normalize them
into a versioned manifest before inspection.

Artifact loading is currently eager and bounded by the shared 64 GiB artifact
ceiling. Large captures therefore require enough host memory for normalization;
a future file-backed segmented reader is required before direct `.vmem` can be
recommended for routinely inspecting large production guests.

## Commands

```console
vmi-cli read-vmware-vmem guest.vmem 0 0x1000 64
vmi-cli read-vmware-core vmss.core 0x1000 64
```

Run qualification against vendor-captured artifacts:

```console
python scripts/qualify-vmware-artifacts.py \
  --vmem guest-Snapshot1.vmem \
  --converted-core vmss.core \
  --source vendor-captured \
  --output reports/vmware-artifacts/vendor-qualification.json
```

Use `--physical-base`, `--vmem-address`, or `--core-address` when the capture
does not use the defaults. The qualifier records artifact and probe hashes,
repeatability, bounds behavior, and complete CLI/provider latency statistics.
It never records guest memory bytes in the evidence file.

`reports/vmware-artifacts/synthetic-qualification.json` proves the deterministic
connector and converted-core paths but is not evidence from VMware software.
Promotion of direct `.vmem` support beyond Experimental requires sanitized,
redistributable captures from a pinned Workstation/Fusion or ESXi matrix.

## Vendor References

- [VMware virtual-machine file types](https://knowledge.broadcom.com/external/article/303392/contents-of-the-virtual-machine-bundle-i.html)
- [Converting checkpoints with vmss2core](https://knowledge.broadcom.com/external/article/323788/converting-a-snapshot-file-to-memory-dum.html)
- [Sparse ESXi memory snapshot behavior](https://knowledge.broadcom.com/external/article/447475/when-generating-a-memory-dump-from-a-vmw.html)
