use std::sync::Arc;

use vmi_artifact::SnapshotBundle;
use vmi_driver_api::{Connector, MemoryAccess, Session};
use vmi_types::{
    AttachRequest, Capability, CapabilitySet, ConsistencyMode, Gpa, GuestArchitecture,
    ProviderDescriptor, ProviderMaturity, Result, TargetDescriptor, TargetSelector, VmiError,
};

#[derive(Clone, Debug)]
pub struct DumpConnector {
    descriptor: Arc<ProviderDescriptor>,
    target: Arc<TargetDescriptor>,
    bundle: SnapshotBundle,
}

impl DumpConnector {
    pub fn new(bundle: SnapshotBundle, architecture: GuestArchitecture) -> Self {
        let capabilities = CapabilitySet::from_caps([Capability::MemoryRead]);
        let target_name = bundle.provenance.source.clone();
        Self {
            descriptor: Arc::new(ProviderDescriptor::new(
                "raw-dump",
                "Raw Memory Dump",
                ProviderMaturity::Preview,
                capabilities,
            )),
            target: Arc::new(TargetDescriptor::new(
                target_name.clone(),
                Some(target_name),
                architecture,
                ConsistencyMode::ImmutableSnapshot,
            )),
            bundle,
        }
    }
}

impl Connector for DumpConnector {
    fn descriptor(&self) -> &ProviderDescriptor {
        &self.descriptor
    }

    fn connect(&self, request: AttachRequest) -> Result<Box<dyn Session>> {
        let missing = request
            .required_capabilities
            .difference_of(self.descriptor.capabilities);
        if !missing.is_empty() {
            return Err(VmiError::AttachRejected {
                provider: try_owned_provider_id(&self.descriptor.id)?,
                missing,
            });
        }
        if let TargetSelector::Named(expected) = request.selector {
            if expected != self.target.id {
                return Err(VmiError::Backend(format!(
                    "dump target {expected} not found"
                )));
            }
        }
        Ok(Box::new(DumpSession {
            descriptor: self.descriptor.clone(),
            target: self.target.clone(),
            bundle: self.bundle.clone(),
        }))
    }
}

#[derive(Clone, Debug)]
struct DumpSession {
    descriptor: Arc<ProviderDescriptor>,
    target: Arc<TargetDescriptor>,
    bundle: SnapshotBundle,
}

fn try_owned_provider_id(provider_id: &str) -> Result<String> {
    let mut owned = String::new();
    owned
        .try_reserve_exact(provider_id.len())
        .map_err(|error| {
            VmiError::Backend(format!("failed to allocate dump provider ID: {error}"))
        })?;
    owned.push_str(provider_id);
    Ok(owned)
}

impl MemoryAccess for DumpSession {
    fn read_into(&self, address: Gpa, buffer: &mut [u8]) -> Result<()> {
        self.bundle.read_into(address, buffer)
    }
}

impl Session for DumpSession {
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
    use std::sync::Arc;

    use super::*;
    use vmi_driver_api::read_bytes;

    #[test]
    fn exposes_raw_snapshot_as_immutable_physical_memory() {
        let bundle = SnapshotBundle::from_raw(
            "qemu-page.bin",
            Gpa::new(0),
            Arc::from([0x53, 0xff, 0x00, 0xf0]),
        );
        let connector = DumpConnector::new(bundle, GuestArchitecture::Amd64);
        let session = connector
            .connect(AttachRequest::any(CapabilitySet::from_caps([
                Capability::MemoryRead,
            ])))
            .unwrap();
        assert_eq!(
            session.target().consistency,
            ConsistencyMode::ImmutableSnapshot
        );
        assert_eq!(
            read_bytes(session.memory().unwrap(), Gpa::new(0), 4).unwrap(),
            [0x53, 0xff, 0x00, 0xf0]
        );
    }
}
