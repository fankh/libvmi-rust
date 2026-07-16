use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    ffi::{OsStr, OsString},
    fmt::Write as _,
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

use vmi_driver_api::{
    AcquisitionAccess, Connector, ControlAccess, CpuAccess, EventAccess, ExecutionState,
    LifecycleEvent, MemoryAccess, MemoryWriteAccess, Session, TargetLifecycle, ViewAccess,
    VmiEvent,
};
use vmi_types::{
    AttachRequest, Capability, CapabilitySet, ConsistencyMode, Gpa, GuestArchitecture,
    ProviderDescriptor, ProviderMaturity, Result, TargetDescriptor, TargetSelector, VmiError,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SparseSegment {
    pub start: Gpa,
    pub bytes: Vec<u8>,
}

impl SparseSegment {
    pub fn new(start: impl Into<Gpa>, bytes: impl Into<Vec<u8>>) -> Self {
        Self {
            start: start.into(),
            bytes: bytes.into(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct FakeConnector {
    descriptor: Arc<ProviderDescriptor>,
    target: Arc<TargetDescriptor>,
    capabilities: CapabilitySet,
    segments: Vec<SparseSegment>,
    registers: BTreeMap<(u32, String), u64>,
    events: VecDeque<VmiEvent>,
    read_faults: BTreeSet<u64>,
    write_faults: BTreeSet<u64>,
}

impl FakeConnector {
    pub fn new() -> Self {
        let capabilities = CapabilitySet::from_caps([Capability::MemoryRead]);
        Self {
            descriptor: Arc::new(ProviderDescriptor::new(
                "fake-read-only",
                "Fake Read-Only Provider",
                ProviderMaturity::Supported,
                capabilities,
            )),
            target: Arc::new(TargetDescriptor::new(
                "fake-target",
                Some("Fake Target"),
                GuestArchitecture::Amd64,
                ConsistencyMode::ImmutableSnapshot,
            )),
            capabilities,
            segments: Vec::new(),
            registers: BTreeMap::new(),
            events: VecDeque::new(),
            read_faults: BTreeSet::new(),
            write_faults: BTreeSet::new(),
        }
    }

    pub fn with_capabilities(mut self, capabilities: CapabilitySet) -> Self {
        self.capabilities = capabilities;
        Arc::make_mut(&mut self.descriptor).capabilities = capabilities;
        self
    }

    pub fn with_segment(mut self, start: impl Into<Gpa>, bytes: impl Into<Vec<u8>>) -> Self {
        self.segments.push(SparseSegment::new(start, bytes));
        self
    }

    pub fn with_register(mut self, vcpu: u32, register: impl Into<String>, value: u64) -> Self {
        self.registers.insert((vcpu, register.into()), value);
        self
    }

    pub fn with_event(mut self, event: VmiEvent) -> Self {
        self.events.push_back(event);
        self
    }

    /// Injects a deterministic failure when a read reaches `address`.
    pub fn with_read_fault(mut self, address: impl Into<Gpa>) -> Self {
        self.read_faults.insert(address.into().raw());
        self
    }

    /// Injects a deterministic failure when a write reaches `address`.
    pub fn with_write_fault(mut self, address: impl Into<Gpa>) -> Self {
        self.write_faults.insert(address.into().raw());
        self
    }

    pub fn descriptor(&self) -> &ProviderDescriptor {
        &self.descriptor
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

impl Default for FakeConnector {
    fn default() -> Self {
        Self::new()
    }
}

impl Connector for FakeConnector {
    fn descriptor(&self) -> &ProviderDescriptor {
        &self.descriptor
    }

    fn connect(&self, request: AttachRequest) -> Result<Box<dyn Session>> {
        validate_segments(&self.segments)?;
        let missing = request
            .required_capabilities
            .difference_of(self.capabilities);
        if !missing.is_empty() {
            return Err(VmiError::AttachRejected {
                provider: try_owned_text(&self.descriptor.id, "fake provider ID")?,
                missing,
            });
        }

        if let TargetSelector::Named(ref expected) = request.selector {
            if expected != &self.target.id
                && expected != self.target.display_name.as_deref().unwrap_or("")
            {
                return Err(VmiError::Backend(format!(
                    "target {expected} not found for provider {}",
                    self.descriptor.id
                )));
            }
        }

        Ok(Box::new(FakeSession {
            provider: self.descriptor.clone(),
            target: self.target.clone(),
            capabilities: self.capabilities,
            segments: Mutex::new(self.segments.clone()),
            registers: Mutex::new(self.registers.clone()),
            execution_state: Mutex::new(ExecutionState::Running),
            events: Mutex::new(self.events.clone()),
            active_view: Mutex::new(0),
            read_faults: self.read_faults.clone(),
            write_faults: self.write_faults.clone(),
            generation: AtomicU64::new(1),
            lifecycle_events: Mutex::new(VecDeque::new()),
        }))
    }
}

#[derive(Debug)]
pub struct FakeSession {
    provider: Arc<ProviderDescriptor>,
    target: Arc<TargetDescriptor>,
    capabilities: CapabilitySet,
    segments: Mutex<Vec<SparseSegment>>,
    registers: Mutex<BTreeMap<(u32, String), u64>>,
    execution_state: Mutex<ExecutionState>,
    events: Mutex<VecDeque<VmiEvent>>,
    active_view: Mutex<u16>,
    read_faults: BTreeSet<u64>,
    write_faults: BTreeSet<u64>,
    generation: AtomicU64,
    lifecycle_events: Mutex<VecDeque<LifecycleEvent>>,
}

impl FakeSession {
    fn read_sparse_into(&self, address: Gpa, buffer: &mut [u8]) -> Result<()> {
        let segments = self
            .segments
            .lock()
            .map_err(|error| VmiError::Backend(format!("fake memory lock failed: {error}")))?;
        let buffer_len = buffer.len();
        for (index, slot) in buffer.iter_mut().enumerate() {
            let absolute = address
                .raw()
                .checked_add(u64::try_from(index).map_err(|_| VmiError::ReadFailed {
                    address: address.raw(),
                    length: buffer_len,
                })?)
                .ok_or(VmiError::ReadFailed {
                    address: address.raw(),
                    length: buffer_len,
                })?;
            let mut found = false;

            if self.read_faults.contains(&absolute) {
                return Err(VmiError::Backend(format!(
                    "injected fake read fault at {absolute:#x}"
                )));
            }

            for segment in segments.iter() {
                let start = segment.start.raw();
                let offset = absolute.checked_sub(start).and_then(|offset| {
                    usize::try_from(offset)
                        .ok()
                        .filter(|offset| *offset < segment.bytes.len())
                });
                if let Some(byte) = offset.and_then(|offset| segment.bytes.get(offset)) {
                    *slot = *byte;
                    found = true;
                    break;
                }
            }

            if !found {
                return Err(VmiError::ReadFailed {
                    address: absolute,
                    length: 1,
                });
            }
        }

        Ok(())
    }
}

impl CpuAccess for FakeSession {
    fn read_register(&self, vcpu: u32, register: &str) -> Result<u64> {
        if !self
            .capabilities
            .contains_capability(Capability::RegisterRead)
        {
            return Err(self.missing(Capability::RegisterRead)?);
        }
        self.registers
            .lock()
            .map_err(fake_lock)?
            .get(&(vcpu, register.to_owned()))
            .copied()
            .ok_or_else(|| {
                VmiError::Backend(format!(
                    "fake register {register} for vCPU {vcpu} is not defined"
                ))
            })
    }

    fn write_register(&self, vcpu: u32, register: &str, value: u64) -> Result<()> {
        if !self
            .capabilities
            .contains_capability(Capability::RegisterWrite)
        {
            return Err(self.missing(Capability::RegisterWrite)?);
        }
        self.registers
            .lock()
            .map_err(fake_lock)?
            .insert((vcpu, register.to_owned()), value);
        Ok(())
    }
}

impl ControlAccess for FakeSession {
    fn execution_state(&self) -> Result<ExecutionState> {
        self.require(Capability::Control)?;
        self.execution_state
            .lock()
            .map(|state| *state)
            .map_err(fake_lock)
    }

    fn pause(&self) -> Result<()> {
        self.require(Capability::Control)?;
        *self.execution_state.lock().map_err(fake_lock)? = ExecutionState::Paused;
        Ok(())
    }

    fn resume(&self) -> Result<()> {
        self.require(Capability::Control)?;
        *self.execution_state.lock().map_err(fake_lock)? = ExecutionState::Running;
        Ok(())
    }
}

impl EventAccess for FakeSession {
    fn next_event(&self, _timeout: Duration) -> Result<Option<VmiEvent>> {
        self.require(Capability::Events)?;
        Ok(self.events.lock().map_err(fake_lock)?.pop_front())
    }
}

impl ViewAccess for FakeSession {
    fn active_view(&self) -> Result<u16> {
        self.require(Capability::MemoryView)?;
        self.active_view.lock().map(|view| *view).map_err(fake_lock)
    }

    fn switch_view(&self, view: u16) -> Result<()> {
        self.require(Capability::MemoryView)?;
        *self.active_view.lock().map_err(fake_lock)? = view;
        Ok(())
    }
}

impl AcquisitionAccess for FakeSession {
    fn save_physical_range(&self, path: &Path, start: Gpa, length: u64) -> Result<()> {
        self.require(Capability::Acquisition)?;
        let length = usize::try_from(length).map_err(|_| VmiError::ReadFailed {
            address: start.raw(),
            length: usize::MAX,
        })?;
        let mut bytes = Vec::new();
        bytes.try_reserve_exact(length).map_err(|error| {
            VmiError::Backend(format!("failed to allocate fake acquisition: {error}"))
        })?;
        bytes.resize(length, 0);
        self.read_sparse_into(start, &mut bytes)?;
        publish_atomically(path, &bytes)
    }

    fn save_snapshot(&self, path: &Path) -> Result<()> {
        self.require(Capability::Acquisition)?;
        let segments = self.segments.lock().map_err(fake_lock)?;
        let mut bytes = Vec::new();
        for segment in segments.iter() {
            let segment_length = u64::try_from(segment.bytes.len()).map_err(|_| {
                VmiError::Backend("fake snapshot segment length exceeds the file format".into())
            })?;
            let record_length = 16usize
                .checked_add(segment.bytes.len())
                .ok_or_else(|| VmiError::Backend("fake snapshot record length overflow".into()))?;
            bytes.try_reserve(record_length).map_err(|error| {
                VmiError::Backend(format!("failed to allocate fake snapshot: {error}"))
            })?;
            bytes.extend_from_slice(&segment.start.raw().to_le_bytes());
            bytes.extend_from_slice(&segment_length.to_le_bytes());
            bytes.extend_from_slice(&segment.bytes);
        }
        publish_atomically(path, &bytes)
    }
}

fn fake_lock(error: impl std::fmt::Display) -> VmiError {
    VmiError::Backend(format!("fake provider lock failed: {error}"))
}

fn fake_io(error: impl std::fmt::Display) -> VmiError {
    VmiError::Backend(format!("fake acquisition I/O failed: {error}"))
}

fn publish_atomically(path: &Path, bytes: &[u8]) -> Result<()> {
    static NEXT_TEMP_ID: AtomicU64 = AtomicU64::new(0);

    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let name = path
        .file_name()
        .ok_or_else(|| VmiError::Backend("fake acquisition destination has no file name".into()))?;
    let sequence = next_temp_id(&NEXT_TEMP_ID)?;
    let mut suffix = String::new();
    suffix.try_reserve_exact(40).map_err(|error| {
        VmiError::Backend(format!(
            "failed to allocate fake acquisition suffix: {error}"
        ))
    })?;
    write!(suffix, ".vmi-tmp-{}-{sequence}", std::process::id()).map_err(|error| {
        VmiError::Backend(format!("failed to format fake acquisition suffix: {error}"))
    })?;
    let temp_name = try_native_temp_name(name, &suffix)?;
    let temp_path = try_child_path(parent, &temp_name)?;
    let result = (|| {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)
            .map_err(fake_io)?;
        file.write_all(bytes).map_err(fake_io)?;
        file.sync_all().map_err(fake_io)?;
        drop(file);
        fs::hard_link(&temp_path, path).map_err(fake_io)?;
        fs::remove_file(&temp_path).map_err(fake_io)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }
    result
}

fn next_temp_id(counter: &AtomicU64) -> Result<u64> {
    let mut current = counter.load(Ordering::Relaxed);
    loop {
        let next = current.checked_add(1).ok_or_else(|| {
            VmiError::Backend("fake acquisition identifier space exhausted".into())
        })?;
        match counter.compare_exchange_weak(current, next, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return Ok(current),
            Err(observed) => current = observed,
        }
    }
}

fn try_native_temp_name(name: &OsStr, suffix: &str) -> Result<OsString> {
    let capacity = name
        .len()
        .checked_add(suffix.len())
        .and_then(|length| length.checked_add(1))
        .ok_or_else(|| VmiError::Backend("fake acquisition filename length overflow".into()))?;
    let mut output = OsString::new();
    output.try_reserve_exact(capacity).map_err(|error| {
        VmiError::Backend(format!(
            "failed to allocate fake acquisition filename: {error}"
        ))
    })?;
    output.push(".");
    output.push(name);
    output.push(suffix);
    Ok(output)
}

fn try_child_path(parent: &Path, name: &OsStr) -> Result<PathBuf> {
    let capacity = parent
        .as_os_str()
        .len()
        .checked_add(name.len())
        .and_then(|length| length.checked_add(1))
        .ok_or_else(|| VmiError::Backend("fake acquisition path length overflow".into()))?;
    let mut output = PathBuf::new();
    output.try_reserve_exact(capacity).map_err(|error| {
        VmiError::Backend(format!("failed to allocate fake acquisition path: {error}"))
    })?;
    output.push(parent);
    output.push(name);
    Ok(output)
}

impl MemoryWriteAccess for FakeSession {
    fn write(&self, address: Gpa, data: &[u8]) -> Result<()> {
        self.require(Capability::MemoryWrite)?;
        let mut segments = self
            .segments
            .lock()
            .map_err(|error| VmiError::Backend(format!("fake memory lock failed: {error}")))?;
        for (index, value) in data.iter().copied().enumerate() {
            let absolute = address
                .raw()
                .checked_add(u64::try_from(index).map_err(|_| {
                    VmiError::Backend(
                        "fake sparse write index does not fit the address model".into(),
                    )
                })?)
                .ok_or(VmiError::ReadFailed {
                    address: address.raw(),
                    length: data.len(),
                })?;
            let mut found = false;
            if self.write_faults.contains(&absolute) {
                return Err(VmiError::Backend(format!(
                    "injected fake write fault at {absolute:#x}"
                )));
            }
            for segment in segments.iter_mut() {
                let start = segment.start.raw();
                let offset = absolute.checked_sub(start).and_then(|offset| {
                    usize::try_from(offset)
                        .ok()
                        .filter(|offset| *offset < segment.bytes.len())
                });
                if let Some(byte) = offset.and_then(|offset| segment.bytes.get_mut(offset)) {
                    *byte = value;
                    found = true;
                    break;
                }
            }
            if !found {
                return Err(VmiError::ReadFailed {
                    address: absolute,
                    length: 1,
                });
            }
        }
        Ok(())
    }
}

fn validate_segments(segments: &[SparseSegment]) -> Result<()> {
    let mut ranges = segments
        .iter()
        .filter(|segment| !segment.bytes.is_empty())
        .map(|segment| {
            let start = u128::from(segment.start.raw());
            (
                start,
                start.saturating_add(u128::try_from(segment.bytes.len()).unwrap_or(u128::MAX)),
            )
        })
        .collect::<Vec<_>>();
    let address_space_end = u128::from(u64::MAX).saturating_add(1);
    if let Some((start, _)) = ranges.iter().find(|(_, end)| *end > address_space_end) {
        return Err(VmiError::Backend(format!(
            "fake sparse segment at {start:#x} exceeds the physical address space"
        )));
    }
    ranges.sort_unstable();
    for pair in ranges.windows(2) {
        let [previous, current] = pair else {
            return Err(VmiError::Backend(
                "fake sparse range window invariant failed".into(),
            ));
        };
        if current.0 < previous.1 {
            return Err(VmiError::Backend(format!(
                "fake sparse segments overlap at {:#x}",
                current.0
            )));
        }
    }
    Ok(())
}

impl MemoryAccess for FakeSession {
    fn read_into(&self, address: Gpa, buffer: &mut [u8]) -> Result<()> {
        self.require(Capability::MemoryRead)?;
        self.read_sparse_into(address, buffer)
    }
}

impl TargetLifecycle for FakeSession {
    fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    fn next_lifecycle_event(&self, _timeout: Duration) -> Result<Option<LifecycleEvent>> {
        self.require(Capability::Lifecycle)?;
        self.lifecycle_events
            .lock()
            .map_err(|error| VmiError::Backend(format!("fake lifecycle lock failed: {error}")))?
            .pop_front()
            .map_or(Ok(None), |event| {
                self.generation
                    .fetch_max(event.generation(), Ordering::AcqRel);
                Ok(Some(event))
            })
    }
}

impl Session for FakeSession {
    fn provider(&self) -> &ProviderDescriptor {
        &self.provider
    }

    fn target(&self) -> &TargetDescriptor {
        &self.target
    }

    fn capabilities(&self) -> CapabilitySet {
        self.capabilities
    }

    fn memory(&self) -> Result<&dyn MemoryAccess> {
        self.facet(Capability::MemoryRead, self)
    }

    fn memory_write(&self) -> Result<&dyn MemoryWriteAccess> {
        if self
            .capabilities
            .contains_capability(Capability::MemoryWrite)
        {
            Ok(self)
        } else {
            Err(self.missing(Capability::MemoryWrite)?)
        }
    }

    fn cpu(&self) -> Result<&dyn CpuAccess> {
        if self
            .capabilities
            .intersects(Capability::RegisterRead.bit() | Capability::RegisterWrite.bit())
        {
            Ok(self)
        } else {
            Err(self.missing(Capability::RegisterRead)?)
        }
    }

    fn control(&self) -> Result<&dyn ControlAccess> {
        self.facet(Capability::Control, self)
    }

    fn events(&self) -> Result<&dyn EventAccess> {
        self.facet(Capability::Events, self)
    }

    fn views(&self) -> Result<&dyn ViewAccess> {
        self.facet(Capability::MemoryView, self)
    }

    fn acquisition(&self) -> Result<&dyn AcquisitionAccess> {
        self.facet(Capability::Acquisition, self)
    }

    fn lifecycle(&self) -> Result<&dyn TargetLifecycle> {
        self.facet(Capability::Lifecycle, self)
    }
}

impl FakeSession {
    fn require(&self, capability: Capability) -> Result<()> {
        if self.capabilities.contains_capability(capability) {
            Ok(())
        } else {
            Err(self.missing(capability)?)
        }
    }

    fn missing(&self, capability: Capability) -> Result<VmiError> {
        Ok(VmiError::CapabilityMissing {
            provider: try_owned_text(&self.provider.id, "fake provider ID")?,
            capability,
        })
    }

    fn facet<'a, T: ?Sized>(&'a self, capability: Capability, value: &'a T) -> Result<&'a T> {
        if self.capabilities.contains_capability(capability) {
            Ok(value)
        } else {
            Err(self.missing(capability)?)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn temporary_ids_never_wrap() {
        let counter = AtomicU64::new(u64::MAX - 1);
        assert_eq!(next_temp_id(&counter).unwrap(), u64::MAX - 1);
        assert!(next_temp_id(&counter).is_err());
        assert!(next_temp_id(&counter).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn temporary_names_preserve_non_utf8_bytes() {
        use std::os::unix::ffi::{OsStrExt, OsStringExt};

        let source = OsString::from_vec(vec![b'f', 0xff]);
        let name = try_native_temp_name(&source, ".suffix").unwrap();
        assert_eq!(name.as_bytes(), b".f\xff.suffix");
    }
}
