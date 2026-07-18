use std::{
    fs::{self, OpenOptions},
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
use vmi_driver_api::{AcquisitionAccess, Connector, ControlAccess, ExecutionState, Session};
use vmi_types::{
    AttachRequest, Capability, CapabilitySet, ConsistencyMode, Gpa, GuestArchitecture,
    ProviderDescriptor, ProviderMaturity, Result, TargetDescriptor, TargetSelector, VmiError,
};

const COMMAND_OUTPUT_CAPACITY: usize = 1024 * 1024;
const DEFAULT_COMMAND_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_COMMAND_TIMEOUT: Duration = Duration::from_secs(24 * 60 * 60);
static TEMPORARY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub trait VirshTransport: Send + Sync {
    fn execute(&self, arguments: &[String]) -> Result<String>;
}

#[derive(Clone, Debug)]
pub struct ProcessTransport {
    executable: String,
    timeout: Duration,
}

impl Default for ProcessTransport {
    fn default() -> Self {
        Self {
            executable: "virsh".into(),
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
        if timeout.is_zero() {
            return Err(VmiError::Backend(
                "virsh command timeout must be non-zero".into(),
            ));
        }
        self.timeout = timeout.min(MAX_COMMAND_TIMEOUT);
        Ok(self)
    }
}

impl VirshTransport for ProcessTransport {
    fn execute(&self, arguments: &[String]) -> Result<String> {
        let (status, stdout, stderr) =
            run_bounded_command(&self.executable, arguments, self.timeout)?;
        if !status.success() {
            return Err(VmiError::Backend(format!(
                "virsh failed with {status}: {}",
                String::from_utf8_lossy(&stderr).trim()
            )));
        }
        String::from_utf8(stdout)
            .map_err(|error| VmiError::Backend(format!("virsh output is not UTF-8: {error}")))
    }
}

fn run_bounded_command(
    executable: &str,
    arguments: &[String],
    timeout: Duration,
) -> Result<(ExitStatus, Vec<u8>, Vec<u8>)> {
    let mut child = Command::new(executable)
        .args(arguments)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| VmiError::Backend(format!("failed to execute {executable}: {error}")))?;
    let stdout = child.stdout.take().ok_or_else(|| {
        terminate_and_reap(&mut child);
        VmiError::Backend("failed to capture virsh stdout".into())
    })?;
    let stderr = child.stderr.take().ok_or_else(|| {
        terminate_and_reap(&mut child);
        VmiError::Backend("failed to capture virsh stderr".into())
    })?;
    let exceeded = Arc::new(AtomicBool::new(false));
    let captured = Arc::new(AtomicUsize::new(0));
    let stdout_reader = spawn_reader(
        stdout,
        Arc::clone(&exceeded),
        Arc::clone(&captured),
        "stdout",
    )?;
    let stderr_reader = spawn_reader(
        stderr,
        Arc::clone(&exceeded),
        Arc::clone(&captured),
        "stderr",
    )?;
    let deadline = Instant::now().checked_add(timeout);
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|error| {
            terminate_and_reap(&mut child);
            VmiError::Backend(format!("failed to wait for virsh: {error}"))
        })? {
            break status;
        }
        if exceeded.load(Ordering::Acquire) {
            terminate_and_reap(&mut child);
            return Err(VmiError::Backend(format!(
                "virsh output exceeds {COMMAND_OUTPUT_CAPACITY} bytes"
            )));
        }
        if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            terminate_and_reap(&mut child);
            return Err(VmiError::Timeout {
                operation: "libvirt command",
            });
        }
        thread::sleep(Duration::from_millis(5));
    };
    let stdout = join_reader(stdout_reader, "stdout")?;
    let stderr = join_reader(stderr_reader, "stderr")?;
    if exceeded.load(Ordering::Acquire) {
        return Err(VmiError::Backend(format!(
            "virsh output exceeds {COMMAND_OUTPUT_CAPACITY} bytes"
        )));
    }
    Ok((status, stdout, stderr))
}

fn spawn_reader(
    mut pipe: impl Read + Send + 'static,
    exceeded: Arc<AtomicBool>,
    captured: Arc<AtomicUsize>,
    stream: &'static str,
) -> Result<thread::JoinHandle<std::io::Result<Vec<u8>>>> {
    thread::Builder::new()
        .name(format!("virsh-{stream}"))
        .spawn(move || {
            let mut output = Vec::new();
            let mut chunk = [0u8; 8192];
            loop {
                let count = loop {
                    match pipe.read(&mut chunk) {
                        Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                        result => break result?,
                    }
                };
                if count == 0 {
                    break;
                }
                if !reserve_captured_bytes(&captured, count, COMMAND_OUTPUT_CAPACITY) {
                    exceeded.store(true, Ordering::Release);
                    break;
                }
                output.try_reserve(count).map_err(std::io::Error::other)?;
                let bytes = chunk
                    .get(..count)
                    .ok_or_else(|| std::io::Error::other("virsh read exceeded its buffer"))?;
                output.extend_from_slice(bytes);
            }
            Ok(output)
        })
        .map_err(|error| {
            VmiError::Backend(format!("failed to start virsh {stream} reader: {error}"))
        })
}

fn reserve_captured_bytes(counter: &AtomicUsize, count: usize, capacity: usize) -> bool {
    let mut current = counter.load(Ordering::Acquire);
    loop {
        let Some(next) = current.checked_add(count) else {
            return false;
        };
        if next > capacity {
            return false;
        }
        match counter.compare_exchange_weak(current, next, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => return true,
            Err(observed) => current = observed,
        }
    }
}

fn join_reader(
    reader: thread::JoinHandle<std::io::Result<Vec<u8>>>,
    stream: &str,
) -> Result<Vec<u8>> {
    reader
        .join()
        .map_err(|_| VmiError::Backend(format!("virsh {stream} reader panicked")))?
        .map_err(|error| VmiError::Backend(format!("failed to read virsh {stream}: {error}")))
}

fn terminate_and_reap(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

#[derive(Clone)]
pub struct LibvirtConnector {
    descriptor: Arc<ProviderDescriptor>,
    target: Arc<TargetDescriptor>,
    transport: Arc<dyn VirshTransport>,
    uri: Option<String>,
}

impl LibvirtConnector {
    pub fn new(domain: impl Into<String>, architecture: GuestArchitecture) -> Self {
        Self::with_transport(domain, architecture, Arc::new(ProcessTransport::default()))
    }

    pub fn with_transport(
        domain: impl Into<String>,
        architecture: GuestArchitecture,
        transport: Arc<dyn VirshTransport>,
    ) -> Self {
        let domain = domain.into();
        let capabilities = CapabilitySet::from_caps([Capability::Control, Capability::Acquisition]);
        Self {
            descriptor: Arc::new(ProviderDescriptor::new(
                "libvirt-qemu",
                "libvirt QEMU/KVM",
                ProviderMaturity::Experimental,
                capabilities,
            )),
            target: Arc::new(TargetDescriptor::new(
                domain.clone(),
                Some(domain),
                architecture,
                ConsistencyMode::LiveBestEffort,
            )),
            transport,
            uri: None,
        }
    }

    pub fn with_uri(mut self, uri: impl Into<String>) -> Self {
        self.uri = Some(uri.into());
        self
    }
}

impl Connector for LibvirtConnector {
    fn descriptor(&self) -> &ProviderDescriptor {
        &self.descriptor
    }

    fn connect(&self, request: AttachRequest) -> Result<Box<dyn Session>> {
        validate_argument(&self.target.id, "libvirt domain")?;
        if let Some(uri) = &self.uri {
            validate_argument(uri, "libvirt URI")?;
        }
        let missing = request
            .required_capabilities
            .difference_of(self.descriptor.capabilities);
        if !missing.is_empty() {
            return Err(VmiError::AttachRejected {
                provider: self.descriptor.id.clone(),
                missing,
            });
        }
        if let TargetSelector::Named(expected) = request.selector {
            if expected != self.target.id {
                return Err(VmiError::Backend(format!(
                    "libvirt domain {expected} does not match {}",
                    self.target.id
                )));
            }
        }
        let session = LibvirtSession {
            descriptor: Arc::clone(&self.descriptor),
            target: Arc::clone(&self.target),
            transport: Arc::clone(&self.transport),
            uri: self.uri.clone(),
        };
        let xml = session.execute(&["dumpxml", &self.target.id])?;
        if !is_qemu_domain(&xml) {
            return Err(VmiError::Backend(
                "libvirt domain is not a QEMU/KVM domain".into(),
            ));
        }
        session.execution_state()?;
        Ok(Box::new(session))
    }
}

fn validate_argument(value: &str, description: &str) -> Result<()> {
    if value.is_empty() || value.starts_with('-') || value.chars().any(char::is_control) {
        return Err(VmiError::Backend(format!("invalid {description}")));
    }
    Ok(())
}

fn is_qemu_domain(xml: &str) -> bool {
    ["type='kvm'", "type=\"kvm\"", "type='qemu'", "type=\"qemu\""]
        .iter()
        .any(|marker| xml.contains(marker))
}

struct LibvirtSession {
    descriptor: Arc<ProviderDescriptor>,
    target: Arc<TargetDescriptor>,
    transport: Arc<dyn VirshTransport>,
    uri: Option<String>,
}

impl LibvirtSession {
    fn execute(&self, arguments: &[&str]) -> Result<String> {
        let extra = if self.uri.is_some() { 2 } else { 0 };
        let mut owned = Vec::new();
        owned
            .try_reserve_exact(arguments.len().checked_add(extra).ok_or_else(|| {
                VmiError::Backend("virsh argument count exceeds addressable memory".into())
            })?)
            .map_err(|error| {
                VmiError::Backend(format!("failed to allocate virsh arguments: {error}"))
            })?;
        if let Some(uri) = &self.uri {
            owned.push("--connect".into());
            owned.push(uri.clone());
        }
        for argument in arguments {
            owned.push((*argument).to_owned());
        }
        self.transport.execute(&owned)
    }
}

impl ControlAccess for LibvirtSession {
    fn execution_state(&self) -> Result<ExecutionState> {
        let state = self.execute(&["domstate", &self.target.id])?;
        Ok(match state.trim().to_ascii_lowercase().as_str() {
            "running" | "idle" | "blocked" => ExecutionState::Running,
            "paused" | "pmsuspended" => ExecutionState::Paused,
            "shut off" | "shutdown" | "crashed" => ExecutionState::Shutdown,
            _ => ExecutionState::Unknown,
        })
    }

    fn pause(&self) -> Result<()> {
        self.execute(&["suspend", &self.target.id])?;
        Ok(())
    }

    fn resume(&self) -> Result<()> {
        self.execute(&["resume", &self.target.id])?;
        Ok(())
    }
}

impl AcquisitionAccess for LibvirtSession {
    fn save_physical_range(&self, path: &Path, start: Gpa, length: u64) -> Result<()> {
        if length == 0 {
            return Err(VmiError::Backend(
                "libvirt acquisition length must be non-zero".into(),
            ));
        }
        start.raw().checked_add(length).ok_or_else(|| {
            VmiError::Backend("libvirt acquisition physical range overflows".into())
        })?;
        let length = usize::try_from(length)
            .map_err(|_| VmiError::Backend("requested libvirt range is too large".into()))?;
        ensure_output_absent(path, "libvirt range")?;
        let temporary = temporary_core_path(path)?;
        let result = (|| {
            self.save_snapshot(&temporary)?;
            let bundle = SnapshotBundle::elf_vmcore_file(&temporary)?;
            let mut bytes = Vec::new();
            bytes
                .try_reserve_exact(length)
                .map_err(|error| VmiError::Backend(format!("failed to allocate range: {error}")))?;
            bytes.resize(length, 0);
            bundle.read_into(start, &mut bytes)?;
            publish_new(path, &bytes)
        })();
        let cleanup = fs::remove_file(&temporary);
        match (result, cleanup) {
            (Err(error), _) => Err(error),
            (Ok(()), Ok(())) => Ok(()),
            (Ok(()), Err(error)) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            (Ok(()), Err(error)) => Err(VmiError::Backend(format!(
                "failed to remove temporary libvirt core: {error}"
            ))),
        }
    }

    fn save_snapshot(&self, path: &Path) -> Result<()> {
        ensure_output_absent(path, "libvirt core")?;
        let destination = path
            .to_str()
            .ok_or_else(|| VmiError::Backend("libvirt core path is not UTF-8".into()))?;
        self.execute(&[
            "dump",
            &self.target.id,
            destination,
            "--memory-only",
            "--live",
            "--format",
            "elf",
        ])?;
        if !path.is_file() {
            return Err(VmiError::Backend(
                "virsh reported success without creating the core".into(),
            ));
        }
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

fn temporary_core_path(path: &Path) -> Result<PathBuf> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| VmiError::Backend("libvirt output filename is not UTF-8".into()))?;
    let mut sequence = TEMPORARY_SEQUENCE.load(Ordering::Acquire);
    loop {
        let next = sequence
            .checked_add(1)
            .ok_or_else(|| VmiError::Backend("libvirt temporary sequence exhausted".into()))?;
        match TEMPORARY_SEQUENCE.compare_exchange_weak(
            sequence,
            next,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => break,
            Err(observed) => sequence = observed,
        }
    }
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| VmiError::Backend(format!("system clock precedes epoch: {error}")))?
        .as_nanos();
    Ok(parent.join(format!(
        ".{name}.libvirt-{}-{nanos}-{sequence}.core",
        std::process::id()
    )))
}

fn publish_new(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| {
            VmiError::Backend(format!("failed to create {}: {error}", path.display()))
        })?;
    if let Err(error) = file.write_all(bytes).and_then(|()| file.sync_all()) {
        drop(file);
        let _ = fs::remove_file(path);
        return Err(VmiError::Backend(format!(
            "failed to publish {}: {error}",
            path.display()
        )));
    }
    Ok(())
}

impl Session for LibvirtSession {
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
    fn acquisition(&self) -> Result<&dyn AcquisitionAccess> {
        Ok(self)
    }
}

#[cfg(test)]
#[allow(clippy::arithmetic_side_effects, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct FakeTransport {
        state: Mutex<String>,
        commands: Mutex<Vec<Vec<String>>>,
    }

    impl FakeTransport {
        fn new(state: &str) -> Self {
            Self {
                state: Mutex::new(state.into()),
                commands: Mutex::new(Vec::new()),
            }
        }
    }

    impl VirshTransport for FakeTransport {
        fn execute(&self, arguments: &[String]) -> Result<String> {
            self.commands.lock().unwrap().push(arguments.to_vec());
            let command = arguments.iter().position(|argument| {
                argument == "dumpxml"
                    || argument == "domstate"
                    || argument == "suspend"
                    || argument == "resume"
                    || argument == "dump"
            });
            match command.map(|index| arguments[index].as_str()) {
                Some("dumpxml") => Ok("<domain type='kvm'><name>guest</name></domain>".into()),
                Some("domstate") => Ok(self.state.lock().unwrap().clone()),
                Some("suspend") => {
                    *self.state.lock().unwrap() = "paused".into();
                    Ok(String::new())
                }
                Some("resume") => {
                    *self.state.lock().unwrap() = "running".into();
                    Ok(String::new())
                }
                Some("dump") => {
                    let index = command.unwrap();
                    let destination = arguments
                        .get(index + 2)
                        .ok_or_else(|| VmiError::Backend("missing fake dump path".into()))?;
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
                    fs::write(destination, elf).map_err(|error| {
                        VmiError::Backend(format!("failed to write fake core: {error}"))
                    })?;
                    Ok(String::new())
                }
                _ => Err(VmiError::Backend("unexpected fake virsh command".into())),
            }
        }
    }

    #[test]
    fn attaches_only_qemu_domains_and_controls_state() {
        let transport = Arc::new(FakeTransport::new("running"));
        let connector =
            LibvirtConnector::with_transport("guest", GuestArchitecture::Amd64, transport.clone())
                .with_uri("qemu:///system");
        let session = connector.connect(AttachRequest::default()).unwrap();
        let control = session.control().unwrap();
        assert_eq!(control.execution_state().unwrap(), ExecutionState::Running);
        control.pause().unwrap();
        assert_eq!(control.execution_state().unwrap(), ExecutionState::Paused);
        control.resume().unwrap();
        assert_eq!(control.execution_state().unwrap(), ExecutionState::Running);
        assert!(
            transport
                .commands
                .lock()
                .unwrap()
                .iter()
                .all(|arguments| arguments
                    .starts_with(&["--connect".into(), "qemu:///system".into()]))
        );
    }

    #[test]
    fn rejects_option_like_targets_and_unsupported_capabilities() {
        let connector = LibvirtConnector::with_transport(
            "--all",
            GuestArchitecture::Amd64,
            Arc::new(FakeTransport::new("running")),
        );
        assert!(connector.connect(AttachRequest::default()).is_err());
        let connector = LibvirtConnector::with_transport(
            "guest",
            GuestArchitecture::Amd64,
            Arc::new(FakeTransport::new("running")),
        );
        let request = AttachRequest::any(CapabilitySet::from_caps([Capability::MemoryRead]));
        assert!(matches!(
            connector.connect(request),
            Err(VmiError::AttachRejected { .. })
        ));
    }

    #[test]
    fn maps_all_documented_domain_states() {
        for (text, expected) in [
            ("running", ExecutionState::Running),
            ("blocked", ExecutionState::Running),
            ("paused", ExecutionState::Paused),
            ("pmsuspended", ExecutionState::Paused),
            ("shut off", ExecutionState::Shutdown),
            ("crashed", ExecutionState::Shutdown),
            ("no state", ExecutionState::Unknown),
        ] {
            let connector = LibvirtConnector::with_transport(
                "guest",
                GuestArchitecture::Amd64,
                Arc::new(FakeTransport::new(text)),
            );
            let session = connector.connect(AttachRequest::default()).unwrap();
            assert_eq!(
                session.control().unwrap().execution_state().unwrap(),
                expected
            );
        }
    }

    #[test]
    fn acquires_elf_core_and_physical_range_without_overwriting() {
        let directory = std::env::temp_dir().join(format!(
            "vmi-libvirt-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&directory).unwrap();
        let connector = LibvirtConnector::with_transport(
            "guest",
            GuestArchitecture::Amd64,
            Arc::new(FakeTransport::new("running")),
        );
        let session = connector.connect(AttachRequest::default()).unwrap();
        let acquisition = session.acquisition().unwrap();
        let core = directory.join("guest.core");
        acquisition.save_snapshot(&core).unwrap();
        assert!(SnapshotBundle::elf_vmcore_file(&core).is_ok());
        assert!(acquisition.save_snapshot(&core).is_err());
        let range = directory.join("range.bin");
        acquisition
            .save_physical_range(&range, Gpa::new(0x3000), 4)
            .unwrap();
        assert_eq!(fs::read(&range).unwrap(), [0x11, 0x22, 0x33, 0x44]);
        assert!(acquisition
            .save_physical_range(&range, Gpa::new(0x3000), 4)
            .is_err());
        fs::remove_dir_all(directory).unwrap();
    }
}
