use std::{
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};

use vmi_types::{
    AttachRequest, ByteOrder, Capability, CapabilitySet, Gpa, ProviderDescriptor, Result, Scalar,
    TargetDescriptor, VmiError,
};

pub trait MemoryAccess: Send + Sync {
    fn read_into(&self, address: Gpa, buffer: &mut [u8]) -> Result<()>;

    fn read_into_cancellable(
        &self,
        address: Gpa,
        buffer: &mut [u8],
        cancellation: &CancellationToken,
    ) -> Result<()> {
        cancellation.check("memory read")?;
        self.read_into(address, buffer)?;
        cancellation.check("memory read")
    }
}

pub trait MemoryWriteAccess: Send + Sync {
    fn write(&self, address: Gpa, data: &[u8]) -> Result<()>;
}

pub trait CpuAccess: Send + Sync {
    fn read_register(&self, vcpu: u32, register: &str) -> Result<u64>;
    fn write_register(&self, vcpu: u32, register: &str, value: u64) -> Result<()>;
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum ExecutionState {
    Running,
    Paused,
    Shutdown,
    Unknown,
}

pub trait ControlAccess: Send + Sync {
    fn execution_state(&self) -> Result<ExecutionState>;
    fn pause(&self) -> Result<()>;
    fn resume(&self) -> Result<()>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VmiEvent {
    pub kind: String,
    pub vcpu: Option<u32>,
    pub address: Option<Gpa>,
}

pub trait EventAccess: Send + Sync {
    fn next_event(&self, timeout: Duration) -> Result<Option<VmiEvent>>;

    fn next_event_cancellable(
        &self,
        timeout: Duration,
        cancellation: &CancellationToken,
    ) -> Result<Option<VmiEvent>> {
        cancellation.check("event wait")?;
        let event = self.next_event(timeout)?;
        cancellation.check("event wait")?;
        Ok(event)
    }
}

pub trait ViewAccess: Send + Sync {
    fn active_view(&self) -> Result<u16>;
    fn switch_view(&self, view: u16) -> Result<()>;
}

pub trait AcquisitionAccess: Send + Sync {
    fn save_physical_range(&self, path: &Path, start: Gpa, length: u64) -> Result<()>;
    fn save_snapshot(&self, path: &Path) -> Result<()>;

    fn save_physical_range_cancellable(
        &self,
        path: &Path,
        start: Gpa,
        length: u64,
        cancellation: &CancellationToken,
    ) -> Result<()> {
        cancellation.check("physical range acquisition")?;
        self.save_physical_range(path, start, length)?;
        cancellation.check("physical range acquisition")
    }

    fn save_snapshot_cancellable(
        &self,
        path: &Path,
        cancellation: &CancellationToken,
    ) -> Result<()> {
        cancellation.check("snapshot acquisition")?;
        self.save_snapshot(path)?;
        cancellation.check("snapshot acquisition")
    }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum LifecycleEvent {
    Reconnected { generation: u64 },
    Rebooted { generation: u64 },
    MemoryTopologyChanged { generation: u64 },
    Destroyed { generation: u64 },
}

impl LifecycleEvent {
    #[must_use]
    pub const fn generation(self) -> u64 {
        match self {
            Self::Reconnected { generation }
            | Self::Rebooted { generation }
            | Self::MemoryTopologyChanged { generation }
            | Self::Destroyed { generation } => generation,
        }
    }
}

pub trait TargetLifecycle: Send + Sync {
    /// Returns the monotonically increasing identity generation for this session.
    fn generation(&self) -> u64;

    /// Waits for the next lifecycle notification, returning `None` on timeout.
    fn next_lifecycle_event(&self, timeout: Duration) -> Result<Option<LifecycleEvent>>;
}

/// A cheaply cloned, thread-safe signal for cooperative operation cancellation.
///
/// Driver methods check cancellation at operation boundaries. Implementations that
/// override a cancellable method may additionally check it inside long-running loops.
#[derive(Clone, Debug, Default)]
pub struct CancellationToken {
    cancelled: Arc<AtomicBool>,
}

impl CancellationToken {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    pub fn check(&self, operation: &'static str) -> Result<()> {
        if self.is_cancelled() {
            Err(VmiError::Cancelled { operation })
        } else {
            Ok(())
        }
    }
}

pub trait Session: Send + Sync {
    fn provider(&self) -> &ProviderDescriptor;
    fn target(&self) -> &TargetDescriptor;
    fn capabilities(&self) -> CapabilitySet;

    fn memory(&self) -> Result<&dyn MemoryAccess> {
        Err(missing_capability(
            &self.provider().id,
            Capability::MemoryRead,
        )?)
    }

    fn memory_write(&self) -> Result<&dyn MemoryWriteAccess> {
        Err(missing_capability(
            &self.provider().id,
            Capability::MemoryWrite,
        )?)
    }

    fn cpu(&self) -> Result<&dyn CpuAccess> {
        Err(missing_capability(
            &self.provider().id,
            Capability::RegisterRead,
        )?)
    }

    fn control(&self) -> Result<&dyn ControlAccess> {
        Err(missing_capability(
            &self.provider().id,
            Capability::Control,
        )?)
    }

    fn events(&self) -> Result<&dyn EventAccess> {
        Err(missing_capability(&self.provider().id, Capability::Events)?)
    }

    fn views(&self) -> Result<&dyn ViewAccess> {
        Err(missing_capability(
            &self.provider().id,
            Capability::MemoryView,
        )?)
    }

    fn acquisition(&self) -> Result<&dyn AcquisitionAccess> {
        Err(missing_capability(
            &self.provider().id,
            Capability::Acquisition,
        )?)
    }

    fn lifecycle(&self) -> Result<&dyn TargetLifecycle> {
        Err(missing_capability(
            &self.provider().id,
            Capability::Lifecycle,
        )?)
    }
}

fn missing_capability(provider_id: &str, capability: Capability) -> Result<VmiError> {
    let mut provider = String::new();
    provider
        .try_reserve_exact(provider_id.len())
        .map_err(|error| {
            VmiError::Backend(format!(
                "failed to allocate missing-capability provider ID: {error}"
            ))
        })?;
    provider.push_str(provider_id);
    Ok(VmiError::CapabilityMissing {
        provider,
        capability,
    })
}

pub trait Connector: Send + Sync {
    fn descriptor(&self) -> &ProviderDescriptor;
    fn connect(&self, request: AttachRequest) -> Result<Box<dyn Session>>;
}

pub fn read_scalar<T: Scalar>(
    memory: &dyn MemoryAccess,
    address: Gpa,
    order: ByteOrder,
) -> Result<T> {
    let mut buffer = allocate_read_buffer(T::WIDTH)?;
    memory.read_into(address, &mut buffer)?;
    vmi_types::decode_scalar::<T>(&buffer, order).map_err(Into::into)
}

pub fn read_bytes(memory: &dyn MemoryAccess, address: Gpa, length: usize) -> Result<Vec<u8>> {
    let mut buffer = allocate_read_buffer(length)?;
    memory.read_into(address, &mut buffer)?;
    Ok(buffer)
}

fn allocate_read_buffer(length: usize) -> Result<Vec<u8>> {
    let mut buffer = Vec::new();
    buffer.try_reserve_exact(length).map_err(|error| {
        VmiError::Backend(format!(
            "failed to allocate {length}-byte read buffer: {error}"
        ))
    })?;
    buffer.resize(length, 0);
    Ok(buffer)
}

pub fn write_bytes(memory: &dyn MemoryWriteAccess, address: Gpa, data: &[u8]) -> Result<()> {
    memory.write(address, data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_capability_preserves_provider_id() {
        assert!(matches!(
            missing_capability("provider-☃", Capability::Events).unwrap(),
            VmiError::CapabilityMissing {
                provider,
                capability: Capability::Events
            } if provider == "provider-☃"
        ));
    }

    #[test]
    fn cancellation_is_shared_and_typed() {
        let token = CancellationToken::new();
        let worker = token.clone();
        assert!(!worker.is_cancelled());
        token.cancel();
        assert!(worker.is_cancelled());
        assert!(matches!(
            worker.check("test operation"),
            Err(VmiError::Cancelled {
                operation: "test operation"
            })
        ));
    }
}
