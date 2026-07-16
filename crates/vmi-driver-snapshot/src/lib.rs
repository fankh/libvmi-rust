use std::{path::Path, sync::Arc};

use vmi_artifact::SnapshotBundle;
use vmi_driver_api::{Connector, MemoryAccess, Session};
use vmi_types::{
    AttachRequest, Capability, CapabilitySet, ConsistencyMode, Gpa, GuestArchitecture,
    ProviderDescriptor, ProviderMaturity, Result, TargetDescriptor, TargetSelector, VmiError,
};

#[derive(Clone, Debug)]
pub struct SnapshotConnector {
    descriptor: Arc<ProviderDescriptor>,
    target: Arc<TargetDescriptor>,
    bundle: SnapshotBundle,
}

impl SnapshotConnector {
    pub fn open_virtualbox_core(
        architecture: GuestArchitecture,
        path: impl AsRef<Path>,
    ) -> Result<Self> {
        Self::new(
            "virtualbox-core",
            "VirtualBox VM Core",
            architecture,
            SnapshotBundle::elf_vmcore_file(path)?,
        )
    }

    pub fn open_vmware_vmem(
        architecture: GuestArchitecture,
        path: impl AsRef<Path>,
        physical_base: Gpa,
    ) -> Result<Self> {
        Self::new(
            "vmware",
            "VMware VMEM Snapshot",
            architecture,
            SnapshotBundle::raw_file(path, physical_base)?,
        )
    }

    pub fn open_vmware_converted_core(
        architecture: GuestArchitecture,
        path: impl AsRef<Path>,
    ) -> Result<Self> {
        Self::new(
            "vmware-core",
            "VMware Converted Core",
            architecture,
            SnapshotBundle::converted_core_file(path)?,
        )
    }

    pub fn open_hyperv_manifest(
        architecture: GuestArchitecture,
        path: impl AsRef<Path>,
    ) -> Result<Self> {
        Self::open_manifest("hyperv", "Hyper-V Saved State", architecture, path)
    }

    pub fn open_hyperv_converted_core(
        architecture: GuestArchitecture,
        path: impl AsRef<Path>,
    ) -> Result<Self> {
        Self::new(
            "hyperv-core",
            "Hyper-V Converted Core",
            architecture,
            SnapshotBundle::converted_core_file(path)?,
        )
    }

    pub fn open_bhyve_manifest(path: impl AsRef<Path>) -> Result<Self> {
        Self::open_manifest("bhyve", "bhyve Saved State", GuestArchitecture::Amd64, path)
    }

    pub fn open_bhyve_converted_core(path: impl AsRef<Path>) -> Result<Self> {
        Self::new(
            "bhyve-core",
            "bhyve Converted Core",
            GuestArchitecture::Amd64,
            SnapshotBundle::converted_core_file(path)?,
        )
    }

    pub fn open_firecracker_manifest(path: impl AsRef<Path>) -> Result<Self> {
        Self::open_manifest(
            "firecracker",
            "Firecracker Snapshot",
            GuestArchitecture::Amd64,
            path,
        )
    }

    pub fn open_cloud_hypervisor_manifest(
        architecture: GuestArchitecture,
        path: impl AsRef<Path>,
    ) -> Result<Self> {
        Self::open_manifest(
            "cloud-hypervisor",
            "Cloud Hypervisor Snapshot",
            architecture,
            path,
        )
    }

    pub fn open_manifest(
        provider_id: impl Into<String>,
        display_name: impl Into<String>,
        architecture: GuestArchitecture,
        path: impl AsRef<Path>,
    ) -> Result<Self> {
        Self::new(
            provider_id,
            display_name,
            architecture,
            SnapshotBundle::manifest_file(path)?,
        )
    }

    pub fn open_xen_core(architecture: GuestArchitecture, path: impl AsRef<Path>) -> Result<Self> {
        Self::new(
            "xen-core",
            "Xen Core Dump",
            architecture,
            SnapshotBundle::xen_core_file(path)?,
        )
    }

    pub fn open_kdmp(path: impl AsRef<Path>) -> Result<Self> {
        Self::new(
            "windows-kdmp",
            "Windows Kernel/Complete Dump",
            GuestArchitecture::Amd64,
            SnapshotBundle::kdmp_file(path)?,
        )
    }

    pub fn new(
        provider_id: impl Into<String>,
        display_name: impl Into<String>,
        architecture: GuestArchitecture,
        bundle: SnapshotBundle,
    ) -> Result<Self> {
        let provider_id = provider_id.into();
        let display_name = display_name.into();
        if provider_id.is_empty() || display_name.is_empty() {
            return Err(VmiError::Backend(
                "snapshot provider ID and display name must not be empty".into(),
            ));
        }
        let capabilities = CapabilitySet::from_caps([Capability::MemoryRead]);
        let source = try_owned_text(&bundle.provenance.source, "snapshot target ID")?;
        let target_name =
            try_snapshot_target_name(&bundle.provenance.source, &bundle.provenance.format)?;
        Ok(Self {
            descriptor: Arc::new(ProviderDescriptor::new(
                provider_id,
                display_name,
                ProviderMaturity::Preview,
                capabilities,
            )),
            target: Arc::new(TargetDescriptor::new(
                source,
                Some(target_name),
                architecture,
                ConsistencyMode::ImmutableSnapshot,
            )),
            bundle,
        })
    }
}

fn try_owned_text(value: &str, description: &str) -> Result<String> {
    let mut owned = String::new();
    owned
        .try_reserve_exact(value.len())
        .map_err(|error| VmiError::Backend(format!("failed to allocate {description}: {error}")))?;
    owned.push_str(value);
    Ok(owned)
}

fn try_snapshot_target_name(source: &str, format_name: &str) -> Result<String> {
    let capacity = source
        .len()
        .checked_add(format_name.len())
        .and_then(|length| length.checked_add(3))
        .ok_or_else(|| VmiError::Backend("snapshot target name length overflow".into()))?;
    let mut target_name = String::new();
    target_name.try_reserve_exact(capacity).map_err(|error| {
        VmiError::Backend(format!("failed to allocate snapshot target name: {error}"))
    })?;
    target_name.push_str(source);
    target_name.push_str(" (");
    target_name.push_str(format_name);
    target_name.push(')');
    Ok(target_name)
}

impl Connector for SnapshotConnector {
    fn descriptor(&self) -> &ProviderDescriptor {
        &self.descriptor
    }

    fn connect(&self, request: AttachRequest) -> Result<Box<dyn Session>> {
        let missing = request
            .required_capabilities
            .difference_of(self.descriptor.capabilities);
        if !missing.is_empty() {
            return Err(VmiError::AttachRejected {
                provider: try_owned_text(&self.descriptor.id, "snapshot provider ID")?,
                missing,
            });
        }
        if let TargetSelector::Named(expected) = request.selector {
            if expected != self.target.id
                && self.target.display_name.as_deref() != Some(expected.as_str())
            {
                return Err(VmiError::Backend(format!(
                    "snapshot target {expected} not found"
                )));
            }
        }
        Ok(Box::new(SnapshotSession {
            descriptor: self.descriptor.clone(),
            target: self.target.clone(),
            bundle: self.bundle.clone(),
        }))
    }
}

#[derive(Clone, Debug)]
struct SnapshotSession {
    descriptor: Arc<ProviderDescriptor>,
    target: Arc<TargetDescriptor>,
    bundle: SnapshotBundle,
}

impl MemoryAccess for SnapshotSession {
    fn read_into(&self, address: Gpa, output: &mut [u8]) -> Result<()> {
        self.bundle.read_into(address, output)
    }
}

impl Session for SnapshotSession {
    fn provider(&self) -> &ProviderDescriptor {
        &self.descriptor
    }
    fn target(&self) -> &TargetDescriptor {
        &self.target
    }
    fn capabilities(&self) -> CapabilitySet {
        self.descriptor.capabilities
    }
    fn memory(&self) -> Result<&dyn MemoryAccess> {
        Ok(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        sync::Arc,
        time::{SystemTime, UNIX_EPOCH},
    };
    use vmi_driver_api::read_bytes;

    #[test]
    fn snapshot_target_name_is_exact_for_unicode_source_and_format() {
        assert_eq!(
            try_snapshot_target_name("guest-☃.mem", "raw").unwrap(),
            "guest-☃.mem (raw)"
        );
        assert_eq!(try_owned_text("target", "test").unwrap(), "target");
    }

    #[test]
    fn exposes_manifest_bundle_as_named_immutable_provider() {
        let bundle = SnapshotBundle::from_raw("vm.mem", Gpa::new(0x1000), Arc::from([1, 2, 3, 4]));
        let connector = SnapshotConnector::new(
            "firecracker-snapshot",
            "Firecracker Snapshot",
            GuestArchitecture::Amd64,
            bundle,
        )
        .unwrap();
        let session = connector
            .connect(AttachRequest::named(
                "vm.mem",
                CapabilitySet::from_caps([Capability::MemoryRead]),
            ))
            .unwrap();
        assert_eq!(session.provider().id, "firecracker-snapshot");
        assert_eq!(
            session.target().consistency,
            ConsistencyMode::ImmutableSnapshot
        );
        assert_eq!(
            read_bytes(session.memory().unwrap(), Gpa::new(0x1000), 4).unwrap(),
            [1, 2, 3, 4]
        );
        assert!(connector
            .connect(AttachRequest::any(CapabilitySet::from_caps([
                Capability::MemoryWrite
            ])))
            .is_err());
        assert!(connector
            .connect(AttachRequest::named("missing", CapabilitySet::empty()))
            .is_err());
    }

    #[test]
    fn exposes_dedicated_microvm_snapshot_providers() {
        let directory = std::env::temp_dir().join(format!(
            "vmi-microvm-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("memory.bin"), [7, 8, 9, 10]).unwrap();
        let manifest = directory.join("snapshot.json");
        fs::write(
            &manifest,
            r#"{"version":1,"format":"microvm-snapshot","segments":[{"file":"memory.bin","file_offset":0,"length":4,"gpa":"0x1000"}]}"#,
        )
        .unwrap();
        for connector in [
            SnapshotConnector::open_firecracker_manifest(&manifest).unwrap(),
            SnapshotConnector::open_cloud_hypervisor_manifest(
                GuestArchitecture::Aarch64,
                &manifest,
            )
            .unwrap(),
        ] {
            let session = connector.connect(AttachRequest::default()).unwrap();
            assert_eq!(
                session.capabilities(),
                CapabilitySet::from_caps([Capability::MemoryRead])
            );
            assert_eq!(
                read_bytes(session.memory().unwrap(), Gpa::new(0x1000), 4).unwrap(),
                [7, 8, 9, 10]
            );
            assert_eq!(
                session.target().consistency,
                ConsistencyMode::ImmutableSnapshot
            );
        }
        assert_eq!(
            SnapshotConnector::open_firecracker_manifest(&manifest)
                .unwrap()
                .descriptor()
                .id,
            "firecracker"
        );
        assert_eq!(
            SnapshotConnector::open_cloud_hypervisor_manifest(GuestArchitecture::Amd64, &manifest,)
                .unwrap()
                .descriptor()
                .id,
            "cloud-hypervisor"
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn exposes_dedicated_vendor_snapshot_providers() {
        let directory = std::env::temp_dir().join(format!(
            "vmi-vendor-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&directory).unwrap();
        let memory = directory.join("memory.bin");
        fs::write(&memory, [0xaa, 0xbb, 0xcc, 0xdd]).unwrap();
        let manifest = directory.join("snapshot.json");
        fs::write(
            &manifest,
            r#"{"version":1,"format":"vendor-saved-state","segments":[{"file":"memory.bin","file_offset":0,"length":4,"gpa":"0x2000"}]}"#,
        )
        .unwrap();
        let providers = [
            SnapshotConnector::open_vmware_vmem(
                GuestArchitecture::Amd64,
                &memory,
                Gpa::new(0x2000),
            )
            .unwrap(),
            SnapshotConnector::open_hyperv_manifest(GuestArchitecture::Amd64, &manifest).unwrap(),
            SnapshotConnector::open_bhyve_manifest(&manifest).unwrap(),
        ];
        assert_eq!(providers[0].descriptor().id, "vmware");
        assert_eq!(providers[1].descriptor().id, "hyperv");
        assert_eq!(providers[2].descriptor().id, "bhyve");
        for provider in providers {
            let session = provider.connect(AttachRequest::default()).unwrap();
            assert_eq!(
                read_bytes(session.memory().unwrap(), Gpa::new(0x2000), 4).unwrap(),
                [0xaa, 0xbb, 0xcc, 0xdd]
            );
            assert_eq!(
                session.capabilities(),
                CapabilitySet::from_caps([Capability::MemoryRead])
            );
        }
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn exposes_virtualbox_elf_core_provider() {
        let directory = std::env::temp_dir().join(format!(
            "vmi-vbox-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&directory).unwrap();
        let core = directory.join("vm.core");
        let mut elf = vec![0u8; 132];
        elf[..6].copy_from_slice(b"\x7fELF\x02\x01");
        elf[32..40].copy_from_slice(&64u64.to_le_bytes());
        elf[54..56].copy_from_slice(&56u16.to_le_bytes());
        elf[56..58].copy_from_slice(&1u16.to_le_bytes());
        elf[64..68].copy_from_slice(&1u32.to_le_bytes());
        elf[72..80].copy_from_slice(&128u64.to_le_bytes());
        elf[88..96].copy_from_slice(&0x3000u64.to_le_bytes());
        elf[96..104].copy_from_slice(&4u64.to_le_bytes());
        elf[104..112].copy_from_slice(&4u64.to_le_bytes());
        elf[128..132].copy_from_slice(&[0x11, 0x22, 0x33, 0x44]);
        fs::write(&core, elf).unwrap();
        let connector =
            SnapshotConnector::open_virtualbox_core(GuestArchitecture::Amd64, &core).unwrap();
        assert_eq!(connector.descriptor().id, "virtualbox-core");
        let session = connector.connect(AttachRequest::default()).unwrap();
        assert_eq!(
            read_bytes(session.memory().unwrap(), Gpa::new(0x3000), 4).unwrap(),
            [0x11, 0x22, 0x33, 0x44]
        );
        assert_eq!(
            session.target().consistency,
            ConsistencyMode::ImmutableSnapshot
        );
        let vmware =
            SnapshotConnector::open_vmware_converted_core(GuestArchitecture::Amd64, &core).unwrap();
        assert_eq!(vmware.descriptor().id, "vmware-core");
        let session = vmware.connect(AttachRequest::default()).unwrap();
        assert_eq!(
            read_bytes(session.memory().unwrap(), Gpa::new(0x3000), 4).unwrap(),
            [0x11, 0x22, 0x33, 0x44]
        );
        for connector in [
            SnapshotConnector::open_hyperv_converted_core(GuestArchitecture::Amd64, &core).unwrap(),
            SnapshotConnector::open_bhyve_converted_core(&core).unwrap(),
        ] {
            let session = connector.connect(AttachRequest::default()).unwrap();
            assert_eq!(
                read_bytes(session.memory().unwrap(), Gpa::new(0x3000), 4).unwrap(),
                [0x11, 0x22, 0x33, 0x44]
            );
        }
        fs::remove_dir_all(directory).unwrap();
    }
}
