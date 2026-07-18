# libvirt QEMU/KVM Provider

The `libvirt-qemu` provider integrates with QEMU and KVM domains managed by
libvirt. It complements the direct `qemu-qmp` provider: operators do not need to
discover or expose a QMP socket, but the available contract is intentionally
narrower.

The connector executes bounded `virsh` commands, optionally against the URI in
`LIBVIRT_DEFAULT_URI`. Attachment validates that domain XML identifies a `qemu`
or `kvm` domain and rejects empty, option-like, or control-character-bearing
domain names and URIs.

## Capabilities

- execution-state queries through `domstate`;
- pause and resume through `suspend` and `resume`;
- full memory-only ELF acquisition through `dump`;
- physical-range extraction by acquiring a temporary ELF core and passing it
  through the shared bounded artifact parser.

The provider does not advertise direct memory access, registers, writes, or
events. Inspect a retained core with `read-elf`, or attach the resulting
`SnapshotBundle` through the immutable dump provider.

## Commands

```console
LIBVIRT_DEFAULT_URI=qemu:///system vmi-cli libvirt-status guest
LIBVIRT_DEFAULT_URI=qemu:///system vmi-cli libvirt-pause guest
LIBVIRT_DEFAULT_URI=qemu:///system vmi-cli libvirt-resume guest
LIBVIRT_DEFAULT_URI=qemu:///system vmi-cli libvirt-dump guest guest.core
LIBVIRT_DEFAULT_URI=qemu:///system vmi-cli libvirt-acquire guest range.bin 0 4096
vmi-cli read-elf guest.core 0 64
```

Set `VIRSH` to an alternate executable path. Output files are never replaced,
command output is bounded, commands have a finite timeout, temporary cores use
collision-resistant names, and successful range publication is synchronized
before return.

## Maturity

The provider is Experimental until a pinned libvirt/QEMU host matrix validates
running and paused domains, local and remote URIs, memory-only dump consistency,
failure recovery, permissions, and a sustained acquisition soak. Proxmox,
KubeVirt, and OpenStack are deployment environments over QEMU/KVM rather than
separate memory formats; they can use this adapter where host policy permits
libvirt access.

Official references:

- [virsh command reference](https://www.libvirt.org/manpages/virsh.html)
- [libvirt domain XML](https://www.libvirt.org/formatdomain)
- [libvirt virtual-machine lifecycle](https://wiki.libvirt.org/VM_lifecycle.html)
