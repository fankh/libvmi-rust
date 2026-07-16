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
    AcquisitionAccess, Connector, ControlAccess, CpuAccess, ExecutionState, MemoryAccess, Session,
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

pub trait VBoxManageTransport: Send + Sync {
    fn execute(&self, arguments: &[String]) -> Result<String>;
}

pub trait VirtualBoxMemoryTransport: Send + Sync {
    fn read_into(&self, address: Gpa, output: &mut [u8]) -> Result<()>;
}

#[derive(Clone, Debug)]
pub struct ProcessTransport {
    executable: String,
    timeout: Duration,
}

impl Default for ProcessTransport {
    fn default() -> Self {
        Self {
            executable: "VBoxManage".into(),
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
        self.timeout = bounded_command_timeout(timeout, "VBoxManage")?;
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

impl VBoxManageTransport for ProcessTransport {
    fn execute(&self, arguments: &[String]) -> Result<String> {
        let (status, stdout, stderr) =
            run_bounded_command(&self.executable, arguments, self.timeout, "VBoxManage")?;
        if !status.success() {
            return Err(VmiError::Backend(format!(
                "VBoxManage failed with {}: {}",
                status,
                String::from_utf8_lossy(&stderr).trim()
            )));
        }
        String::from_utf8(stdout)
            .map_err(|error| VmiError::Backend(format!("VBoxManage output is not UTF-8: {error}")))
    }
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
                operation: "VirtualBox command",
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

#[derive(Clone)]
pub struct VirtualBoxConnector {
    descriptor: Arc<ProviderDescriptor>,
    target: Arc<TargetDescriptor>,
    transport: Arc<dyn VBoxManageTransport>,
    memory: Option<Arc<dyn VirtualBoxMemoryTransport>>,
}

impl VirtualBoxConnector {
    pub fn new(vm: impl Into<String>, architecture: GuestArchitecture) -> Self {
        Self::with_transport(vm, architecture, Arc::new(ProcessTransport::default()))
    }

    pub fn with_transport(
        vm: impl Into<String>,
        architecture: GuestArchitecture,
        transport: Arc<dyn VBoxManageTransport>,
    ) -> Self {
        let vm = vm.into();
        let capabilities = CapabilitySet::from_caps([
            Capability::MemoryRead,
            Capability::RegisterRead,
            Capability::Control,
            Capability::Acquisition,
        ]);
        Self {
            descriptor: Arc::new(ProviderDescriptor::new(
                "virtualbox",
                "VirtualBox Live",
                ProviderMaturity::Preview,
                capabilities,
            )),
            target: Arc::new(TargetDescriptor::new(
                vm.clone(),
                Some(vm),
                architecture,
                ConsistencyMode::LiveBestEffort,
            )),
            transport,
            memory: None,
        }
    }

    pub fn with_memory_transport(mut self, memory: Arc<dyn VirtualBoxMemoryTransport>) -> Self {
        self.memory = Some(memory);
        self
    }

    pub fn with_register_write(mut self) -> Self {
        Arc::make_mut(&mut self.descriptor)
            .capabilities
            .insert_capability(Capability::RegisterWrite);
        self
    }
}

impl Connector for VirtualBoxConnector {
    fn descriptor(&self) -> &ProviderDescriptor {
        &self.descriptor
    }

    fn connect(&self, request: AttachRequest) -> Result<Box<dyn Session>> {
        validate_target_name(&self.target.id, "VirtualBox VM")?;
        let missing = request
            .required_capabilities
            .difference_of(self.descriptor.capabilities);
        if !missing.is_empty() {
            return Err(VmiError::AttachRejected {
                provider: try_owned_text(&self.descriptor.id, "VirtualBox provider ID")?,
                missing,
            });
        }
        if let TargetSelector::Named(expected) = request.selector {
            if expected != self.target.id {
                return Err(VmiError::Backend(format!(
                    "VirtualBox VM {expected} does not match {}",
                    self.target.id
                )));
            }
        }
        let session = VirtualBoxSession {
            descriptor: self.descriptor.clone(),
            target: self.target.clone(),
            transport: Arc::clone(&self.transport),
            memory: self.memory.clone(),
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

struct VirtualBoxSession {
    descriptor: Arc<ProviderDescriptor>,
    target: Arc<TargetDescriptor>,
    transport: Arc<dyn VBoxManageTransport>,
    memory: Option<Arc<dyn VirtualBoxMemoryTransport>>,
}

impl VirtualBoxSession {
    fn execute(&self, arguments: &[&str]) -> Result<String> {
        let mut owned = Vec::new();
        owned.try_reserve_exact(arguments.len()).map_err(|error| {
            VmiError::Backend(format!("failed to allocate VBoxManage arguments: {error}"))
        })?;
        for argument in arguments {
            let mut owned_argument = String::new();
            owned_argument
                .try_reserve_exact(argument.len())
                .map_err(|error| {
                    VmiError::Backend(format!("failed to allocate VBoxManage argument: {error}"))
                })?;
            owned_argument.push_str(argument);
            owned.push(owned_argument);
        }
        self.transport.execute(&owned)
    }
}

impl CpuAccess for VirtualBoxSession {
    fn read_register(&self, vcpu: u32, register: &str) -> Result<u64> {
        if register.is_empty() || !register.chars().all(|c| c.is_ascii_alphanumeric()) {
            return Err(VmiError::Backend("invalid VirtualBox register name".into()));
        }
        let cpu = try_decimal_u32(vcpu, "VirtualBox vCPU index")?;
        let output = self.execute(&[
            "debugvm",
            &self.target.id,
            "getregisters",
            "--cpu",
            &cpu,
            register,
        ])?;
        parse_register(&output, register)
    }

    fn write_register(&self, vcpu: u32, register: &str, value: u64) -> Result<()> {
        if !self
            .descriptor
            .capabilities
            .contains_capability(Capability::RegisterWrite)
        {
            return Err(VmiError::CapabilityMissing {
                provider: try_owned_text(&self.descriptor.id, "VirtualBox provider ID")?,
                capability: Capability::RegisterWrite,
            });
        }
        if register.is_empty() || !register.chars().all(|c| c.is_ascii_alphanumeric()) {
            return Err(VmiError::Backend("invalid VirtualBox register name".into()));
        }
        let cpu = try_decimal_u32(vcpu, "VirtualBox vCPU index")?;
        let assignment = try_register_assignment(register, value)?;
        self.execute(&[
            "debugvm",
            &self.target.id,
            "setregisters",
            "--cpu",
            &cpu,
            &assignment,
        ])?;
        let actual = self.read_register(vcpu, register)?;
        if actual != value {
            return Err(VmiError::Backend(format!(
                "VirtualBox register write verification failed: expected {value:#x}, got {actual:#x}"
            )));
        }
        Ok(())
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

fn try_register_assignment(register: &str, value: u64) -> Result<String> {
    let capacity = register.len().checked_add(19).ok_or_else(|| {
        VmiError::Backend("VirtualBox register assignment length overflow".into())
    })?;
    let mut assignment = String::new();
    assignment.try_reserve_exact(capacity).map_err(|error| {
        VmiError::Backend(format!(
            "failed to allocate VirtualBox register assignment: {error}"
        ))
    })?;
    write!(assignment, "{register}=0x{value:016x}").map_err(|error| {
        VmiError::Backend(format!(
            "failed to format VirtualBox register assignment: {error}"
        ))
    })?;
    Ok(assignment)
}

impl AcquisitionAccess for VirtualBoxSession {
    fn save_physical_range(&self, path: &Path, start: Gpa, length: u64) -> Result<()> {
        if length == 0 {
            return Err(VmiError::Backend(
                "VirtualBox acquisition length must be non-zero".into(),
            ));
        }
        start.raw().checked_add(length).ok_or_else(|| {
            VmiError::Backend("VirtualBox acquisition physical range overflows".into())
        })?;
        let length = usize::try_from(length)
            .map_err(|_| VmiError::Backend("requested VirtualBox range is too large".into()))?;
        let temporary = temporary_core_path(path)?;
        self.save_snapshot(&temporary)?;
        let result = (|| {
            let bundle = SnapshotBundle::elf_vmcore_file(&temporary)?;
            let mut bytes = Vec::new();
            bytes.try_reserve_exact(length).map_err(|error| {
                VmiError::Backend(format!(
                    "failed to allocate {length}-byte VirtualBox acquisition buffer: {error}"
                ))
            })?;
            bytes.resize(length, 0);
            bundle.read_into(start, &mut bytes)?;
            publish_range(path, &bytes)
        })();
        finish_temporary_core(&temporary, result)
    }

    fn save_snapshot(&self, path: &Path) -> Result<()> {
        ensure_output_absent(path, "VirtualBox core")?;
        let path = path
            .to_str()
            .ok_or_else(|| VmiError::Backend("VirtualBox core path is not UTF-8".into()))?;
        let capacity = path.len().checked_add(11).ok_or_else(|| {
            VmiError::Backend("VirtualBox filename argument length overflow".into())
        })?;
        let mut filename = String::new();
        filename.try_reserve_exact(capacity).map_err(|error| {
            VmiError::Backend(format!(
                "failed to allocate VirtualBox filename argument: {error}"
            ))
        })?;
        write!(filename, "--filename={path}").map_err(|error| {
            VmiError::Backend(format!(
                "failed to format VirtualBox filename argument: {error}"
            ))
        })?;
        self.execute(&["debugvm", &self.target.id, "dumpvmcore", &filename])?;
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
    let sequence = next_sequence(&NEXT_ID, "VirtualBox range publication")?;
    let suffix = try_publication_suffix(std::process::id(), sequence)?;
    let temporary_name = try_native_name(name, &suffix, "VirtualBox publication")?;
    let temporary = try_child_path(parent, &temporary_name, "VirtualBox publication")?;
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

impl MemoryAccess for VirtualBoxSession {
    fn read_into(&self, address: Gpa, buffer: &mut [u8]) -> Result<()> {
        if buffer.is_empty() {
            return Ok(());
        }
        if let Some(memory) = &self.memory {
            return memory.read_into(address, buffer);
        }
        let temporary_directory = std::env::temp_dir();
        let live_core = try_child_path(
            &temporary_directory,
            OsStr::new("virtualbox-live.core"),
            "VirtualBox live core",
        )?;
        let temporary = temporary_core_path(&live_core)?;
        self.save_snapshot(&temporary)?;
        let result = (|| {
            let bundle = SnapshotBundle::elf_vmcore_file(&temporary)?;
            bundle.read_into(address, buffer)
        })();
        finish_temporary_core(&temporary, result)
    }
}

impl ControlAccess for VirtualBoxSession {
    fn execution_state(&self) -> Result<ExecutionState> {
        let output = self.execute(&["showvminfo", &self.target.id, "--machinereadable"])?;
        parse_state(&output)
    }

    fn pause(&self) -> Result<()> {
        self.execute(&["controlvm", &self.target.id, "pause"])?;
        Ok(())
    }

    fn resume(&self) -> Result<()> {
        self.execute(&["controlvm", &self.target.id, "resume"])?;
        Ok(())
    }
}

impl Session for VirtualBoxSession {
    fn provider(&self) -> &ProviderDescriptor {
        &self.descriptor
    }
    fn target(&self) -> &TargetDescriptor {
        &self.target
    }
    fn capabilities(&self) -> CapabilitySet {
        self.descriptor.capabilities
    }
    fn cpu(&self) -> Result<&dyn CpuAccess> {
        Ok(self)
    }
    fn memory(&self) -> Result<&dyn MemoryAccess> {
        Ok(self)
    }
    fn control(&self) -> Result<&dyn ControlAccess> {
        Ok(self)
    }
    fn acquisition(&self) -> Result<&dyn AcquisitionAccess> {
        Ok(self)
    }
}

fn temporary_core_path(destination: &Path) -> Result<PathBuf> {
    static NEXT_TEMPORARY_CORE: AtomicU64 = AtomicU64::new(0);
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let sequence = next_sequence(&NEXT_TEMPORARY_CORE, "VirtualBox temporary core")?;
    let mut extension = String::new();
    extension.try_reserve_exact(73).map_err(|error| {
        VmiError::Backend(format!(
            "failed to allocate VirtualBox core extension: {error}"
        ))
    })?;
    write!(
        extension,
        "vmi-{}-{nonce}-{sequence}.core",
        std::process::id()
    )
    .map_err(|error| {
        VmiError::Backend(format!(
            "failed to format VirtualBox core extension: {error}"
        ))
    })?;
    try_with_extension(destination, &extension, "VirtualBox temporary core")
}

fn try_publication_suffix(process_id: u32, sequence: u64) -> Result<String> {
    let mut suffix = String::new();
    suffix.try_reserve_exact(42).map_err(|error| {
        VmiError::Backend(format!(
            "failed to allocate VirtualBox publication suffix: {error}"
        ))
    })?;
    write!(suffix, ".vmi-range-{process_id}-{sequence}").map_err(|error| {
        VmiError::Backend(format!(
            "failed to format VirtualBox publication suffix: {error}"
        ))
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
            "failed to remove temporary VirtualBox core {}: {error}",
            path.display()
        ))),
    }
}

fn parse_state(output: &str) -> Result<ExecutionState> {
    let value = output
        .lines()
        .find_map(|line| line.strip_prefix("VMState="))
        .map(|value| value.trim_matches('"'))
        .ok_or_else(|| VmiError::Backend("VBoxManage output lacks VMState".into()))?;
    Ok(match value {
        "running" => ExecutionState::Running,
        "paused" => ExecutionState::Paused,
        "poweroff" | "saved" | "aborted" => ExecutionState::Shutdown,
        _ => ExecutionState::Unknown,
    })
}

fn parse_register(output: &str, register: &str) -> Result<u64> {
    for line in output.lines() {
        let Some((name, value)) = line.split_once('=') else {
            continue;
        };
        if name.trim().eq_ignore_ascii_case(register) {
            let value = value.trim().trim_matches('"');
            return u64::from_str_radix(strip_hex_prefix(value), 16)
                .map_err(|error| VmiError::Backend(format!("invalid register value: {error}")));
        }
    }
    Err(VmiError::Backend(format!(
        "VBoxManage output lacks register {register}"
    )))
}

fn strip_hex_prefix(value: &str) -> &str {
    value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
        .unwrap_or(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_reader_thread_names_are_exact() {
        assert_eq!(
            try_thread_name("VBoxManage", "stdout").unwrap(),
            "VBoxManage-stdout"
        );
        assert_eq!(try_thread_name("명령", "stderr").unwrap(), "명령-stderr");
        assert_eq!(try_decimal_u32(u32::MAX, "test").unwrap(), "4294967295");
        assert_eq!(
            try_register_assignment("rax", u64::MAX).unwrap(),
            "rax=0xffffffffffffffff"
        );
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
        let original = VirtualBoxConnector::new("guest", GuestArchitecture::Amd64);
        let writable = original.clone().with_register_write();
        assert!(!original
            .descriptor()
            .capabilities
            .contains_capability(Capability::RegisterWrite));
        assert!(writable
            .descriptor()
            .capabilities
            .contains_capability(Capability::RegisterWrite));
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
        assert!(validate_target_name("guest vm", "test").is_ok());
        assert!(validate_target_name("", "test").is_err());
        assert!(validate_target_name("--help", "test").is_err());
        assert!(validate_target_name("guest\nshutdown", "test").is_err());
    }

    struct FakeTransport {
        replies: Mutex<VecDeque<String>>,
        commands: Mutex<Vec<Vec<String>>>,
    }
    impl VBoxManageTransport for FakeTransport {
        fn execute(&self, arguments: &[String]) -> Result<String> {
            self.commands.lock().unwrap().push(arguments.to_vec());
            self.replies
                .lock()
                .unwrap()
                .pop_front()
                .ok_or_else(|| VmiError::Backend("missing fake reply".into()))
        }
    }

    struct DumpingTransport;
    impl VBoxManageTransport for DumpingTransport {
        fn execute(&self, arguments: &[String]) -> Result<String> {
            if arguments.first().map(String::as_str) == Some("showvminfo") {
                return Ok("VMState=\"running\"\n".into());
            }
            let filename = arguments
                .iter()
                .find_map(|argument| argument.strip_prefix("--filename="))
                .ok_or_else(|| VmiError::Backend("missing dump filename".into()))?;
            let mut elf = vec![0u8; 132];
            elf[..6].copy_from_slice(b"\x7fELF\x02\x01");
            elf[32..40].copy_from_slice(&64u64.to_le_bytes());
            elf[54..56].copy_from_slice(&56u16.to_le_bytes());
            elf[56..58].copy_from_slice(&1u16.to_le_bytes());
            elf[64..68].copy_from_slice(&1u32.to_le_bytes());
            elf[72..80].copy_from_slice(&128u64.to_le_bytes());
            elf[88..96].copy_from_slice(&0x2000u64.to_le_bytes());
            elf[96..104].copy_from_slice(&4u64.to_le_bytes());
            elf[104..112].copy_from_slice(&4u64.to_le_bytes());
            elf[128..132].copy_from_slice(&[1, 2, 3, 4]);
            fs::write(filename, elf)
                .map_err(|error| VmiError::Backend(format!("fixture write failed: {error}")))?;
            Ok(String::new())
        }
    }

    struct DirectMemory;
    impl VirtualBoxMemoryTransport for DirectMemory {
        fn read_into(&self, address: Gpa, output: &mut [u8]) -> Result<()> {
            for (index, byte) in output.iter_mut().enumerate() {
                *byte = address.raw().wrapping_add(index as u64) as u8;
            }
            Ok(())
        }
    }

    #[test]
    fn direct_memory_transport_bypasses_core_acquisition() {
        let transport = Arc::new(FakeTransport {
            replies: Mutex::new(VecDeque::from(["VMState=\"running\"\n".into()])),
            commands: Mutex::new(Vec::new()),
        });
        let connector = VirtualBoxConnector::with_transport(
            "test-vm",
            GuestArchitecture::Amd64,
            transport.clone(),
        )
        .with_memory_transport(Arc::new(DirectMemory));
        let session = connector.connect(AttachRequest::default()).unwrap();
        let mut bytes = [0; 3];
        session
            .memory()
            .unwrap()
            .read_into(Gpa::new(0x10fe), &mut bytes)
            .unwrap();
        assert_eq!(bytes, [0xfe, 0xff, 0x00]);
        assert_eq!(transport.commands.lock().unwrap().len(), 1);
    }

    #[test]
    fn reads_live_memory_through_temporary_core() {
        let connector = VirtualBoxConnector::with_transport(
            "test-vm",
            GuestArchitecture::Amd64,
            Arc::new(DumpingTransport),
        );
        let session = connector.connect(AttachRequest::default()).unwrap();
        let mut bytes = [0; 2];
        session
            .memory()
            .unwrap()
            .read_into(Gpa::new(0x2001), &mut bytes)
            .unwrap();
        assert_eq!(bytes, [2, 3]);
    }

    #[test]
    fn attaches_reads_registers_and_controls_vm() {
        let transport = Arc::new(FakeTransport {
            replies: Mutex::new(VecDeque::from([
                "VMState=\"running\"\n".into(),
                "rax=0x0000000000001234\n".into(),
                String::new(),
                "rax=0x0000000000005678\n".into(),
                String::new(),
                String::new(),
                String::new(),
            ])),
            commands: Mutex::new(Vec::new()),
        });
        let connector = VirtualBoxConnector::with_transport(
            "test-vm",
            GuestArchitecture::Amd64,
            transport.clone(),
        )
        .with_register_write();
        let session = connector.connect(AttachRequest::default()).unwrap();
        assert_eq!(
            session.cpu().unwrap().read_register(0, "rax").unwrap(),
            0x1234
        );
        session
            .cpu()
            .unwrap()
            .write_register(0, "rax", 0x5678)
            .unwrap();
        session.control().unwrap().pause().unwrap();
        session.control().unwrap().resume().unwrap();
        session
            .acquisition()
            .unwrap()
            .save_snapshot(Path::new("guest.core"))
            .unwrap();
        let commands = transport.commands.lock().unwrap();
        assert_eq!(commands[0], ["showvminfo", "test-vm", "--machinereadable"]);
        assert_eq!(commands[2][2], "setregisters");
        assert_eq!(commands[3][2], "getregisters");
        assert_eq!(commands[4], ["controlvm", "test-vm", "pause"]);
        assert_eq!(commands[5], ["controlvm", "test-vm", "resume"]);
        assert_eq!(commands[6][2], "dumpvmcore");
    }

    #[test]
    fn parsers_fail_closed() {
        assert_eq!(
            parse_state("VMState=\"paused\"\n").unwrap(),
            ExecutionState::Paused
        );
        assert!(parse_state("Name=vm").is_err());
        assert_eq!(
            parse_register("RIP=fffff80000101234", "rip").unwrap(),
            0xffff_f800_0010_1234
        );
        assert_eq!(parse_register("RAX=0X2A", "rax").unwrap(), 0x2a);
        assert!(parse_register("rax=invalid", "rax").is_err());
    }

    #[test]
    fn snapshot_preflight_rejects_existing_files_and_directories() {
        let directory = std::env::temp_dir().join(format!(
            "vmi-vbox-preflight-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        fs::create_dir_all(&directory).unwrap();
        let file = directory.join("existing.core");
        fs::write(&file, b"preserve").unwrap();
        assert!(ensure_output_absent(&file, "VirtualBox core").is_err());
        assert!(ensure_output_absent(&directory, "VirtualBox core").is_err());
        assert!(ensure_output_absent(&directory.join("new.core"), "VirtualBox core").is_ok());
        assert_eq!(fs::read(&file).unwrap(), b"preserve");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn rejects_zero_length_acquisition_and_failed_write_verification() {
        let transport = Arc::new(FakeTransport {
            replies: Mutex::new(VecDeque::from([
                "VMState=\"running\"\n".into(),
                String::new(),
                "rax=0x2\n".into(),
            ])),
            commands: Mutex::new(Vec::new()),
        });
        let session =
            VirtualBoxConnector::with_transport("test-vm", GuestArchitecture::Amd64, transport)
                .with_register_write()
                .connect(AttachRequest::default())
                .unwrap();
        assert!(session.cpu().unwrap().write_register(0, "rax", 1).is_err());
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

        let connector = VirtualBoxConnector::with_transport(
            "test-vm",
            GuestArchitecture::Amd64,
            Arc::new(FakeTransport {
                replies: Mutex::new(VecDeque::new()),
                commands: Mutex::new(Vec::new()),
            }),
        );
        assert!(connector
            .connect(AttachRequest::any(CapabilitySet::from_caps([
                Capability::RegisterWrite
            ])))
            .is_err());
    }

    proptest! {
        #[test]
    fn private_virtualbox_parsers_fail_without_panicking(
            output in any::<String>(),
            register in any::<String>(),
        ) {
            let _ = parse_state(&output);
            let _ = parse_register(&output, &register);
        }
    }

    #[test]
    fn range_publication_is_atomic_and_never_clobbers() {
        let directory = std::env::temp_dir().join(format!(
            "vmi-vbox-publish-{}-{:?}",
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
    fn process_transport_captures_output_and_enforces_deadline() {
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

        let transport = ProcessTransport::new(executable)
            .with_timeout(Duration::from_secs(2))
            .unwrap();
        assert_eq!(transport.execute(&success).unwrap(), "ready");
        let timed = ProcessTransport::new(executable)
            .with_timeout(Duration::from_millis(50))
            .unwrap();
        assert!(matches!(
            timed.execute(&delayed),
            Err(VmiError::Timeout {
                operation: "VirtualBox command"
            })
        ));
        assert!(ProcessTransport::new(executable)
            .with_timeout(Duration::ZERO)
            .is_err());
        assert_eq!(
            ProcessTransport::new(executable)
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
