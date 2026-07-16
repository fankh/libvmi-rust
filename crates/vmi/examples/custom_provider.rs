use std::sync::Arc;

use vmi::{
    driver::{Connector, MemoryAccess, Session},
    AttachRequest, Capability, CapabilitySet, ConsistencyMode, Gpa, GuestArchitecture,
    ProviderDescriptor, ProviderMaturity, Result, TargetDescriptor, VmiError, VmiSession,
};

struct ExampleConnector {
    descriptor: Arc<ProviderDescriptor>,
    target: Arc<TargetDescriptor>,
}

impl ExampleConnector {
    fn new() -> Self {
        Self {
            descriptor: Arc::new(ProviderDescriptor::new(
                "example-memory",
                "Example Memory Provider",
                ProviderMaturity::Experimental,
                CapabilitySet::from_caps([Capability::MemoryRead]),
            )),
            target: Arc::new(TargetDescriptor::new(
                "example-target",
                Some("Example Target"),
                GuestArchitecture::Amd64,
                ConsistencyMode::ImmutableSnapshot,
            )),
        }
    }
}

impl Connector for ExampleConnector {
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
        Ok(Box::new(ExampleSession {
            descriptor: self.descriptor.clone(),
            target: self.target.clone(),
        }))
    }
}

struct ExampleSession {
    descriptor: Arc<ProviderDescriptor>,
    target: Arc<TargetDescriptor>,
}

fn try_owned_provider_id(provider_id: &str) -> Result<String> {
    let mut owned = String::new();
    owned
        .try_reserve_exact(provider_id.len())
        .map_err(|error| {
            VmiError::Backend(format!("failed to allocate example provider ID: {error}"))
        })?;
    owned.push_str(provider_id);
    Ok(owned)
}

impl MemoryAccess for ExampleSession {
    fn read_into(&self, address: Gpa, output: &mut [u8]) -> Result<()> {
        const MEMORY: &[u8] = b"VMI!";
        let start = usize::try_from(address.raw()).map_err(|_| VmiError::ReadFailed {
            address: address.raw(),
            length: output.len(),
        })?;
        let end = start
            .checked_add(output.len())
            .ok_or(VmiError::ReadFailed {
                address: address.raw(),
                length: output.len(),
            })?;
        let source = MEMORY.get(start..end).ok_or(VmiError::ReadFailed {
            address: address.raw(),
            length: output.len(),
        })?;
        output.copy_from_slice(source);
        Ok(())
    }
}

impl Session for ExampleSession {
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

fn main() -> Result<()> {
    let connector = ExampleConnector::new();
    let session = VmiSession::attach(
        &connector,
        AttachRequest::any(CapabilitySet::from_caps([Capability::MemoryRead])),
    )?;
    let bytes = session.read_bytes(Gpa::new(0), 4)?;
    assert_eq!(bytes, b"VMI!");
    println!(
        "{}: {}",
        session.session().provider().id,
        String::from_utf8_lossy(&bytes)
    );
    Ok(())
}
