use std::{
    collections::VecDeque,
    fmt::{Arguments, Write as _},
    io::{BufRead, BufReader, Read, Write},
    net::{TcpStream, ToSocketAddrs},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use serde_json::{json, Value};
use vmi_driver_api::{
    AcquisitionAccess, Connector, ControlAccess, CpuAccess, EventAccess, ExecutionState,
    MemoryAccess, Session, VmiEvent,
};
use vmi_types::{
    AttachRequest, Capability, CapabilitySet, ConsistencyMode, Gpa, GuestArchitecture,
    ProviderDescriptor, ProviderMaturity, Result, TargetDescriptor, TargetSelector, VmiError,
};

const EVENT_QUEUE_CAPACITY: usize = 1024;
const QMP_MESSAGE_CAPACITY: u64 = 16 * 1024 * 1024;
const GDB_PACKET_CAPACITY: usize = 1024 * 1024;
const MAX_SOCKET_WAIT: Duration = Duration::from_secs(24 * 60 * 60);

fn bounded_socket_timeout(timeout: Duration) -> Duration {
    timeout.min(MAX_SOCKET_WAIT)
}

struct QmpClient {
    stream: BufReader<QmpStream>,
    next_id: Option<u64>,
    events: VecDeque<VmiEvent>,
    timeout: Duration,
}

enum QmpStream {
    Tcp(TcpStream),
    #[cfg(unix)]
    Unix(std::os::unix::net::UnixStream),
}

impl Read for QmpStream {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Self::Tcp(stream) => stream.read(buffer),
            #[cfg(unix)]
            Self::Unix(stream) => stream.read(buffer),
        }
    }
}

impl Write for QmpStream {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        match self {
            Self::Tcp(stream) => stream.write(buffer),
            #[cfg(unix)]
            Self::Unix(stream) => stream.write(buffer),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Self::Tcp(stream) => stream.flush(),
            #[cfg(unix)]
            Self::Unix(stream) => stream.flush(),
        }
    }
}

impl QmpStream {
    fn set_read_timeout(&self, timeout: Option<Duration>) -> std::io::Result<()> {
        match self {
            Self::Tcp(stream) => stream.set_read_timeout(timeout),
            #[cfg(unix)]
            Self::Unix(stream) => stream.set_read_timeout(timeout),
        }
    }
}

struct GdbClient {
    stream: TcpStream,
    timeout: Duration,
}

impl GdbClient {
    fn connect(address: impl ToSocketAddrs, timeout: Duration) -> Result<Self> {
        let stream = TcpStream::connect(address).map_err(backend)?;
        let timeout = bounded_socket_timeout(timeout);
        stream.set_read_timeout(Some(timeout)).map_err(backend)?;
        stream.set_write_timeout(Some(timeout)).map_err(backend)?;
        Ok(Self { stream, timeout })
    }

    fn command(&mut self, payload: &str) -> Result<String> {
        let deadline = Instant::now().checked_add(self.timeout);
        let checksum = payload.bytes().fold(0u8, u8::wrapping_add);
        write!(self.stream, "${payload}#{checksum:02x}").map_err(backend)?;
        self.stream.flush().map_err(backend)?;
        let mut byte = [0u8; 1];
        self.read_exact_before(deadline, &mut byte, "GDB acknowledgement")?;
        if byte[0] != b'+' {
            return Err(VmiError::Backend("GDB server rejected packet".into()));
        }
        loop {
            self.read_exact_before(deadline, &mut byte, "GDB response start")?;
            if byte[0] == b'$' {
                break;
            }
        }
        let mut response = Vec::new();
        loop {
            self.read_exact_before(deadline, &mut byte, "GDB response payload")?;
            if byte[0] == b'#' {
                break;
            }
            if response.len() == GDB_PACKET_CAPACITY {
                return Err(VmiError::Backend(format!(
                    "GDB response exceeds {GDB_PACKET_CAPACITY} bytes"
                )));
            }
            if response.len() == response.capacity() {
                response.try_reserve(1).map_err(|error| {
                    VmiError::Backend(format!("failed to grow GDB response buffer: {error}"))
                })?;
            }
            response.push(byte[0]);
        }
        let mut checksum_bytes = [0u8; 2];
        self.read_exact_before(deadline, &mut checksum_bytes, "GDB response checksum")?;
        let expected =
            u8::from_str_radix(std::str::from_utf8(&checksum_bytes).map_err(backend)?, 16)
                .map_err(backend)?;
        let actual = response.iter().copied().fold(0u8, u8::wrapping_add);
        if expected != actual {
            return Err(VmiError::Backend("GDB response checksum mismatch".into()));
        }
        self.stream.write_all(b"+").map_err(backend)?;
        let response = String::from_utf8(response).map_err(backend)?;
        if response.starts_with('E') {
            return Err(VmiError::Backend(format!("GDB command failed: {response}")));
        }
        Ok(response)
    }

    fn read_exact_before(
        &mut self,
        deadline: Option<Instant>,
        output: &mut [u8],
        phase: &'static str,
    ) -> Result<()> {
        let now = Instant::now();
        let remaining = match deadline {
            Some(deadline) if now >= deadline => {
                return Err(VmiError::Timeout { operation: phase })
            }
            Some(deadline) => bounded_socket_timeout(deadline.saturating_duration_since(now)),
            None => MAX_SOCKET_WAIT,
        };
        self.stream
            .set_read_timeout(Some(remaining))
            .map_err(backend)?;
        self.stream.read_exact(output).map_err(|error| {
            if matches!(
                error.kind(),
                std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
            ) {
                VmiError::Timeout { operation: phase }
            } else {
                backend(error)
            }
        })
    }

    fn read_register(&mut self, register: u32, width: usize) -> Result<u64> {
        let command = try_format_command(9, format_args!("p{register:x}"), "GDB register read")?;
        let response = self.command(&command)?;
        decode_little_hex(&response, width)
    }

    fn select_vcpu(&mut self, vcpu: u32) -> Result<()> {
        let thread = gdb_thread_id(vcpu)?;
        let command = try_format_command(10, format_args!("Hg{thread}"), "GDB thread selection")?;
        let response = self.command(&command)?;
        if response != "OK" {
            return Err(VmiError::Backend(format!(
                "GDB failed to select vCPU {vcpu}: {response}"
            )));
        }
        Ok(())
    }

    fn write_register(&mut self, register: u32, width: usize, value: u64) -> Result<()> {
        let width_bits = width
            .checked_mul(8)
            .ok_or_else(|| VmiError::Backend("GDB register width overflow".into()))?;
        let limit = u32::try_from(width_bits)
            .ok()
            .and_then(|bits| 1u64.checked_shl(bits));
        if width < 8 && limit.is_none_or(|limit| value >= limit) {
            return Err(VmiError::Backend(format!(
                "register value {value:#x} does not fit {width} bytes"
            )));
        }
        let encoded = encode_little_hex(value, width)?;
        let capacity = encoded.len().checked_add(10).ok_or_else(|| {
            VmiError::Backend("GDB register write command length overflow".into())
        })?;
        let command = try_format_command(
            capacity,
            format_args!("P{register:x}={encoded}"),
            "GDB register write",
        )?;
        let response = self.command(&command)?;
        if response != "OK" {
            return Err(VmiError::Backend(format!(
                "unexpected GDB write response: {response}"
            )));
        }
        Ok(())
    }
}

fn gdb_thread_id(vcpu: u32) -> Result<String> {
    let thread = vcpu
        .checked_add(1)
        .ok_or_else(|| VmiError::Backend("vCPU index is too large".into()))?;
    try_format_command(8, format_args!("{thread:x}"), "GDB thread ID")
}

fn try_format_command(
    capacity: usize,
    arguments: Arguments<'_>,
    description: &str,
) -> Result<String> {
    let mut command = String::new();
    command.try_reserve_exact(capacity).map_err(|error| {
        VmiError::Backend(format!("failed to allocate {description} command: {error}"))
    })?;
    command.write_fmt(arguments).map_err(|error| {
        VmiError::Backend(format!("failed to format {description} command: {error}"))
    })?;
    Ok(command)
}

fn encode_little_hex(value: u64, width: usize) -> Result<String> {
    if width > 8 {
        return Err(VmiError::Backend(format!(
            "invalid GDB register width: {width} bytes"
        )));
    }
    fn hex_digit(nibble: u8) -> char {
        char::from(if nibble < 10 {
            b'0'.wrapping_add(nibble)
        } else {
            b'a'.wrapping_add(nibble.wrapping_sub(10))
        })
    }

    let mut encoded = String::new();
    let encoded_length = width
        .checked_mul(2)
        .ok_or_else(|| VmiError::Backend("GDB register encoding length overflow".into()))?;
    encoded.try_reserve_exact(encoded_length).map_err(|error| {
        VmiError::Backend(format!("failed to allocate GDB register encoding: {error}"))
    })?;
    for byte in value.to_le_bytes().into_iter().take(width) {
        encoded.push(hex_digit(byte >> 4));
        encoded.push(hex_digit(byte & 0xf));
    }
    Ok(encoded)
}

fn decode_little_hex(value: &str, width: usize) -> Result<u64> {
    let expected = width.checked_mul(2);
    if width > 8 || expected != Some(value.len()) {
        return Err(VmiError::Backend(format!(
            "invalid GDB register width: {} hex digits",
            value.len()
        )));
    }
    let mut bytes = [0u8; 8];
    let encoded = value.as_bytes();
    for (index, (pair, output)) in encoded.chunks_exact(2).zip(bytes.iter_mut()).enumerate() {
        let position = index
            .checked_mul(2)
            .ok_or_else(|| VmiError::Backend("GDB register hex position overflow".into()))?;
        let [high_byte, low_byte] = pair else {
            return Err(VmiError::Backend("invalid GDB register hex pair".into()));
        };
        let high = hex_nibble(*high_byte);
        let low = hex_nibble(*low_byte);
        *output = match (high, low) {
            (Some(high), Some(low)) => (high << 4) | low,
            _ => {
                return Err(VmiError::Backend(format!(
                    "invalid GDB register hex at byte {position}"
                )))
            }
        };
    }
    Ok(u64::from_le_bytes(bytes))
}

const fn hex_nibble(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value.wrapping_sub(b'0')),
        b'a'..=b'f' => Some(value.wrapping_sub(b'a').wrapping_add(10)),
        b'A'..=b'F' => Some(value.wrapping_sub(b'A').wrapping_add(10)),
        _ => None,
    }
}

fn amd64_register(register: &str) -> Option<(u32, usize)> {
    let (number, width) = match register.to_ascii_lowercase().as_str() {
        "rax" | "eax" => (0, 8),
        "rbx" | "ebx" => (1, 8),
        "rcx" | "ecx" => (2, 8),
        "rdx" | "edx" => (3, 8),
        "rsi" | "esi" => (4, 8),
        "rdi" | "edi" => (5, 8),
        "rbp" | "ebp" => (6, 8),
        "rsp" | "esp" => (7, 8),
        "r8" => (8, 8),
        "r9" => (9, 8),
        "r10" => (10, 8),
        "r11" => (11, 8),
        "r12" => (12, 8),
        "r13" => (13, 8),
        "r14" => (14, 8),
        "r15" => (15, 8),
        "rip" | "eip" => (16, 8),
        "rflags" | "eflags" | "efl" => (17, 4),
        _ => return None,
    };
    Some((number, width))
}

impl QmpClient {
    fn connect(address: impl ToSocketAddrs, timeout: Duration) -> Result<Self> {
        let stream = TcpStream::connect(address).map_err(backend)?;
        let socket_timeout = bounded_socket_timeout(timeout);
        stream
            .set_read_timeout(Some(socket_timeout))
            .map_err(backend)?;
        stream
            .set_write_timeout(Some(socket_timeout))
            .map_err(backend)?;
        Self::initialize(QmpStream::Tcp(stream), timeout)
    }

    #[cfg(unix)]
    fn connect_unix(path: &Path, timeout: Duration) -> Result<Self> {
        let stream = std::os::unix::net::UnixStream::connect(path).map_err(backend)?;
        let socket_timeout = bounded_socket_timeout(timeout);
        stream
            .set_read_timeout(Some(socket_timeout))
            .map_err(backend)?;
        stream
            .set_write_timeout(Some(socket_timeout))
            .map_err(backend)?;
        Self::initialize(QmpStream::Unix(stream), timeout)
    }

    fn initialize(stream: QmpStream, timeout: Duration) -> Result<Self> {
        let mut events = VecDeque::new();
        events
            .try_reserve_exact(EVENT_QUEUE_CAPACITY)
            .map_err(|error| {
                VmiError::Backend(format!("failed to allocate QMP event queue: {error}"))
            })?;
        let mut client = Self {
            stream: BufReader::new(stream),
            next_id: Some(1),
            events,
            timeout,
        };
        let greeting = client.read_json()?;
        if greeting.get("QMP").is_none() {
            return Err(VmiError::Backend("QMP greeting was not received".into()));
        }
        client.execute("qmp_capabilities", None)?;
        Ok(client)
    }

    fn read_json(&mut self) -> Result<Value> {
        self.read_json_with_capacity(QMP_MESSAGE_CAPACITY)
    }

    fn read_json_with_capacity(&mut self, capacity: u64) -> Result<Value> {
        let mut line = Vec::new();
        loop {
            let (available, consumed, complete) = {
                let available = self.stream.fill_buf().map_err(qmp_read_error)?;
                if available.is_empty() {
                    if line.is_empty() {
                        return Err(VmiError::Backend("QMP connection closed".into()));
                    }
                    return Err(VmiError::Backend(format!(
                        "QMP message exceeds {capacity} bytes"
                    )));
                }
                let consumed = available
                    .iter()
                    .position(|byte| *byte == b'\n')
                    .and_then(|position| position.checked_add(1))
                    .unwrap_or(available.len());
                let total = line
                    .len()
                    .checked_add(consumed)
                    .ok_or_else(|| VmiError::Backend("QMP message length overflow".into()))?;
                if u64::try_from(total).map_or(true, |total| total > capacity) {
                    return Err(VmiError::Backend(format!(
                        "QMP message exceeds {capacity} bytes"
                    )));
                }
                line.try_reserve(consumed).map_err(|error| {
                    VmiError::Backend(format!("failed to grow QMP message buffer: {error}"))
                })?;
                let chunk = available.get(..consumed).ok_or_else(|| {
                    VmiError::Backend("QMP buffered-read boundary invariant failed".into())
                })?;
                let complete = chunk.last().copied() == Some(b'\n');
                line.extend_from_slice(chunk);
                (available.len(), consumed, complete)
            };
            self.stream.consume(consumed);
            if complete {
                break;
            }
            debug_assert_eq!(available, consumed);
        }
        serde_json::from_slice(&line)
            .map_err(|error| VmiError::Backend(format!("invalid QMP JSON: {error}")))
    }

    fn execute(&mut self, command: &str, arguments: Option<Value>) -> Result<Value> {
        let deadline = Instant::now().checked_add(self.timeout);
        let id = take_request_id(&mut self.next_id)?;
        let mut request = json!({ "execute": command, "id": id });
        if let Some(arguments) = arguments {
            request
                .as_object_mut()
                .ok_or_else(|| VmiError::Backend("QMP request is not an object".into()))?
                .insert("arguments".into(), arguments);
        }
        writeln!(self.stream.get_mut(), "{request}").map_err(backend)?;
        self.stream.get_mut().flush().map_err(backend)?;
        loop {
            let now = Instant::now();
            let remaining = match deadline {
                Some(deadline) if now >= deadline => {
                    return Err(VmiError::Timeout {
                        operation: "QMP command",
                    })
                }
                Some(deadline) => bounded_socket_timeout(deadline.saturating_duration_since(now)),
                None => MAX_SOCKET_WAIT,
            };
            self.stream
                .get_ref()
                .set_read_timeout(Some(remaining))
                .map_err(backend)?;
            let mut response = match self.read_json() {
                Err(VmiError::Timeout {
                    operation: "QMP read",
                }) => {
                    return Err(VmiError::Timeout {
                        operation: "QMP command",
                    })
                }
                result => result?,
            };
            if let Some(event) = qmp_event(&response)? {
                enqueue_event(&mut self.events, event)?;
                continue;
            }
            if response.get("id").and_then(Value::as_u64) != Some(id) {
                continue;
            }
            if let Some(error) = response.get("error") {
                return Err(VmiError::Backend(format!("QMP {command} failed: {error}")));
            }
            return Ok(take_qmp_return(&mut response));
        }
    }

    fn next_event(&mut self, timeout: Duration) -> Result<Option<VmiEvent>> {
        if let Some(event) = self.events.pop_front() {
            return Ok(Some(event));
        }
        if timeout.is_zero() {
            return Ok(None);
        }
        let deadline = Instant::now().checked_add(timeout);
        loop {
            let now = Instant::now();
            let remaining = match deadline {
                Some(deadline) if now >= deadline => return Ok(None),
                Some(deadline) => bounded_socket_timeout(deadline.saturating_duration_since(now)),
                None => MAX_SOCKET_WAIT,
            };
            self.stream
                .get_ref()
                .set_read_timeout(Some(remaining))
                .map_err(backend)?;
            match self.read_json() {
                Ok(value) => {
                    if let Some(event) = qmp_event(&value)? {
                        return Ok(Some(event));
                    }
                }
                Err(VmiError::Timeout {
                    operation: "QMP read",
                }) => {
                    if deadline.is_some() {
                        return Ok(None);
                    }
                }
                Err(error) => return Err(error),
            }
        }
    }

    fn hmp(&mut self, command: String) -> Result<Value> {
        self.execute(
            "human-monitor-command",
            Some(json!({ "command-line": command })),
        )
    }
}

fn take_request_id(next_id: &mut Option<u64>) -> Result<u64> {
    let id = next_id
        .take()
        .ok_or_else(|| VmiError::Backend("QMP request ID space exhausted".into()))?;
    *next_id = id.checked_add(1);
    Ok(id)
}

fn take_qmp_return(response: &mut Value) -> Value {
    response
        .as_object_mut()
        .and_then(|object| object.remove("return"))
        .unwrap_or(Value::Null)
}

fn qmp_event(value: &Value) -> Result<Option<VmiEvent>> {
    let Some(kind) = value.get("event").and_then(Value::as_str) else {
        return Ok(None);
    };
    let mut owned_kind = String::new();
    owned_kind.try_reserve_exact(kind.len()).map_err(|error| {
        VmiError::Backend(format!("failed to allocate QMP event kind: {error}"))
    })?;
    owned_kind.push_str(kind);
    let data = value.get("data");
    let vcpu = data
        .and_then(|data| data.get("cpu-index").or_else(|| data.get("vcpu")))
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok());
    let address = data
        .and_then(|data| data.get("address").or_else(|| data.get("addr")))
        .and_then(Value::as_u64)
        .map(Gpa::new);
    Ok(Some(VmiEvent {
        kind: owned_kind,
        vcpu,
        address,
    }))
}

fn enqueue_event(events: &mut VecDeque<VmiEvent>, event: VmiEvent) -> Result<()> {
    if events.len() == EVENT_QUEUE_CAPACITY {
        return Err(VmiError::Backend(format!(
            "QMP event queue capacity {EVENT_QUEUE_CAPACITY} exceeded"
        )));
    }
    events
        .try_reserve(1)
        .map_err(|error| VmiError::Backend(format!("failed to grow QMP event queue: {error}")))?;
    events.push_back(event);
    Ok(())
}

fn backend(error: impl std::fmt::Display) -> VmiError {
    VmiError::Backend(error.to_string())
}

fn qmp_read_error(error: std::io::Error) -> VmiError {
    if matches!(
        error.kind(),
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
    ) {
        VmiError::Timeout {
            operation: "QMP read",
        }
    } else {
        backend(error)
    }
}

#[derive(Clone, Debug)]
pub struct QemuConnector {
    endpoint: QmpEndpoint,
    gdb_address: Option<String>,
    descriptor: Arc<ProviderDescriptor>,
    timeout: Duration,
}

#[derive(Clone, Debug)]
enum QmpEndpoint {
    Tcp(String),
    #[cfg(unix)]
    Unix(PathBuf),
}

impl QmpEndpoint {
    fn try_name(&self) -> Result<String> {
        match self {
            Self::Tcp(address) => try_endpoint_text(address, "QMP TCP target"),
            #[cfg(unix)]
            Self::Unix(path) => {
                let rendered = path
                    .to_str()
                    .ok_or_else(|| VmiError::Backend("QMP Unix target is not UTF-8".into()))?;
                try_endpoint_text(rendered, "QMP Unix target")
            }
        }
    }
}

fn try_endpoint_text(value: &str, description: &str) -> Result<String> {
    validate_endpoint_text(value, description)?;
    try_owned_text(value, description)
}

fn validate_endpoint_text(value: &str, description: &str) -> Result<()> {
    if value.is_empty() || value.chars().any(char::is_control) {
        return Err(VmiError::Backend(format!("invalid {description}")));
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

impl QemuConnector {
    pub fn tcp(address: impl Into<String>) -> Self {
        let capabilities = CapabilitySet::from_caps([
            Capability::MemoryRead,
            Capability::RegisterRead,
            Capability::Control,
            Capability::Acquisition,
            Capability::Events,
        ]);
        Self {
            endpoint: QmpEndpoint::Tcp(address.into()),
            gdb_address: None,
            descriptor: Arc::new(ProviderDescriptor::new(
                "qemu-qmp",
                "QEMU QMP",
                ProviderMaturity::Preview,
                capabilities,
            )),
            timeout: Duration::from_secs(10),
        }
    }

    #[cfg(unix)]
    pub fn unix(path: impl Into<PathBuf>) -> Self {
        let mut connector = Self::tcp(String::new());
        connector.endpoint = QmpEndpoint::Unix(path.into());
        connector
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Result<Self> {
        if timeout.is_zero() {
            return Err(VmiError::Backend("QMP timeout must be non-zero".into()));
        }
        self.timeout = bounded_socket_timeout(timeout);
        Ok(self)
    }

    pub fn with_gdb(mut self, address: impl Into<String>) -> Self {
        self.gdb_address = Some(address.into());
        Arc::make_mut(&mut self.descriptor)
            .capabilities
            .insert_capability(Capability::RegisterWrite);
        self
    }
}

impl Connector for QemuConnector {
    fn descriptor(&self) -> &ProviderDescriptor {
        &self.descriptor
    }
    fn connect(&self, request: AttachRequest) -> Result<Box<dyn Session>> {
        let missing = request
            .required_capabilities
            .difference_of(self.descriptor.capabilities);
        if !missing.is_empty() {
            return Err(VmiError::AttachRejected {
                provider: try_owned_text(&self.descriptor.id, "QEMU provider ID")?,
                missing,
            });
        }
        let target = self.endpoint.try_name()?;
        if let TargetSelector::Named(expected) = request.selector {
            if expected != target {
                return Err(VmiError::Backend(format!(
                    "QMP target {expected} not found"
                )));
            }
        }
        let display_name = try_owned_text(&target, "QMP target display name")?;
        if let Some(address) = self.gdb_address.as_deref() {
            validate_endpoint_text(address, "QEMU GDB target")?;
        }
        let client = match &self.endpoint {
            QmpEndpoint::Tcp(address) => QmpClient::connect(address, self.timeout)?,
            #[cfg(unix)]
            QmpEndpoint::Unix(path) => QmpClient::connect_unix(path, self.timeout)?,
        };
        let gdb = match self.gdb_address.as_deref() {
            Some(address) => Some(Mutex::new(GdbClient::connect(address, self.timeout)?)),
            None => None,
        };
        Ok(Box::new(QemuSession {
            descriptor: self.descriptor.clone(),
            target: TargetDescriptor {
                id: target,
                display_name: Some(display_name),
                architecture: GuestArchitecture::Amd64,
                consistency: ConsistencyMode::LiveBestEffort,
            },
            client: Mutex::new(client),
            gdb,
        }))
    }
}

struct QemuSession {
    descriptor: Arc<ProviderDescriptor>,
    target: TargetDescriptor,
    client: Mutex<QmpClient>,
    gdb: Option<Mutex<GdbClient>>,
}

fn register_name(register: &str) -> Result<String> {
    if register.is_empty()
        || !register
            .chars()
            .all(|character| character.is_ascii_alphanumeric())
    {
        return Err(VmiError::Backend(format!(
            "invalid register name {register:?}"
        )));
    }
    let mut normalized = String::new();
    normalized
        .try_reserve_exact(register.len())
        .map_err(|error| {
            VmiError::Backend(format!(
                "failed to allocate normalized register name: {error}"
            ))
        })?;
    normalized.extend(
        register
            .bytes()
            .map(|byte| char::from(byte.to_ascii_uppercase())),
    );
    Ok(normalized)
}

fn parse_hmp_register(text: &str, register: &str) -> Option<u64> {
    let aliases: &[&str] = match register {
        "RIP" => &["RIP", "EIP"],
        "RFLAGS" => &["RFLAGS", "EFLAGS", "EFL"],
        _ => std::slice::from_ref(&register),
    };
    text.split_whitespace().find_map(|token| {
        let (name, value) = token.split_once('=')?;
        aliases
            .iter()
            .any(|candidate| name.eq_ignore_ascii_case(candidate))
            .then(|| u64::from_str_radix(strip_hex_prefix(value), 16).ok())
            .flatten()
    })
}

fn strip_hex_prefix(value: &str) -> &str {
    value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
        .unwrap_or(value)
}

impl CpuAccess for QemuSession {
    fn read_register(&self, vcpu: u32, register: &str) -> Result<u64> {
        let register = register_name(register)?;
        if let (Some(gdb), Some((number, width))) = (&self.gdb, amd64_register(&register)) {
            let mut gdb = gdb.lock().map_err(backend)?;
            gdb.select_vcpu(vcpu)?;
            return gdb.read_register(number, width);
        }
        let mut client = self.client.lock().map_err(backend)?;
        client.hmp(try_format_command(
            14,
            format_args!("cpu {vcpu}"),
            "QEMU CPU selection",
        )?)?;
        let response = client.hmp(try_owned_text("info registers", "QEMU HMP command")?)?;
        let text = response
            .as_str()
            .ok_or_else(|| VmiError::Backend("QEMU register response was not text".into()))?;
        if let Some(value) = parse_hmp_register(text, &register) {
            return Ok(value);
        }
        Err(VmiError::Backend(format!(
            "register {register} was not reported for vCPU {vcpu}"
        )))
    }

    fn write_register(&self, vcpu: u32, register: &str, value: u64) -> Result<()> {
        let Some((number, width)) = amd64_register(register) else {
            return Err(VmiError::UnsupportedOperation {
                provider: try_owned_text(&self.descriptor.id, "QEMU provider ID")?,
                operation: "GDB register mapping",
            });
        };
        let Some(gdb) = self.gdb.as_ref() else {
            return Err(VmiError::CapabilityMissing {
                provider: try_owned_text(&self.descriptor.id, "QEMU provider ID")?,
                capability: Capability::RegisterWrite,
            });
        };
        let mut gdb = gdb.lock().map_err(backend)?;
        gdb.select_vcpu(vcpu)?;
        gdb.write_register(number, width, value)?;
        let actual = gdb.read_register(number, width)?;
        if actual != value {
            return Err(VmiError::Backend(format!(
                "GDB register write verification failed: expected {value:#x}, got {actual:#x}"
            )));
        }
        Ok(())
    }
}

impl MemoryAccess for QemuSession {
    fn read_into(&self, address: Gpa, buffer: &mut [u8]) -> Result<()> {
        const CHUNK_SIZE: usize = 256;
        let total_length = buffer.len();
        for (chunk_index, output) in buffer.chunks_mut(CHUNK_SIZE).enumerate() {
            let chunk_offset = chunk_index
                .checked_mul(CHUNK_SIZE)
                .and_then(|offset| u64::try_from(offset).ok())
                .ok_or(VmiError::ReadFailed {
                    address: address.raw(),
                    length: total_length,
                })?;
            let chunk_address =
                address
                    .raw()
                    .checked_add(chunk_offset)
                    .ok_or(VmiError::ReadFailed {
                        address: address.raw(),
                        length: total_length,
                    })?;
            let command = try_format_command(
                45,
                format_args!("xp /{}bx {chunk_address:#x}", output.len()),
                "QEMU physical-memory read",
            )?;
            let response = self.client.lock().map_err(backend)?.hmp(command)?;
            let text = response.as_str().ok_or_else(|| {
                VmiError::Backend("QEMU physical-memory response was not text".into())
            })?;
            let bytes = parse_hmp_memory(text, chunk_address, output.len())?;
            output.copy_from_slice(&bytes);
        }
        Ok(())
    }
}

fn parse_hmp_memory(text: &str, address: u64, expected: usize) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(expected).map_err(|error| {
        VmiError::Backend(format!("failed to allocate QEMU memory response: {error}"))
    })?;
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        let (_, values) = line.split_once(':').ok_or(VmiError::ReadFailed {
            address,
            length: expected,
        })?;
        for token in values.split_whitespace() {
            let hex = token
                .trim_end_matches(',')
                .strip_prefix("0x")
                .or_else(|| token.trim_end_matches(',').strip_prefix("0X"))
                .ok_or(VmiError::ReadFailed {
                    address,
                    length: expected,
                })?;
            if hex.len() != 2 || bytes.len() == expected {
                return Err(VmiError::ReadFailed {
                    address,
                    length: expected,
                });
            }
            bytes.push(
                u8::from_str_radix(hex, 16).map_err(|_| VmiError::ReadFailed {
                    address,
                    length: expected,
                })?,
            );
        }
    }
    if bytes.len() != expected {
        return Err(VmiError::ReadFailed {
            address,
            length: expected,
        });
    }
    Ok(bytes)
}

impl ControlAccess for QemuSession {
    fn execution_state(&self) -> Result<ExecutionState> {
        let value = self
            .client
            .lock()
            .map_err(backend)?
            .execute("query-status", None)?;
        Ok(match value.get("status").and_then(Value::as_str) {
            Some("running") => ExecutionState::Running,
            Some("paused" | "prelaunch" | "suspended") => ExecutionState::Paused,
            Some("shutdown") => ExecutionState::Shutdown,
            _ => ExecutionState::Unknown,
        })
    }
    fn pause(&self) -> Result<()> {
        self.client
            .lock()
            .map_err(backend)?
            .execute("stop", None)
            .map(|_| ())
    }
    fn resume(&self) -> Result<()> {
        self.client
            .lock()
            .map_err(backend)?
            .execute("cont", None)
            .map(|_| ())
    }
}

impl EventAccess for QemuSession {
    fn next_event(&self, timeout: Duration) -> Result<Option<VmiEvent>> {
        self.client.lock().map_err(backend)?.next_event(timeout)
    }
}

impl AcquisitionAccess for QemuSession {
    fn save_physical_range(&self, path: &Path, start: Gpa, length: u64) -> Result<()> {
        if length == 0 {
            return Err(VmiError::Backend(
                "acquisition length must be non-zero".into(),
            ));
        }
        start
            .raw()
            .checked_add(length)
            .ok_or_else(|| VmiError::Backend("QEMU acquisition physical range overflows".into()))?;
        let path = monitor_destination(path)?;
        let escaped_path = try_escape_monitor_path(&path)?;
        let capacity = escaped_path
            .len()
            .checked_add(53)
            .ok_or_else(|| VmiError::Backend("QEMU acquisition command length overflow".into()))?;
        let command = try_format_command(
            capacity,
            format_args!("pmemsave {} {} \"{escaped_path}\"", start.raw(), length),
            "QEMU acquisition",
        )?;
        self.client
            .lock()
            .map_err(backend)?
            .hmp(command)
            .map(|_| ())
    }

    fn save_snapshot(&self, path: &Path) -> Result<()> {
        let path = monitor_destination(path)?;
        let capacity = path
            .len()
            .checked_add(5)
            .ok_or_else(|| VmiError::Backend("QEMU dump protocol length overflow".into()))?;
        let protocol =
            try_format_command(capacity, format_args!("file:{path}"), "QEMU dump protocol")?;
        self.client
            .lock()
            .map_err(backend)?
            .execute(
                "dump-guest-memory",
                Some(json!({ "paging": false, "protocol": protocol })),
            )
            .map(|_| ())
    }
}

fn try_escape_monitor_path(path: &str) -> Result<String> {
    let extra = path
        .bytes()
        .filter(|byte| matches!(byte, b'\\' | b'"'))
        .count();
    let capacity = path
        .len()
        .checked_add(extra)
        .ok_or_else(|| VmiError::Backend("QEMU monitor path length overflow".into()))?;
    let mut escaped = String::new();
    escaped.try_reserve_exact(capacity).map_err(|error| {
        VmiError::Backend(format!(
            "failed to allocate escaped QEMU monitor path: {error}"
        ))
    })?;
    for character in path.chars() {
        if matches!(character, '\\' | '"') {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    Ok(escaped)
}

fn monitor_path(path: &Path) -> Result<String> {
    let resolved = if path.is_absolute() {
        try_owned_path(path, "QEMU acquisition")?
    } else {
        let current = std::env::current_dir().map_err(|error| {
            VmiError::Backend(format!("failed to resolve QEMU acquisition path: {error}"))
        })?;
        try_join_path(&current, path, "QEMU acquisition")?
    };
    let resolved = resolved
        .to_str()
        .ok_or_else(|| VmiError::Backend("QEMU acquisition path is not UTF-8".into()))?;
    let mut normalized = String::new();
    normalized
        .try_reserve_exact(resolved.len())
        .map_err(|error| {
            VmiError::Backend(format!("failed to allocate QEMU acquisition path: {error}"))
        })?;
    normalized.extend(resolved.chars().map(
        |character| {
            if character == '\\' {
                '/'
            } else {
                character
            }
        },
    ));
    if normalized.chars().any(char::is_control) {
        return Err(VmiError::Backend(
            "QEMU acquisition path contains control characters".into(),
        ));
    }
    Ok(normalized)
}

fn try_owned_path(path: &Path, description: &str) -> Result<PathBuf> {
    let mut output = PathBuf::new();
    output
        .try_reserve_exact(path.as_os_str().len())
        .map_err(|error| {
            VmiError::Backend(format!("failed to allocate {description} path: {error}"))
        })?;
    output.push(path);
    Ok(output)
}

fn try_join_path(parent: &Path, child: &Path, description: &str) -> Result<PathBuf> {
    let capacity = parent
        .as_os_str()
        .len()
        .checked_add(child.as_os_str().len())
        .and_then(|length| length.checked_add(1))
        .ok_or_else(|| VmiError::Backend(format!("{description} path length overflow")))?;
    let mut output = PathBuf::new();
    output.try_reserve_exact(capacity).map_err(|error| {
        VmiError::Backend(format!("failed to allocate {description} path: {error}"))
    })?;
    output.push(parent);
    output.push(child);
    Ok(output)
}

fn monitor_destination(path: &Path) -> Result<String> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => {
            return Err(VmiError::Backend(format!(
                "refusing to replace existing QEMU acquisition output {}",
                path.display()
            )))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(VmiError::Backend(format!(
                "failed to inspect QEMU acquisition output {}: {error}",
                path.display()
            )))
        }
    }
    monitor_path(path)
}

impl Session for QemuSession {
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
    fn cpu(&self) -> Result<&dyn CpuAccess> {
        Ok(self)
    }
    fn control(&self) -> Result<&dyn ControlAccess> {
        Ok(self)
    }
    fn events(&self) -> Result<&dyn EventAccess> {
        Ok(self)
    }
    fn acquisition(&self) -> Result<&dyn AcquisitionAccess> {
        Ok(self)
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use std::{
        io::{BufRead, BufReader, Write},
        net::TcpListener,
        thread,
    };

    use super::*;

    #[test]
    fn register_names_are_normalized_exactly() {
        assert_eq!(register_name("r15").unwrap(), "R15");
        assert!(register_name("r_ip").is_err());
        assert!(register_name("레지스터").is_err());
        assert_eq!(
            try_escape_monitor_path(r#"C:\guest "one".bin"#).unwrap(),
            r#"C:\\guest \"one\".bin"#
        );
        assert_eq!(gdb_thread_id(u32::MAX - 1).unwrap(), "ffffffff");
        assert!(gdb_thread_id(u32::MAX).is_err());
    }

    #[test]
    fn endpoint_names_fail_closed_before_connection() {
        assert!(QmpEndpoint::Tcp(String::new()).try_name().is_err());
        assert!(QmpEndpoint::Tcp("host:1\nignored".into())
            .try_name()
            .is_err());
        assert_eq!(
            QmpEndpoint::Tcp("127.0.0.1:4444".into())
                .try_name()
                .unwrap(),
            "127.0.0.1:4444"
        );
        assert!(validate_endpoint_text("", "test").is_err());
        assert!(validate_endpoint_text("host:1\rignored", "test").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn unix_endpoint_names_reject_non_utf8_and_controls() {
        use std::os::unix::ffi::OsStringExt;

        assert!(
            QmpEndpoint::Unix(PathBuf::from(std::ffi::OsString::from_vec(vec![0xff])))
                .try_name()
                .is_err()
        );
        assert!(QmpEndpoint::Unix(PathBuf::from("socket\nname"))
            .try_name()
            .is_err());
    }

    #[test]
    fn cloned_connector_capability_builders_are_isolated() {
        let original = QemuConnector::tcp("127.0.0.1:4444");
        let writable = original.clone().with_gdb("127.0.0.1:5555");
        assert!(!original
            .descriptor()
            .capabilities
            .contains_capability(Capability::RegisterWrite));
        assert!(writable
            .descriptor()
            .capabilities
            .contains_capability(Capability::RegisterWrite));
    }
    use vmi_driver_api::Connector;

    #[test]
    fn acquisition_paths_reject_monitor_control_characters() {
        let safe = monitor_path(Path::new("safe-output.core")).unwrap();
        assert!(Path::new(&safe).is_absolute());
        #[cfg(windows)]
        assert!(!safe.contains('\\'));
        assert!(monitor_path(Path::new("unsafe\nquit.core")).is_err());
        assert!(monitor_path(Path::new("unsafe\rquit.core")).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn acquisition_paths_reject_non_utf8_names() {
        use std::os::unix::ffi::OsStringExt;

        let path = PathBuf::from(std::ffi::OsString::from_vec(vec![b'x', 0xff]));
        assert!(monitor_path(&path).is_err());
    }

    #[test]
    fn qemu_acquisition_refuses_existing_destinations() {
        let path = std::env::temp_dir().join(format!(
            "vmi-qemu-existing-{}-{:?}.core",
            std::process::id(),
            thread::current().id()
        ));
        std::fs::write(&path, b"existing").unwrap();
        assert!(monitor_destination(&path)
            .unwrap_err()
            .to_string()
            .contains("refusing to replace"));
        assert_eq!(std::fs::read(&path).unwrap(), b"existing");
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn connector_timeout_is_nonzero_and_portably_bounded() {
        assert!(QemuConnector::tcp("127.0.0.1:1")
            .with_timeout(Duration::ZERO)
            .is_err());
        assert_eq!(
            QemuConnector::tcp("127.0.0.1:1")
                .with_timeout(Duration::MAX)
                .unwrap()
                .timeout,
            MAX_SOCKET_WAIT
        );
    }

    #[test]
    fn tcp_endpoint_name_is_owned_fallibly_and_exactly() {
        let endpoint = QmpEndpoint::Tcp("127.0.0.1:4444".into());
        assert_eq!(endpoint.try_name().unwrap(), "127.0.0.1:4444");
        assert_eq!(
            try_owned_text("display target", "test").unwrap(),
            "display target"
        );
    }

    #[test]
    fn gdb_register_codec_is_little_endian_and_validated() {
        assert_eq!(
            encode_little_hex(0x1234_5678, 8).unwrap(),
            "7856341200000000"
        );
        assert_eq!(
            decode_little_hex("7856341200000000", 8).unwrap(),
            0x1234_5678
        );
        assert!(decode_little_hex("xyz", 8).is_err());
        assert!(encode_little_hex(0, 9).is_err());
        assert!(decode_little_hex("", usize::MAX).is_err());
        assert_eq!(amd64_register("EAX"), Some((0, 8)));
        assert_eq!(amd64_register("rip"), Some((16, 8)));
        assert_eq!(amd64_register("rflags"), Some((17, 4)));
        assert_eq!(amd64_register("cr3"), None);
        assert_eq!(gdb_thread_id(0).unwrap(), "1");
        assert_eq!(gdb_thread_id(1).unwrap(), "2");
        assert!(gdb_thread_id(u32::MAX).is_err());
    }

    #[test]
    fn hmp_register_parser_accepts_legacy_instruction_and_flag_aliases() {
        let text = "EIP=0000fff0 EFL=00000002 RAX=0000000000000000";
        assert_eq!(parse_hmp_register(text, "RIP"), Some(0xfff0));
        assert_eq!(parse_hmp_register(text, "RFLAGS"), Some(2));
        assert_eq!(parse_hmp_register(text, "RAX"), Some(0));
        assert_eq!(parse_hmp_register("RAX=0X2A", "RAX"), Some(0x2a));
        assert_eq!(parse_hmp_register(text, "CR3"), None);
    }

    fn request(reader: &mut impl BufRead) -> Value {
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap()
    }

    #[test]
    fn correlates_replies_while_ignoring_async_events() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            writeln!(
                stream,
                "{}",
                json!({ "QMP": { "version": {}, "capabilities": [] } })
            )
            .unwrap();
            let capabilities = request(&mut reader);
            writeln!(stream, "{}", json!({ "event": "RESET" })).unwrap();
            writeln!(
                stream,
                "{}",
                json!({ "return": {}, "id": capabilities["id"] })
            )
            .unwrap();
            let status = request(&mut reader);
            writeln!(stream, "{}", json!({ "event": "STOP" })).unwrap();
            writeln!(
                stream,
                "{}",
                json!({ "return": { "status": "running" }, "id": status["id"] })
            )
            .unwrap();
        });
        let connector = QemuConnector::tcp(address.to_string());
        let session = connector.connect(AttachRequest::default()).unwrap();
        assert_eq!(
            session.control().unwrap().execution_state().unwrap(),
            ExecutionState::Running
        );
        assert_eq!(
            session
                .events()
                .unwrap()
                .next_event(Duration::ZERO)
                .unwrap()
                .unwrap()
                .kind,
            "RESET"
        );
        assert_eq!(
            session
                .events()
                .unwrap()
                .next_event(Duration::ZERO)
                .unwrap()
                .unwrap()
                .kind,
            "STOP"
        );
        server.join().unwrap();
    }

    #[test]
    fn maps_qmp_event_metadata() {
        let event = qmp_event(&json!({
            "event": "MEMORY_FAILURE",
            "data": { "cpu-index": 3, "address": 0x1234 }
        }))
        .unwrap();
        let event = event.unwrap();
        assert_eq!(event.kind, "MEMORY_FAILURE");
        assert_eq!(event.vcpu, Some(3));
        assert_eq!(event.address, Some(Gpa::new(0x1234)));
        assert!(qmp_event(&json!({ "return": {} })).unwrap().is_none());
    }

    #[test]
    fn bounds_pending_qmp_events() {
        let mut events = VecDeque::new();
        for index in 0..EVENT_QUEUE_CAPACITY {
            enqueue_event(
                &mut events,
                VmiEvent {
                    kind: format!("event-{index}"),
                    vcpu: None,
                    address: None,
                },
            )
            .unwrap();
        }
        assert!(enqueue_event(
            &mut events,
            VmiEvent {
                kind: "overflow".into(),
                vcpu: None,
                address: None,
            }
        )
        .is_err());
        assert_eq!(events.len(), EVENT_QUEUE_CAPACITY);
    }

    #[test]
    fn request_ids_use_final_value_then_remain_exhausted() {
        let mut next_id = Some(u64::MAX - 1);
        assert_eq!(take_request_id(&mut next_id).unwrap(), u64::MAX - 1);
        assert_eq!(take_request_id(&mut next_id).unwrap(), u64::MAX);
        assert!(take_request_id(&mut next_id).is_err());
        assert!(next_id.is_none());
    }

    #[test]
    fn qmp_return_value_is_moved_out_of_owned_response() {
        let mut response = json!({
            "id": 1,
            "return": { "nested": [1, 2, 3] }
        });
        assert_eq!(
            take_qmp_return(&mut response),
            json!({ "nested": [1, 2, 3] })
        );
        assert!(response.get("return").is_none());
        assert_eq!(take_qmp_return(&mut response), Value::Null);
    }

    #[cfg(unix)]
    #[test]
    fn connects_over_unix_domain_socket() {
        use std::os::unix::net::UnixListener;

        let path = std::env::temp_dir().join(format!("vmi-qmp-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path).unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            writeln!(stream, "{}", json!({ "QMP": {} })).unwrap();
            let capabilities = request(&mut reader);
            writeln!(
                stream,
                "{}",
                json!({ "return": {}, "id": capabilities["id"] })
            )
            .unwrap();
            let status = request(&mut reader);
            writeln!(
                stream,
                "{}",
                json!({ "return": { "status": "paused" }, "id": status["id"] })
            )
            .unwrap();
        });
        let connector = QemuConnector::unix(&path);
        let session = connector.connect(AttachRequest::default()).unwrap();
        assert_eq!(
            session.control().unwrap().execution_state().unwrap(),
            ExecutionState::Paused
        );
        server.join().unwrap();
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn surfaces_qmp_errors_and_timeouts() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            writeln!(stream, "{}", json!({ "QMP": {} })).unwrap();
            let capabilities = request(&mut reader);
            writeln!(stream, "{}", json!({ "error": { "class": "GenericError", "desc": "denied" }, "id": capabilities["id"] })).unwrap();
        });
        assert!(QemuConnector::tcp(address.to_string())
            .connect(AttachRequest::default())
            .is_err());
        server.join().unwrap();

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            writeln!(stream, "{}", json!({ "QMP": {} })).unwrap();
            thread::sleep(Duration::from_millis(100));
        });
        let connector = QemuConnector::tcp(address.to_string())
            .with_timeout(Duration::from_millis(20))
            .unwrap();
        assert!(connector.connect(AttachRequest::default()).is_err());
        server.join().unwrap();
        assert!(QemuConnector::tcp("127.0.0.1:1")
            .with_timeout(Duration::ZERO)
            .is_err());
    }

    #[test]
    fn qmp_framing_rejects_small_oversized_and_unterminated_messages() {
        for message in [b"12345\n".as_slice(), b"1234".as_slice()] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let message = message.to_vec();
            let server = thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                stream.write_all(&message).unwrap();
            });
            let stream = TcpStream::connect(address).unwrap();
            let mut client = QmpClient {
                stream: BufReader::new(QmpStream::Tcp(stream)),
                next_id: Some(1),
                events: VecDeque::new(),
                timeout: Duration::from_secs(1),
            };
            assert!(client
                .read_json_with_capacity(4)
                .unwrap_err()
                .to_string()
                .contains("exceeds 4 bytes"));
            server.join().unwrap();
        }
    }

    #[test]
    fn qmp_timeout_errors_are_platform_independent() {
        for kind in [std::io::ErrorKind::TimedOut, std::io::ErrorKind::WouldBlock] {
            assert!(matches!(
                qmp_read_error(std::io::Error::from(kind)),
                VmiError::Timeout {
                    operation: "QMP read"
                }
            ));
        }
    }

    #[test]
    fn hmp_memory_parser_is_exact_and_fail_closed() {
        assert_eq!(
            parse_hmp_memory("0000000000001000: 0x01 0xA2,\n0x1002: 0xff\n", 0x1000, 3).unwrap(),
            [0x01, 0xa2, 0xff]
        );
        for malformed in [
            "QEMU monitor error",
            "0x1000: 0x01 garbage",
            "0x1000: 0x1",
            "0x1000: 0xgg",
            "0x1000: 0x01 0x02",
        ] {
            assert!(matches!(
                parse_hmp_memory(malformed, 0x1000, 1),
                Err(VmiError::ReadFailed {
                    address: 0x1000,
                    length: 1
                })
            ));
        }
    }

    #[test]
    fn oversized_event_timeout_waits_for_qmp_input() {
        assert_eq!(bounded_socket_timeout(Duration::MAX), MAX_SOCKET_WAIT);
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            thread::sleep(Duration::from_millis(10));
            writeln!(stream, "{}", json!({ "event": "RESUME" })).unwrap();
        });
        let stream = TcpStream::connect(address).unwrap();
        let mut client = QmpClient {
            stream: BufReader::new(QmpStream::Tcp(stream)),
            next_id: Some(1),
            events: VecDeque::new(),
            timeout: Duration::MAX,
        };
        assert_eq!(
            client.next_event(Duration::MAX).unwrap().unwrap().kind,
            "RESUME"
        );
        server.join().unwrap();
    }

    #[test]
    fn qmp_command_timeout_is_end_to_end_across_unrelated_replies() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = String::new();
            BufReader::new(stream.try_clone().unwrap())
                .read_line(&mut request)
                .unwrap();
            for wrong_id in 100..1100 {
                thread::sleep(Duration::from_millis(5));
                if writeln!(stream, "{}", json!({ "return": {}, "id": wrong_id })).is_err() {
                    break;
                }
            }
        });
        let stream = TcpStream::connect(address).unwrap();
        let mut client = QmpClient {
            stream: BufReader::new(QmpStream::Tcp(stream)),
            next_id: Some(1),
            events: VecDeque::new(),
            timeout: Duration::from_millis(20),
        };
        let started = Instant::now();
        assert!(matches!(
            client.execute("query-status", None),
            Err(VmiError::Timeout {
                operation: "QMP command"
            })
        ));
        assert!(started.elapsed() < Duration::from_secs(1));
        drop(client);
        server.join().unwrap();
    }

    #[test]
    fn gdb_command_timeout_is_end_to_end_across_slow_bytes() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0u8; 1];
            stream.read_exact(&mut request).unwrap();
            for byte in b"+$1234567890#00" {
                thread::sleep(Duration::from_millis(5));
                if stream.write_all(&[*byte]).is_err() {
                    break;
                }
            }
        });
        let stream = TcpStream::connect(address).unwrap();
        let mut client = GdbClient {
            stream,
            timeout: Duration::from_millis(20),
        };
        assert!(matches!(
            client.command("p0"),
            Err(VmiError::Timeout { .. })
        ));
        server.join().unwrap();
    }

    proptest! {
        #[test]
        fn gdb_register_codec_round_trips_supported_widths(
            value in any::<u64>(),
            width in 0usize..=8,
        ) {
            let encoded = encode_little_hex(value, width).unwrap();
            let decoded = decode_little_hex(&encoded, width).unwrap();
            let expected = if width == 8 {
                value
            } else if width == 0 {
                0
            } else {
                value & ((1u64 << (width * 8)) - 1)
            };
            prop_assert_eq!(decoded, expected);
        }

        #[test]
        fn private_qemu_text_decoders_fail_without_panicking(
            text in any::<String>(),
            register in any::<String>(),
            width in 0usize..=16,
        ) {
            let _ = parse_hmp_register(&text, &register);
            let _ = decode_little_hex(&text, width);
            let _ = register_name(&register);
        }
    }
}
