use std::{
    ffi::{OsStr, OsString},
    fmt::Write as _,
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    sync::{
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        Arc,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use vmi_artifact::SnapshotBundle;
use vmi_driver_api::{
    AcquisitionAccess, Connector, ControlAccess, CpuAccess, EventAccess, ExecutionState,
    MemoryAccess, MemoryWriteAccess, Session, VmiEvent,
};
use vmi_types::{
    AttachRequest, Capability, CapabilitySet, ConsistencyMode, Gpa, GuestArchitecture,
    ProviderDescriptor, ProviderMaturity, Result, TargetDescriptor, TargetSelector, VmiError,
};

const COMMAND_OUTPUT_CAPACITY: usize = 16 * 1024 * 1024;
const DEFAULT_COMMAND_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_COMMAND_TIMEOUT: Duration = Duration::from_secs(24 * 60 * 60);

fn read_retry(reader: &mut dyn Read, buffer: &mut [u8]) -> std::io::Result<usize> {
    loop {
        match reader.read(buffer) {
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            result => return result,
        }
    }
}

fn reserve_captured_bytes(counter: &AtomicUsize, count: usize, capacity: usize) -> bool {
    let mut current = counter.load(Ordering::Acquire);
    loop {
        let Some(next) = current.checked_add(count).filter(|next| *next <= capacity) else {
            return false;
        };
        match counter.compare_exchange_weak(current, next, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => return true,
            Err(observed) => current = observed,
        }
    }
}

pub trait XlTransport: Send + Sync {
    fn execute(&self, arguments: &[String]) -> Result<String>;
}

pub trait XenMemoryTransport: Send + Sync {
    fn read_into(&self, address: Gpa, output: &mut [u8]) -> Result<()>;
    fn write(&self, address: Gpa, data: &[u8]) -> Result<()>;
}

pub trait XenCpuTransport: Send + Sync {
    fn read_register(&self, vcpu: u32, register: &str) -> Result<u64>;
    fn supports_write(&self) -> bool {
        false
    }
    fn write_register(&self, _vcpu: u32, _register: &str, _value: u64) -> Result<()> {
        Err(VmiError::Backend(
            "Xen CPU transport does not support register writes".into(),
        ))
    }
}

pub trait XenEventTransport: Send + Sync {
    fn next_event(&self, timeout: std::time::Duration) -> Result<Option<VmiEvent>>;
}

#[derive(Clone, Debug)]
pub struct XenCtxTransport {
    executable: String,
    domain_id: u32,
    timeout: Duration,
}

impl XenCtxTransport {
    pub fn new(domain_id: u32) -> Self {
        Self {
            executable: "xenctx".into(),
            domain_id,
            timeout: DEFAULT_COMMAND_TIMEOUT,
        }
    }
    pub fn with_executable(domain_id: u32, executable: impl Into<String>) -> Self {
        Self {
            executable: executable.into(),
            domain_id,
            timeout: DEFAULT_COMMAND_TIMEOUT,
        }
    }
    pub fn with_timeout(mut self, timeout: Duration) -> Result<Self> {
        self.timeout = bounded_command_timeout(timeout, "xenctx")?;
        Ok(self)
    }
}

impl XenCpuTransport for XenCtxTransport {
    fn read_register(&self, vcpu: u32, register: &str) -> Result<u64> {
        validate_register(register)?;
        let arguments = [
            try_decimal_u32(self.domain_id, "Xen domain ID")?,
            try_decimal_u32(vcpu, "Xen vCPU index")?,
        ];
        let (status, stdout, stderr) =
            run_bounded_command(&self.executable, &arguments, self.timeout, "xenctx")?;
        if !status.success() {
            return Err(VmiError::Backend(format!(
                "xenctx failed with {}: {}",
                status,
                String::from_utf8_lossy(&stderr).trim()
            )));
        }
        let text = String::from_utf8(stdout)
            .map_err(|error| VmiError::Backend(format!("xenctx output is not UTF-8: {error}")))?;
        parse_xenctx_register(&text, register)
    }
}

fn try_decimal_u32(value: u32, description: &str) -> Result<String> {
    let mut output = String::new();
    output
        .try_reserve_exact(10)
        .map_err(|error| VmiError::Backend(format!("failed to allocate {description}: {error}")))?;
    write!(output, "{value}")
        .map_err(|error| VmiError::Backend(format!("failed to format {description}: {error}")))?;
    Ok(output)
}

#[derive(Clone, Debug)]
pub struct ProcessTransport {
    executable: String,
    timeout: Duration,
}

impl Default for ProcessTransport {
    fn default() -> Self {
        Self {
            executable: "xl".into(),
            timeout: DEFAULT_COMMAND_TIMEOUT,
        }
    }
}

impl ProcessTransport {
    pub fn new(executable: impl Into<String>) -> Self {
        Self {
            executable: executable.into(),
            timeout: DEFAULT_COMMAND_TIMEOUT,
        }
    }
    pub fn with_timeout(mut self, timeout: Duration) -> Result<Self> {
        self.timeout = bounded_command_timeout(timeout, "xl")?;
        Ok(self)
    }
}

fn bounded_command_timeout(timeout: Duration, command_name: &str) -> Result<Duration> {
    if timeout.is_zero() {
        return Err(VmiError::Backend(format!(
            "{command_name} timeout must be non-zero"
        )));
    }
    Ok(timeout.min(MAX_COMMAND_TIMEOUT))
}

impl XlTransport for ProcessTransport {
    fn execute(&self, arguments: &[String]) -> Result<String> {
        let (status, stdout, stderr) =
            run_bounded_command(&self.executable, arguments, self.timeout, "xl")?;
        if !status.success() {
            return Err(VmiError::Backend(format!(
                "xl failed with {}: {}",
                status,
                String::from_utf8_lossy(&stderr).trim()
            )));
        }
        String::from_utf8(stdout)
            .map_err(|error| VmiError::Backend(format!("xl output is not UTF-8: {error}")))
    }
}

#[cfg(test)]
fn validate_command_output_lengths(stdout: usize, stderr: usize, command: &str) -> Result<()> {
    validate_command_output_lengths_with_limit(stdout, stderr, command, COMMAND_OUTPUT_CAPACITY)
}

fn validate_command_output_lengths_with_limit(
    stdout: usize,
    stderr: usize,
    command: &str,
    capacity: usize,
) -> Result<()> {
    let total = stdout
        .checked_add(stderr)
        .ok_or_else(|| VmiError::Backend(format!("{command} output length overflow")))?;
    if total > capacity {
        return Err(VmiError::Backend(format!(
            "{command} output exceeds {capacity} bytes"
        )));
    }
    Ok(())
}

fn run_bounded_command(
    executable: &str,
    arguments: &[String],
    timeout: Duration,
    command_name: &str,
) -> Result<(ExitStatus, Vec<u8>, Vec<u8>)> {
    run_bounded_command_with_limit(
        executable,
        arguments,
        timeout,
        command_name,
        COMMAND_OUTPUT_CAPACITY,
    )
}

fn run_bounded_command_with_limit(
    executable: &str,
    arguments: &[String],
    timeout: Duration,
    command_name: &str,
    output_capacity: usize,
) -> Result<(ExitStatus, Vec<u8>, Vec<u8>)> {
    let timeout = bounded_command_timeout(timeout, command_name)?;
    let stdout_name = try_thread_name(command_name, "stdout")?;
    let stderr_name = try_thread_name(command_name, "stderr")?;
    output_capacity.checked_add(1).ok_or_else(|| {
        VmiError::Backend(format!(
            "{command_name} output capacity is too large for bounded capture"
        ))
    })?;
    let mut child = Command::new(executable)
        .args(arguments)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| VmiError::Backend(format!("failed to execute {executable}: {error}")))?;
    let Some(stdout) = child.stdout.take() else {
        terminate_and_reap(&mut child);
        return Err(VmiError::Backend(format!(
            "failed to capture {command_name} stdout"
        )));
    };
    let Some(stderr) = child.stderr.take() else {
        terminate_and_reap(&mut child);
        return Err(VmiError::Backend(format!(
            "failed to capture {command_name} stderr"
        )));
    };
    let output_exceeded = Arc::new(AtomicBool::new(false));
    let captured = Arc::new(AtomicUsize::new(0));
    let read_pipe = move |mut pipe: Box<dyn Read + Send>,
                          exceeded: Arc<AtomicBool>,
                          captured: Arc<AtomicUsize>| {
        let mut bytes = Vec::new();
        let mut chunk = [0u8; 8192];
        loop {
            let count = read_retry(pipe.as_mut(), &mut chunk)?;
            if count == 0 {
                break;
            }
            if !reserve_captured_bytes(&captured, count, output_capacity) {
                exceeded.store(true, Ordering::Release);
                break;
            }
            bytes
                .try_reserve(count)
                .map_err(|error| std::io::Error::other(error.to_string()))?;
            let captured = chunk
                .get(..count)
                .ok_or_else(|| std::io::Error::other("command read exceeded its buffer"))?;
            bytes.extend_from_slice(captured);
        }
        Ok::<_, std::io::Error>(bytes)
    };
    let stdout_exceeded = Arc::clone(&output_exceeded);
    let stderr_exceeded = Arc::clone(&output_exceeded);
    let stdout_captured = Arc::clone(&captured);
    let stderr_captured = Arc::clone(&captured);
    let stdout_reader = thread::Builder::new()
        .name(stdout_name)
        .spawn(move || read_pipe(Box::new(stdout), stdout_exceeded, stdout_captured))
        .map_err(|error| {
            terminate_and_reap(&mut child);
            VmiError::Backend(format!(
                "failed to start {command_name} stdout reader: {error}"
            ))
        })?;
    let stderr_reader = thread::Builder::new()
        .name(stderr_name)
        .spawn(move || read_pipe(Box::new(stderr), stderr_exceeded, stderr_captured))
        .map_err(|error| {
            terminate_and_reap(&mut child);
            VmiError::Backend(format!(
                "failed to start {command_name} stderr reader: {error}"
            ))
        })?;
    let deadline = Instant::now().checked_add(timeout);
    let status = loop {
        let status = child.try_wait().map_err(|error| {
            terminate_and_reap(&mut child);
            VmiError::Backend(format!("failed to wait for {command_name}: {error}"))
        })?;
        if let Some(status) = status {
            break status;
        }
        if output_exceeded.load(Ordering::Acquire) {
            terminate_and_reap(&mut child);
            return Err(VmiError::Backend(format!(
                "{command_name} output exceeds {output_capacity} bytes"
            )));
        }
        if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            terminate_and_reap(&mut child);
            return Err(VmiError::Timeout {
                operation: "Xen command",
            });
        }
        thread::sleep(Duration::from_millis(5));
    };
    let stdout = stdout_reader
        .join()
        .map_err(|_| VmiError::Backend(format!("{command_name} stdout reader panicked")))?
        .map_err(|error| {
            VmiError::Backend(format!("failed to read {command_name} stdout: {error}"))
        })?;
    let stderr = stderr_reader
        .join()
        .map_err(|_| VmiError::Backend(format!("{command_name} stderr reader panicked")))?
        .map_err(|error| {
            VmiError::Backend(format!("failed to read {command_name} stderr: {error}"))
        })?;
    if output_exceeded.load(Ordering::Acquire) {
        return Err(VmiError::Backend(format!(
            "{command_name} output exceeds {output_capacity} bytes"
        )));
    }
    validate_command_output_lengths_with_limit(
        stdout.len(),
        stderr.len(),
        command_name,
        output_capacity,
    )?;
    Ok((status, stdout, stderr))
}

fn try_thread_name(command_name: &str, stream_name: &str) -> Result<String> {
    let capacity = command_name
        .len()
        .checked_add(stream_name.len())
        .and_then(|length| length.checked_add(1))
        .ok_or_else(|| VmiError::Backend("command reader thread name length overflow".into()))?;
    let mut name = String::new();
    name.try_reserve_exact(capacity).map_err(|error| {
        VmiError::Backend(format!(
            "failed to allocate command reader thread name: {error}"
        ))
    })?;
    name.push_str(command_name);
    name.push('-');
    name.push_str(stream_name);
    Ok(name)
}

fn terminate_and_reap(child: &mut Child) {
    if child.kill().is_ok() {
        let _ = child.wait();
    }
}

#[derive(Clone)]
pub struct XenConnector {
    descriptor: Arc<ProviderDescriptor>,
    target: Arc<TargetDescriptor>,
    transport: Arc<dyn XlTransport>,
    memory: Option<Arc<dyn XenMemoryTransport>>,
    cpu: Option<Arc<dyn XenCpuTransport>>,
    events: Option<Arc<dyn XenEventTransport>>,
}

impl XenConnector {
    pub fn new(domain: impl Into<String>, architecture: GuestArchitecture) -> Self {
        Self::with_transport(domain, architecture, Arc::new(ProcessTransport::default()))
    }

    pub fn with_transport(
        domain: impl Into<String>,
        architecture: GuestArchitecture,
        transport: Arc<dyn XlTransport>,
    ) -> Self {
        let domain = domain.into();
        let capabilities = CapabilitySet::from_caps([Capability::Control, Capability::Acquisition]);
        Self {
            descriptor: Arc::new(ProviderDescriptor::new(
                "xen-xl",
                "Xen xl",
                ProviderMaturity::Preview,
                capabilities,
            )),
            target: Arc::new(TargetDescriptor::new(
                domain.clone(),
                Some(domain),
                architecture,
                ConsistencyMode::LiveBestEffort,
            )),
            transport,
            memory: None,
            cpu: None,
            events: None,
        }
    }

    pub fn with_memory_transport(mut self, memory: Arc<dyn XenMemoryTransport>) -> Self {
        Arc::make_mut(&mut self.descriptor)
            .capabilities
            .insert_capability(Capability::MemoryRead);
        Arc::make_mut(&mut self.descriptor)
            .capabilities
            .insert_capability(Capability::MemoryWrite);
        self.memory = Some(memory);
        self
    }

    pub fn with_cpu_transport(mut self, cpu: Arc<dyn XenCpuTransport>) -> Self {
        Arc::make_mut(&mut self.descriptor)
            .capabilities
            .insert_capability(Capability::RegisterRead);
        if cpu.supports_write() {
            Arc::make_mut(&mut self.descriptor)
                .capabilities
                .insert_capability(Capability::RegisterWrite);
        }
        self.cpu = Some(cpu);
        self
    }

    pub fn with_xenctx(mut self, domain_id: u32) -> Self {
        Arc::make_mut(&mut self.descriptor)
            .capabilities
            .insert_capability(Capability::RegisterRead);
        self.cpu = Some(Arc::new(XenCtxTransport::new(domain_id)));
        self
    }

    pub fn with_event_transport(mut self, events: Arc<dyn XenEventTransport>) -> Self {
        Arc::make_mut(&mut self.descriptor)
            .capabilities
            .insert_capability(Capability::Events);
        self.events = Some(events);
        self
    }

    #[cfg(unix)]
    pub fn with_xenctrl(
        domain: impl Into<String>,
        domain_id: u32,
        architecture: GuestArchitecture,
    ) -> Result<Self> {
        let memory = Arc::new(XenCtrlMemory::open(domain_id)?);
        Ok(Self::new(domain, architecture).with_memory_transport(memory))
    }
}

impl Connector for XenConnector {
    fn descriptor(&self) -> &ProviderDescriptor {
        &self.descriptor
    }

    fn connect(&self, request: AttachRequest) -> Result<Box<dyn Session>> {
        validate_target_name(&self.target.id, "Xen domain")?;
        let missing = request
            .required_capabilities
            .difference_of(self.descriptor.capabilities);
        if !missing.is_empty() {
            return Err(VmiError::AttachRejected {
                provider: try_owned_text(&self.descriptor.id, "Xen provider ID")?,
                missing,
            });
        }
        if let TargetSelector::Named(expected) = request.selector {
            if expected != self.target.id {
                return Err(VmiError::Backend(format!(
                    "Xen domain {expected} does not match {}",
                    self.target.id
                )));
            }
        }
        let session = XenSession {
            descriptor: self.descriptor.clone(),
            target: self.target.clone(),
            transport: Arc::clone(&self.transport),
            memory: self.memory.clone(),
            cpu: self.cpu.clone(),
            events: self.events.clone(),
        };
        session.execution_state()?;
        Ok(Box::new(session))
    }
}

fn validate_target_name(target: &str, description: &str) -> Result<()> {
    if target.is_empty() || target.starts_with('-') || target.chars().any(char::is_control) {
        return Err(VmiError::Backend(format!(
            "invalid {description} target name"
        )));
    }
    Ok(())
}

fn try_owned_text(value: &str, description: &str) -> Result<String> {
    let mut owned = String::new();
    owned
        .try_reserve_exact(value.len())
        .map_err(|error| VmiError::Backend(format!("failed to allocate {description}: {error}")))?;
    owned.push_str(value);
    Ok(owned)
}

struct XenSession {
    descriptor: Arc<ProviderDescriptor>,
    target: Arc<TargetDescriptor>,
    transport: Arc<dyn XlTransport>,
    memory: Option<Arc<dyn XenMemoryTransport>>,
    cpu: Option<Arc<dyn XenCpuTransport>>,
    events: Option<Arc<dyn XenEventTransport>>,
}

impl XenSession {
    fn execute(&self, arguments: &[&str]) -> Result<String> {
        let mut owned = Vec::new();
        owned.try_reserve_exact(arguments.len()).map_err(|error| {
            VmiError::Backend(format!("failed to allocate xl arguments: {error}"))
        })?;
        for argument in arguments {
            let mut owned_argument = String::new();
            owned_argument
                .try_reserve_exact(argument.len())
                .map_err(|error| {
                    VmiError::Backend(format!("failed to allocate xl argument: {error}"))
                })?;
            owned_argument.push_str(argument);
            owned.push(owned_argument);
        }
        self.transport.execute(&owned)
    }

    fn dump_core(&self, path: &Path) -> Result<()> {
        ensure_output_absent(path, "Xen core")?;
        let path = path
            .to_str()
            .ok_or_else(|| VmiError::Backend("Xen core path is not UTF-8".into()))?;
        self.execute(&["dump-core", &self.target.id, path])?;
        Ok(())
    }
}

fn ensure_output_absent(path: &Path, description: &str) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(_) => Err(VmiError::Backend(format!(
            "refusing to replace existing {description} output {}",
            path.display()
        ))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(VmiError::Backend(format!(
            "failed to inspect {description} output {}: {error}",
            path.display()
        ))),
    }
}

impl ControlAccess for XenSession {
    fn execution_state(&self) -> Result<ExecutionState> {
        let output = self.execute(&["list", &self.target.id])?;
        parse_state(&output, &self.target.id)
    }

    fn pause(&self) -> Result<()> {
        self.execute(&["pause", &self.target.id])?;
        Ok(())
    }

    fn resume(&self) -> Result<()> {
        self.execute(&["unpause", &self.target.id])?;
        Ok(())
    }
}

impl AcquisitionAccess for XenSession {
    fn save_physical_range(&self, path: &Path, start: Gpa, length: u64) -> Result<()> {
        if length == 0 {
            return Err(VmiError::Backend(
                "Xen acquisition length must be non-zero".into(),
            ));
        }
        start
            .raw()
            .checked_add(length)
            .ok_or_else(|| VmiError::Backend("Xen acquisition physical range overflows".into()))?;
        let length = usize::try_from(length)
            .map_err(|_| VmiError::Backend("requested Xen range is too large".into()))?;
        let temporary = temporary_core_path(path)?;
        self.dump_core(&temporary)?;
        let result = (|| {
            let bundle = SnapshotBundle::xen_core_file(&temporary)?;
            let mut bytes = Vec::new();
            bytes.try_reserve_exact(length).map_err(|error| {
                VmiError::Backend(format!(
                    "failed to allocate {length}-byte Xen acquisition buffer: {error}"
                ))
            })?;
            bytes.resize(length, 0);
            bundle.read_into(start, &mut bytes)?;
            publish_range(path, &bytes)
        })();
        finish_temporary_core(&temporary, result)
    }

    fn save_snapshot(&self, path: &Path) -> Result<()> {
        self.dump_core(path)
    }
}

fn publish_range(path: &Path, bytes: &[u8]) -> Result<()> {
    static NEXT_ID: AtomicU64 = AtomicU64::new(0);
    if path.exists() {
        return Err(VmiError::Backend(format!(
            "refusing to replace existing acquisition output {}",
            path.display()
        )));
    }
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let name = path
        .file_name()
        .ok_or_else(|| VmiError::Backend("acquisition path has no file name".into()))?;
    let sequence = next_sequence(&NEXT_ID, "Xen range publication")?;
    let suffix = try_publication_suffix(std::process::id(), sequence)?;
    let temporary_name = try_native_name(name, &suffix, "Xen publication")?;
    let temporary = try_child_path(parent, &temporary_name, "Xen publication")?;
    let result = (|| {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|error| {
                VmiError::Backend(format!("failed to create acquisition output: {error}"))
            })?;
        file.write_all(bytes)
            .and_then(|_| file.sync_all())
            .map_err(|error| {
                VmiError::Backend(format!("failed to write acquisition output: {error}"))
            })?;
        drop(file);
        fs::hard_link(&temporary, path).map_err(|error| {
            VmiError::Backend(format!("failed to publish {}: {error}", path.display()))
        })?;
        fs::remove_file(&temporary).map_err(|error| {
            VmiError::Backend(format!(
                "published {} but failed to remove temporary output: {error}",
                path.display()
            ))
        })
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

impl MemoryAccess for XenSession {
    fn read_into(&self, address: Gpa, buffer: &mut [u8]) -> Result<()> {
        require_transport(
            self.memory.as_ref(),
            &self.descriptor.id,
            Capability::MemoryRead,
        )?
        .read_into(address, buffer)
    }
}

impl MemoryWriteAccess for XenSession {
    fn write(&self, address: Gpa, data: &[u8]) -> Result<()> {
        require_transport(
            self.memory.as_ref(),
            &self.descriptor.id,
            Capability::MemoryWrite,
        )?
        .write(address, data)
    }
}

impl CpuAccess for XenSession {
    fn read_register(&self, vcpu: u32, register: &str) -> Result<u64> {
        validate_register(register)?;
        require_transport(
            self.cpu.as_ref(),
            &self.descriptor.id,
            Capability::RegisterRead,
        )?
        .read_register(vcpu, register)
    }
    fn write_register(&self, _vcpu: u32, _register: &str, _value: u64) -> Result<()> {
        validate_register(_register)?;
        let cpu = require_transport(
            self.cpu.as_ref(),
            &self.descriptor.id,
            Capability::RegisterWrite,
        )?;
        if !cpu.supports_write() {
            return Err(capability_missing(
                &self.descriptor.id,
                Capability::RegisterWrite,
            )?);
        }
        cpu.write_register(_vcpu, _register, _value)
    }
}

impl EventAccess for XenSession {
    fn next_event(&self, timeout: std::time::Duration) -> Result<Option<VmiEvent>> {
        require_transport(
            self.events.as_ref(),
            &self.descriptor.id,
            Capability::Events,
        )?
        .next_event(timeout)
    }
}

impl Session for XenSession {
    fn provider(&self) -> &ProviderDescriptor {
        &self.descriptor
    }
    fn target(&self) -> &TargetDescriptor {
        &self.target
    }
    fn capabilities(&self) -> CapabilitySet {
        self.descriptor.capabilities
    }
    fn control(&self) -> Result<&dyn ControlAccess> {
        Ok(self)
    }
    fn memory(&self) -> Result<&dyn MemoryAccess> {
        if self.memory.is_some() {
            Ok(self)
        } else {
            Err(capability_missing(
                &self.descriptor.id,
                Capability::MemoryRead,
            )?)
        }
    }
    fn memory_write(&self) -> Result<&dyn MemoryWriteAccess> {
        if self.memory.is_some() {
            Ok(self)
        } else {
            Err(capability_missing(
                &self.descriptor.id,
                Capability::MemoryWrite,
            )?)
        }
    }
    fn cpu(&self) -> Result<&dyn CpuAccess> {
        if self.cpu.is_some() {
            Ok(self)
        } else {
            Err(capability_missing(
                &self.descriptor.id,
                Capability::RegisterRead,
            )?)
        }
    }
    fn events(&self) -> Result<&dyn EventAccess> {
        if self.events.is_some() {
            Ok(self)
        } else {
            Err(capability_missing(&self.descriptor.id, Capability::Events)?)
        }
    }
    fn acquisition(&self) -> Result<&dyn AcquisitionAccess> {
        Ok(self)
    }
}

fn capability_missing(provider_id: &str, capability: Capability) -> Result<VmiError> {
    Ok(VmiError::CapabilityMissing {
        provider: try_owned_text(provider_id, "Xen provider ID")?,
        capability,
    })
}

fn require_transport<'a, T: ?Sized>(
    transport: Option<&'a Arc<T>>,
    provider_id: &str,
    capability: Capability,
) -> Result<&'a Arc<T>> {
    match transport {
        Some(transport) => Ok(transport),
        None => Err(capability_missing(provider_id, capability)?),
    }
}

fn validate_register(register: &str) -> Result<()> {
    if register.is_empty()
        || !register
            .chars()
            .all(|character| character.is_ascii_alphanumeric())
    {
        return Err(VmiError::Backend("invalid Xen register name".into()));
    }
    Ok(())
}

fn parse_xenctx_register(output: &str, register: &str) -> Result<u64> {
    for line in output.lines() {
        let mut previous = None;
        for field in line.split_whitespace() {
            if previous
                .is_some_and(|name: &str| name.trim_end_matches(':').eq_ignore_ascii_case(register))
            {
                return u64::from_str_radix(strip_hex_prefix(field), 16).map_err(|error| {
                    VmiError::Backend(format!("invalid xenctx register value: {error}"))
                });
            }
            previous = Some(field);
        }
    }
    Err(VmiError::Backend(format!(
        "xenctx output lacks register {register}"
    )))
}

fn strip_hex_prefix(value: &str) -> &str {
    value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
        .unwrap_or(value)
}

#[cfg(unix)]
mod xenctrl {
    use super::*;
    use libloading::Library;
    use std::{ffi::c_void, ptr, sync::Mutex};

    type Open = unsafe extern "C" fn(*mut c_void, *mut c_void, u32) -> *mut c_void;
    type Close = unsafe extern "C" fn(*mut c_void) -> i32;
    type MapPages =
        unsafe extern "C" fn(*mut c_void, u32, i32, *const libc::c_ulong, i32) -> *mut c_void;

    pub struct XenCtrlMemory {
        library: Library,
        interface: Mutex<*mut c_void>,
        domain_id: u32,
    }

    // SAFETY: every access to the xenctrl handle is serialized by `interface`,
    // and `library` remains owned for the entire handle lifetime.
    unsafe impl Send for XenCtrlMemory {}
    // SAFETY: shared operations lock `interface`; no native call can use the
    // handle concurrently or outlive the loaded xenctrl library.
    unsafe impl Sync for XenCtrlMemory {}

    impl XenCtrlMemory {
        pub fn open(domain_id: u32) -> Result<Self> {
            let library = ["libxenctrl.so.4.19", "libxenctrl.so.4.18", "libxenctrl.so"]
                .into_iter()
                // SAFETY: loading a vendor library runs its initializers; the
                // accepted names are fixed xenctrl ABI candidates, not input.
                .find_map(|name| unsafe { Library::new(name).ok() })
                .ok_or_else(|| VmiError::Backend("unable to load libxenctrl".into()))?;
            // SAFETY: `Open` matches the published xc_interface_open ABI and
            // the symbol cannot outlive the retained `library`.
            let open: libloading::Symbol<Open> = unsafe { library.get(b"xc_interface_open\0") }
                .map_err(|error| {
                    VmiError::Backend(format!("missing xc_interface_open: {error}"))
                })?;
            // SAFETY: null logger/Dombuilder arguments and flag zero are valid
            // inputs to xc_interface_open; its result is null-checked below.
            let interface = unsafe { open(ptr::null_mut(), ptr::null_mut(), 0) };
            if interface.is_null() {
                return Err(VmiError::Backend("xc_interface_open failed".into()));
            }
            Ok(Self {
                library,
                interface: Mutex::new(interface),
                domain_id,
            })
        }

        fn access(&self, address: Gpa, buffer: &mut [u8], write: bool) -> Result<()> {
            // SAFETY: `MapPages` matches xc_map_foreign_pages and the symbol is
            // used only while the retained library remains loaded.
            let map: libloading::Symbol<MapPages> = unsafe {
                self.library.get(b"xc_map_foreign_pages\0")
            }
            .map_err(|error| VmiError::Backend(format!("missing xc_map_foreign_pages: {error}")))?;
            let interface = self
                .interface
                .lock()
                .map_err(|_| VmiError::Backend("xenctrl interface lock is poisoned".into()))?;
            let mut completed = 0usize;
            while completed < buffer.len() {
                let completed_address = u64::try_from(completed).map_err(|_| {
                    VmiError::Backend("Xen transfer offset exceeds physical addressing".into())
                })?;
                let current = address
                    .raw()
                    .checked_add(completed_address)
                    .ok_or_else(|| VmiError::Backend("Xen physical address overflow".into()))?;
                let page_offset = usize::try_from(current & 0xfff).map_err(|_| {
                    VmiError::Backend("Xen page offset exceeds host pointer width".into())
                })?;
                let length = (4096 - page_offset).min(buffer.len() - completed);
                let frame = libc::c_ulong::try_from(current >> 12).map_err(|_| {
                    VmiError::Backend("Xen guest frame number exceeds host c_ulong".into())
                })?;
                let protection = if write {
                    libc::PROT_READ | libc::PROT_WRITE
                } else {
                    libc::PROT_READ
                };
                // SAFETY: the locked interface is live, `frame` points to one
                // valid GFN value, and the requested page count is exactly one.
                let mapping = unsafe { map(*interface, self.domain_id, protection, &frame, 1) };
                if mapping == libc::MAP_FAILED || mapping.is_null() {
                    return Err(VmiError::Backend(format!(
                        "xc_map_foreign_pages failed for GFN {frame:#x}"
                    )));
                }
                // SAFETY: mapping success was checked; `page_offset + length`
                // stays within the single 4 KiB page, and the Rust buffer slice
                // contains at least `length` bytes. The mapping is then unmapped.
                unsafe {
                    let pointer = mapping.cast::<u8>().add(page_offset);
                    if write {
                        ptr::copy_nonoverlapping(buffer[completed..].as_ptr(), pointer, length);
                    } else {
                        ptr::copy_nonoverlapping(pointer, buffer[completed..].as_mut_ptr(), length);
                    }
                    if libc::munmap(mapping, 4096) != 0 {
                        return Err(VmiError::Backend(format!(
                            "munmap failed after Xen GFN {frame:#x} access"
                        )));
                    }
                }
                completed += length;
            }
            Ok(())
        }
    }

    impl XenMemoryTransport for XenCtrlMemory {
        fn read_into(&self, address: Gpa, output: &mut [u8]) -> Result<()> {
            self.access(address, output, false)
        }
        fn write(&self, address: Gpa, data: &[u8]) -> Result<()> {
            let mut bytes = Vec::new();
            bytes.try_reserve_exact(data.len()).map_err(|error| {
                VmiError::Backend(format!("failed to allocate Xen write buffer: {error}"))
            })?;
            bytes.extend_from_slice(data);
            self.access(address, &mut bytes, true)
        }
    }

    impl Drop for XenCtrlMemory {
        fn drop(&mut self) {
            if let Ok(interface) = self.interface.get_mut() {
                // SAFETY: `Close` matches xc_interface_close and cannot outlive
                // the still-owned library.
                if let Ok(close) = unsafe { self.library.get::<Close>(b"xc_interface_close\0") } {
                    // SAFETY: exclusive `&mut self` access guarantees no native
                    // operation is using the live interface during destruction.
                    unsafe { close(*interface) };
                }
            }
        }
    }
}

#[cfg(unix)]
pub use xenctrl::XenCtrlMemory;

fn parse_state(output: &str, domain: &str) -> Result<ExecutionState> {
    let line = output
        .lines()
        .skip(1)
        .find(|line| line.split_whitespace().next() == Some(domain))
        .ok_or_else(|| VmiError::Backend(format!("xl output does not contain domain {domain}")))?;
    let state = line
        .split_whitespace()
        .nth(4)
        .ok_or_else(|| VmiError::Backend("xl list row lacks state".into()))?;
    Ok(if state.contains('r') {
        ExecutionState::Running
    } else if state.contains('p') {
        ExecutionState::Paused
    } else if state.contains('s') || state.contains('d') || state.contains('c') {
        ExecutionState::Shutdown
    } else {
        ExecutionState::Unknown
    })
}

fn temporary_core_path(destination: &Path) -> Result<PathBuf> {
    static NEXT_TEMPORARY_CORE: AtomicU64 = AtomicU64::new(0);
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let sequence = next_sequence(&NEXT_TEMPORARY_CORE, "Xen temporary core")?;
    let mut extension = String::new();
    extension.try_reserve_exact(73).map_err(|error| {
        VmiError::Backend(format!("failed to allocate Xen core extension: {error}"))
    })?;
    write!(
        extension,
        "vmi-{}-{nonce}-{sequence}.core",
        std::process::id()
    )
    .map_err(|error| VmiError::Backend(format!("failed to format Xen core extension: {error}")))?;
    try_with_extension(destination, &extension, "Xen temporary core")
}

fn try_publication_suffix(process_id: u32, sequence: u64) -> Result<String> {
    let mut suffix = String::new();
    suffix.try_reserve_exact(42).map_err(|error| {
        VmiError::Backend(format!(
            "failed to allocate Xen publication suffix: {error}"
        ))
    })?;
    write!(suffix, ".vmi-range-{process_id}-{sequence}").map_err(|error| {
        VmiError::Backend(format!("failed to format Xen publication suffix: {error}"))
    })?;
    Ok(suffix)
}

fn try_native_name(name: &OsStr, suffix: &str, description: &str) -> Result<OsString> {
    let capacity = name
        .len()
        .checked_add(suffix.len())
        .and_then(|length| length.checked_add(1))
        .ok_or_else(|| VmiError::Backend(format!("{description} filename length overflow")))?;
    let mut output = OsString::new();
    output.try_reserve_exact(capacity).map_err(|error| {
        VmiError::Backend(format!(
            "failed to allocate {description} filename: {error}"
        ))
    })?;
    output.push(".");
    output.push(name);
    output.push(suffix);
    Ok(output)
}

fn try_child_path(parent: &Path, name: &OsStr, description: &str) -> Result<PathBuf> {
    let capacity = parent
        .as_os_str()
        .len()
        .checked_add(name.len())
        .and_then(|length| length.checked_add(1))
        .ok_or_else(|| VmiError::Backend(format!("{description} path length overflow")))?;
    let mut output = PathBuf::new();
    output.try_reserve_exact(capacity).map_err(|error| {
        VmiError::Backend(format!("failed to allocate {description} path: {error}"))
    })?;
    output.push(parent);
    output.push(name);
    Ok(output)
}

fn try_with_extension(destination: &Path, extension: &str, description: &str) -> Result<PathBuf> {
    let capacity = destination
        .as_os_str()
        .len()
        .checked_add(extension.len())
        .and_then(|length| length.checked_add(1))
        .ok_or_else(|| VmiError::Backend(format!("{description} path length overflow")))?;
    let mut output = PathBuf::new();
    output.try_reserve_exact(capacity).map_err(|error| {
        VmiError::Backend(format!("failed to allocate {description} path: {error}"))
    })?;
    output.push(destination);
    output.set_extension(extension);
    Ok(output)
}

fn next_sequence(counter: &AtomicU64, description: &str) -> Result<u64> {
    let mut current = counter.load(Ordering::Relaxed);
    loop {
        let next = current.checked_add(1).ok_or_else(|| {
            VmiError::Backend(format!("{description} identifier space exhausted"))
        })?;
        match counter.compare_exchange_weak(current, next, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return Ok(current),
            Err(observed) => current = observed,
        }
    }
}

fn finish_temporary_core(path: &Path, result: Result<()>) -> Result<()> {
    match (result, fs::remove_file(path)) {
        (Err(error), _) => Err(error),
        (Ok(()), Ok(())) => Ok(()),
        (Ok(()), Err(error)) => Err(VmiError::Backend(format!(
            "failed to remove temporary Xen core {}: {error}",
            path.display()
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_reader_thread_names_are_exact() {
        assert_eq!(try_thread_name("xl", "stdout").unwrap(), "xl-stdout");
        assert_eq!(try_thread_name("명령", "stderr").unwrap(), "명령-stderr");
        assert_eq!(try_decimal_u32(u32::MAX, "test").unwrap(), "4294967295");
    }

    #[cfg(unix)]
    #[test]
    fn publication_names_preserve_non_utf8_bytes() {
        use std::os::unix::ffi::{OsStrExt, OsStringExt};

        let source = OsString::from_vec(vec![b'g', 0xff, b't']);
        let name = try_native_name(&source, ".suffix", "test").unwrap();
        assert_eq!(name.as_bytes(), b".g\xfft.suffix");
    }

    #[test]
    fn cloned_connector_capability_builders_are_isolated() {
        let original = XenConnector::new("guest", GuestArchitecture::Amd64);
        let with_cpu = original.clone().with_xenctx(7);
        assert!(!original
            .descriptor()
            .capabilities
            .contains_capability(Capability::RegisterRead));
        assert!(with_cpu
            .descriptor()
            .capabilities
            .contains_capability(Capability::RegisterRead));
    }
    use proptest::prelude::*;
    use std::{collections::VecDeque, sync::Mutex, thread};

    #[test]
    fn concurrent_temporary_core_paths_are_unique() {
        let destination = Path::new("guest.core");
        let handles: Vec<_> = (0..64)
            .map(|_| thread::spawn(move || temporary_core_path(destination).unwrap()))
            .collect();
        let paths: std::collections::BTreeSet<_> = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect();
        assert_eq!(paths.len(), 64);
        assert!(paths.iter().all(|path| path != destination));
    }

    #[test]
    fn command_output_limits_fail_closed() {
        assert!(validate_command_output_lengths(1, 1, "test").is_ok());
        assert!(validate_command_output_lengths(COMMAND_OUTPUT_CAPACITY, 1, "test").is_err());
        assert!(validate_command_output_lengths(usize::MAX, 1, "test").is_err());
    }

    #[test]
    fn captured_byte_reservation_is_bounded_and_non_mutating_on_failure() {
        let counter = AtomicUsize::new(0);
        assert!(reserve_captured_bytes(&counter, 6, 10));
        assert!(!reserve_captured_bytes(&counter, 5, 10));
        assert!(!reserve_captured_bytes(&counter, usize::MAX, 10));
        assert_eq!(counter.load(Ordering::Relaxed), 6);
    }

    #[test]
    fn temporary_sequence_never_wraps() {
        let counter = AtomicU64::new(u64::MAX - 1);
        assert_eq!(next_sequence(&counter, "test").unwrap(), u64::MAX - 1);
        assert!(next_sequence(&counter, "test").is_err());
        assert_eq!(counter.load(Ordering::Relaxed), u64::MAX);
    }

    #[test]
    fn target_names_cannot_be_empty_options_or_control_bearing() {
        assert!(validate_target_name("guest domain", "test").is_ok());
        assert!(validate_target_name("", "test").is_err());
        assert!(validate_target_name("--help", "test").is_err());
        assert!(validate_target_name("guest\nshutdown", "test").is_err());
    }

    struct FakeTransport {
        replies: Mutex<VecDeque<String>>,
        commands: Mutex<Vec<Vec<String>>>,
    }
    impl XlTransport for FakeTransport {
        fn execute(&self, arguments: &[String]) -> Result<String> {
            self.commands.lock().unwrap().push(arguments.to_vec());
            self.replies
                .lock()
                .unwrap()
                .pop_front()
                .ok_or_else(|| VmiError::Backend("missing fake reply".into()))
        }
    }

    struct FakeMemory(Mutex<Vec<u8>>);
    impl XenMemoryTransport for FakeMemory {
        fn read_into(&self, address: Gpa, output: &mut [u8]) -> Result<()> {
            let start = address.raw() as usize;
            output.copy_from_slice(&self.0.lock().unwrap()[start..start + output.len()]);
            Ok(())
        }
        fn write(&self, address: Gpa, data: &[u8]) -> Result<()> {
            let start = address.raw() as usize;
            self.0.lock().unwrap()[start..start + data.len()].copy_from_slice(data);
            Ok(())
        }
    }

    struct FakeCpu;
    impl XenCpuTransport for FakeCpu {
        fn read_register(&self, vcpu: u32, register: &str) -> Result<u64> {
            assert_eq!((vcpu, register), (2, "rip"));
            Ok(0xffff_8000_1234_5678)
        }
    }

    struct WritableCpu(Mutex<u64>);
    impl XenCpuTransport for WritableCpu {
        fn read_register(&self, _vcpu: u32, _register: &str) -> Result<u64> {
            Ok(*self.0.lock().unwrap())
        }
        fn supports_write(&self) -> bool {
            true
        }
        fn write_register(&self, _vcpu: u32, _register: &str, value: u64) -> Result<()> {
            *self.0.lock().unwrap() = value;
            Ok(())
        }
    }

    #[test]
    fn writable_cpu_transport_enables_register_write_capability() {
        let transport = Arc::new(FakeTransport {
            replies: Mutex::new(VecDeque::from([
                "Name ID Mem VCPUs State Time(s)\nguest 7 1024 3 r----- 1.0\n".into(),
            ])),
            commands: Mutex::new(Vec::new()),
        });
        let connector = XenConnector::with_transport("guest", GuestArchitecture::Amd64, transport)
            .with_cpu_transport(Arc::new(WritableCpu(Mutex::new(1))));
        let session = connector.connect(AttachRequest::default()).unwrap();
        assert!(session
            .capabilities()
            .contains_capability(Capability::RegisterWrite));
        session.cpu().unwrap().write_register(0, "rax", 9).unwrap();
        assert_eq!(session.cpu().unwrap().read_register(0, "rax").unwrap(), 9);
        assert!(session
            .cpu()
            .unwrap()
            .write_register(0, "rax;bad", 0)
            .is_err());
    }

    struct FakeEvents {
        timeouts: Mutex<Vec<std::time::Duration>>,
        events: Mutex<VecDeque<VmiEvent>>,
    }
    impl XenEventTransport for FakeEvents {
        fn next_event(&self, timeout: std::time::Duration) -> Result<Option<VmiEvent>> {
            self.timeouts.lock().unwrap().push(timeout);
            Ok(self.events.lock().unwrap().pop_front())
        }
    }

    #[test]
    fn optional_vm_event_transport_enables_typed_events() {
        let transport = Arc::new(FakeTransport {
            replies: Mutex::new(VecDeque::from([
                "Name ID Mem VCPUs State Time(s)\nguest 7 1024 3 r----- 1.0\n".into(),
            ])),
            commands: Mutex::new(Vec::new()),
        });
        let events = Arc::new(FakeEvents {
            timeouts: Mutex::new(Vec::new()),
            events: Mutex::new(VecDeque::from([VmiEvent {
                kind: "memory-access".into(),
                vcpu: Some(2),
                address: Some(Gpa::new(0x1234)),
            }])),
        });
        let connector = XenConnector::with_transport("guest", GuestArchitecture::Amd64, transport)
            .with_event_transport(events.clone());
        let session = connector.connect(AttachRequest::default()).unwrap();
        let timeout = std::time::Duration::from_millis(25);
        let event = session
            .events()
            .unwrap()
            .next_event(timeout)
            .unwrap()
            .unwrap();
        assert_eq!(
            (event.kind.as_str(), event.vcpu, event.address),
            ("memory-access", Some(2), Some(Gpa::new(0x1234)))
        );
        assert_eq!(events.timeouts.lock().unwrap().as_slice(), &[timeout]);
        assert!(session
            .events()
            .unwrap()
            .next_event(timeout)
            .unwrap()
            .is_none());
    }

    #[test]
    fn optional_xenctx_transport_enables_register_reads() {
        let transport = Arc::new(FakeTransport {
            replies: Mutex::new(VecDeque::from([
                "Name ID Mem VCPUs State Time(s)\nguest 7 1024 3 r----- 1.0\n".into(),
            ])),
            commands: Mutex::new(Vec::new()),
        });
        let connector = XenConnector::with_transport("guest", GuestArchitecture::Amd64, transport)
            .with_cpu_transport(Arc::new(FakeCpu));
        let session = connector.connect(AttachRequest::default()).unwrap();
        assert_eq!(
            session.cpu().unwrap().read_register(2, "rip").unwrap(),
            0xffff_8000_1234_5678
        );
        assert!(session.cpu().unwrap().write_register(2, "rip", 0).is_err());
    }

    #[test]
    fn parses_xenctx_register_rows() {
        let output = "rip: ffff800012345678 flags: 00000246 rsp: ffff800000001000\n\
                      rax: 0000000000000001\trcx: 0000000000000002\trdx: 0000000000000003\n\
                       cr0: 0000000080050033 cr3: 00000000001aa000\n";
        assert_eq!(
            parse_xenctx_register(output, "RIP").unwrap(),
            0xffff_8000_1234_5678
        );
        assert_eq!(parse_xenctx_register(output, "cr3").unwrap(), 0x1aa000);
        assert_eq!(parse_xenctx_register("RAX: 0X2A", "rax").unwrap(), 0x2a);
        assert!(parse_xenctx_register(output, "r15").is_err());
        assert!(validate_register("rip;shutdown").is_err());
    }

    #[test]
    fn optional_xenctrl_transport_enables_memory_capabilities() {
        let transport = Arc::new(FakeTransport {
            replies: Mutex::new(VecDeque::from([
                "Name ID Mem VCPUs State Time(s)\nguest 7 1024 2 r----- 1.0\n".into(),
            ])),
            commands: Mutex::new(Vec::new()),
        });
        let memory = Arc::new(FakeMemory(Mutex::new(vec![0; 0x2000])));
        let connector = XenConnector::with_transport("guest", GuestArchitecture::Amd64, transport)
            .with_memory_transport(memory);
        let session = connector.connect(AttachRequest::default()).unwrap();
        assert!(session
            .capabilities()
            .contains_capability(Capability::MemoryRead));
        session
            .memory_write()
            .unwrap()
            .write(Gpa::new(0xfff), &[1, 2])
            .unwrap();
        let mut bytes = [0; 2];
        session
            .memory()
            .unwrap()
            .read_into(Gpa::new(0xfff), &mut bytes)
            .unwrap();
        assert_eq!(bytes, [1, 2]);
    }

    #[test]
    fn attaches_controls_and_requests_core_dump() {
        let transport = Arc::new(FakeTransport {
            replies: Mutex::new(VecDeque::from([
                "Name ID Mem VCPUs State Time(s)\nguest 7 1024 2 r----- 1.0\n".into(),
                String::new(),
                String::new(),
                String::new(),
            ])),
            commands: Mutex::new(Vec::new()),
        });
        let connector =
            XenConnector::with_transport("guest", GuestArchitecture::Amd64, transport.clone());
        let session = connector.connect(AttachRequest::default()).unwrap();
        session.control().unwrap().pause().unwrap();
        session.control().unwrap().resume().unwrap();
        session
            .acquisition()
            .unwrap()
            .save_snapshot(Path::new("guest.core"))
            .unwrap();
        assert!(session
            .acquisition()
            .unwrap()
            .save_physical_range(Path::new("empty.bin"), Gpa::new(0), 0)
            .is_err());
        assert!(session
            .acquisition()
            .unwrap()
            .save_physical_range(Path::new("overflow.bin"), Gpa::new(u64::MAX), 1)
            .is_err());
        let commands = transport.commands.lock().unwrap();
        assert_eq!(commands[0], ["list", "guest"]);
        assert_eq!(commands[1], ["pause", "guest"]);
        assert_eq!(commands[2], ["unpause", "guest"]);
        assert_eq!(commands[3], ["dump-core", "guest", "guest.core"]);
    }

    #[test]
    fn snapshot_preflight_rejects_existing_files_and_directories() {
        let directory = std::env::temp_dir().join(format!(
            "vmi-xen-preflight-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        fs::create_dir_all(&directory).unwrap();
        let file = directory.join("existing.core");
        fs::write(&file, b"preserve").unwrap();
        assert!(ensure_output_absent(&file, "Xen core").is_err());
        assert!(ensure_output_absent(&directory, "Xen core").is_err());
        assert!(ensure_output_absent(&directory.join("new.core"), "Xen core").is_ok());
        assert_eq!(fs::read(&file).unwrap(), b"preserve");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn parses_xl_states_and_rejects_missing_domains() {
        assert_eq!(
            parse_state("Name ID Mem VCPUs State Time(s)\ng 1 1 1 -p---- 0\n", "g").unwrap(),
            ExecutionState::Paused
        );
        assert!(parse_state("Name ID Mem VCPUs State Time(s)\n", "g").is_err());
    }

    proptest! {
        #[test]
    fn private_xen_parsers_fail_without_panicking(
            output in any::<String>(),
            register in any::<String>(),
            domain in any::<String>(),
        ) {
            let _ = validate_register(&register);
            let _ = parse_xenctx_register(&output, &register);
            let _ = parse_state(&output, &domain);
        }
    }

    #[test]
    fn range_publication_is_atomic_and_never_clobbers() {
        let directory = std::env::temp_dir().join(format!(
            "vmi-xen-publish-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        fs::create_dir_all(&directory).unwrap();
        let destination = directory.join("range.bin");
        publish_range(&destination, &[1, 2, 3]).unwrap();
        assert_eq!(fs::read(&destination).unwrap(), [1, 2, 3]);
        assert!(publish_range(&destination, &[9]).is_err());
        assert_eq!(fs::read(&destination).unwrap(), [1, 2, 3]);
        let raced = directory.join("raced.bin");
        let handles = (0u8..8)
            .map(|value| {
                let raced = raced.clone();
                thread::spawn(move || publish_range(&raced, &[value]).map(|_| value))
            })
            .collect::<Vec<_>>();
        let winners = handles
            .into_iter()
            .filter_map(|handle| handle.join().unwrap().ok())
            .collect::<Vec<_>>();
        assert_eq!(winners.len(), 1);
        assert_eq!(fs::read(&raced).unwrap(), winners);
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 2);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn process_runner_captures_output_and_enforces_deadline() {
        #[cfg(windows)]
        let (executable, success, delayed): (&str, Vec<String>, Vec<String>) = (
            "powershell.exe",
            ["-NoProfile", "-Command", "[Console]::Out.Write('ready')"]
                .into_iter()
                .map(str::to_owned)
                .collect(),
            ["-NoProfile", "-Command", "Start-Sleep -Seconds 5"]
                .into_iter()
                .map(str::to_owned)
                .collect(),
        );
        #[cfg(unix)]
        let (executable, success, delayed): (&str, Vec<String>, Vec<String>) = (
            "sh",
            ["-c", "printf ready"]
                .into_iter()
                .map(str::to_owned)
                .collect(),
            ["-c", "sleep 5"].into_iter().map(str::to_owned).collect(),
        );
        #[cfg(windows)]
        let excessive = [
            "-NoProfile",
            "-Command",
            "[Console]::Out.Write('123456789'); [Console]::Out.Flush(); Start-Sleep -Seconds 5",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>();
        #[cfg(unix)]
        let excessive = ["-c", "printf 123456789; sleep 5"]
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        #[cfg(windows)]
        let quick_excessive = [
            "-NoProfile",
            "-Command",
            "[Console]::Out.Write('123456789')",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>();
        #[cfg(unix)]
        let quick_excessive = ["-c", "printf 123456789"]
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        #[cfg(windows)]
        let split_excessive = [
            "-NoProfile",
            "-Command",
            "[Console]::Out.Write('123'); [Console]::Out.Flush(); [Console]::Error.Write('456'); [Console]::Error.Flush(); Start-Sleep -Seconds 5",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>();
        #[cfg(unix)]
        let split_excessive = ["-c", "printf 123; printf 456 >&2; sleep 5"]
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>();

        let (status, stdout, stderr) =
            run_bounded_command(executable, &success, Duration::from_secs(2), "test").unwrap();
        assert!(status.success());
        assert_eq!(stdout, b"ready");
        assert!(stderr.is_empty());
        assert!(matches!(
            run_bounded_command(executable, &delayed, Duration::from_millis(50), "test"),
            Err(VmiError::Timeout {
                operation: "Xen command"
            })
        ));
        assert!(ProcessTransport::new(executable)
            .with_timeout(Duration::ZERO)
            .is_err());
        assert!(XenCtxTransport::with_executable(1, executable)
            .with_timeout(Duration::ZERO)
            .is_err());
        assert_eq!(
            ProcessTransport::new(executable)
                .with_timeout(Duration::MAX)
                .unwrap()
                .timeout,
            MAX_COMMAND_TIMEOUT
        );
        assert_eq!(
            XenCtxTransport::with_executable(1, executable)
                .with_timeout(Duration::MAX)
                .unwrap()
                .timeout,
            MAX_COMMAND_TIMEOUT
        );
        let excessive_start = Instant::now();
        assert!(run_bounded_command_with_limit(
            executable,
            &excessive,
            Duration::from_secs(2),
            "test",
            4
        )
        .unwrap_err()
        .to_string()
        .contains("exceeds 4 bytes"));
        assert!(excessive_start.elapsed() < Duration::from_secs(1));
        for _ in 0..20 {
            assert!(run_bounded_command_with_limit(
                executable,
                &quick_excessive,
                Duration::from_secs(2),
                "test",
                4
            )
            .unwrap_err()
            .to_string()
            .contains("exceeds 4 bytes"));
        }
        assert!(run_bounded_command_with_limit(
            executable,
            &split_excessive,
            Duration::from_secs(2),
            "test",
            4
        )
        .unwrap_err()
        .to_string()
        .contains("exceeds 4 bytes"));
        assert!(run_bounded_command_with_limit(
            executable,
            &success,
            Duration::from_secs(2),
            "test",
            usize::MAX
        )
        .unwrap_err()
        .to_string()
        .contains("capacity is too large"));
    }
}
