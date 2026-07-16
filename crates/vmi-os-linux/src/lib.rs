use std::{
    collections::HashSet,
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
};

use vmi_arch_api::AddressTranslator;
use vmi_core::VmiSession;
use vmi_profile::SymbolTable;
use vmi_types::{Gva, Result, TranslationRoot, VmiError};

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct LinuxTaskOffsets {
    pub tasks: u64,
    pub pid: u64,
    pub comm: u64,
    pub comm_length: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LinuxProcess {
    pub task: Gva,
    pub pid: u32,
    pub command: String,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct LinuxModuleOffsets {
    pub list: u64,
    pub name: u64,
    pub name_length: usize,
    pub core_base: u64,
    pub core_size: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LinuxModule {
    pub module: Gva,
    pub name: String,
    pub core_base: Gva,
    pub core_size: u64,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct LinuxFileOffsets {
    pub task_files: u64,
    pub files_fdt: u64,
    pub fdtable_max_fds: u64,
    pub fdtable_fd: u64,
    pub file_path: u64,
    pub path_dentry: u64,
    pub dentry_name: u64,
    pub qstr_length: u64,
    pub qstr_name: u64,
    pub maximum_name_bytes: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LinuxOpenFile {
    pub descriptor: u32,
    pub file: Gva,
    pub dentry: Gva,
    pub name: String,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct LinuxDentryOffsets {
    pub parent: u64,
    pub name: u64,
    pub qstr_length: u64,
    pub qstr_name: u64,
    pub maximum_name_bytes: usize,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct LinuxMountOffsets {
    pub parent: u64,
    pub mountpoint: u64,
    pub root: u64,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct LinuxSocketOffsets {
    pub file_private_data: u64,
    pub socket_sk: u64,
    pub sock_family: u64,
    pub sock_protocol: u64,
    pub sock_state: u64,
    pub ipv4_source: u64,
    pub ipv4_destination: u64,
    pub ipv6_source: u64,
    pub ipv6_destination: u64,
    pub source_port: u64,
    pub destination_port: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LinuxSocket {
    pub file: Gva,
    pub socket: Gva,
    pub sock: Gva,
    pub family: u16,
    pub protocol: u8,
    pub state: u8,
    pub source: IpAddr,
    pub source_port: u16,
    pub destination: IpAddr,
    pub destination_port: u16,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct LinuxSocketListOffsets {
    pub node_next: u64,
    pub node_sock: u64,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct LinuxSocketHashOffsets {
    pub bucket_first: u64,
    pub bucket_stride: u64,
    pub node_next: u64,
    pub node_sock: u64,
}

pub struct LinuxIntrospector<'a> {
    session: &'a VmiSession,
    translator: &'a dyn AddressTranslator,
    root: TranslationRoot,
    profile: &'a SymbolTable,
    offsets: LinuxTaskOffsets,
}

impl<'a> LinuxIntrospector<'a> {
    pub fn new(
        session: &'a VmiSession,
        translator: &'a dyn AddressTranslator,
        root: TranslationRoot,
        profile: &'a SymbolTable,
        offsets: LinuxTaskOffsets,
    ) -> Self {
        Self {
            session,
            translator,
            root,
            profile,
            offsets,
        }
    }

    pub fn processes(&self, limit: usize) -> Result<Vec<LinuxProcess>> {
        if limit == 0 || self.offsets.comm_length == 0 || self.offsets.comm_length > 4096 {
            return Err(VmiError::Backend(
                "invalid Linux traversal limit or comm length".into(),
            ));
        }
        let init = self
            .profile
            .symbol("init_task")
            .ok_or_else(|| VmiError::Backend("profile does not contain init_task".into()))?
            .address;
        let head = init
            .checked_add(self.offsets.tasks)
            .ok_or_else(|| VmiError::Backend("init_task list address overflow".into()))?;
        let mut current_task = init;
        let mut seen = HashSet::new();
        let mut output = Vec::new();
        loop {
            reserve_seen(&mut seen, "Linux task cycle detector")?;
            if !seen.insert(current_task) {
                return Err(VmiError::Backend(format!(
                    "Linux task list looped at unexpected task {current_task:#x}"
                )));
            }
            reserve_one(&mut output, "Linux process list")?;
            output.push(self.read_process(current_task)?);
            let list_node = current_task
                .checked_add(self.offsets.tasks)
                .ok_or_else(|| VmiError::Backend("task list address overflow".into()))?;
            let next_node = self.read_u64(list_node)?;
            if next_node == head {
                break;
            }
            if output.len() >= limit {
                return Err(VmiError::Backend(format!(
                    "Linux task list exceeded limit {limit}"
                )));
            }
            current_task = next_node.checked_sub(self.offsets.tasks).ok_or_else(|| {
                VmiError::Backend(format!("invalid Linux task list pointer {next_node:#x}"))
            })?;
        }
        Ok(output)
    }

    pub fn modules(&self, offsets: LinuxModuleOffsets, limit: usize) -> Result<Vec<LinuxModule>> {
        if limit == 0 || offsets.name_length == 0 || offsets.name_length > 4096 {
            return Err(VmiError::Backend(
                "invalid Linux module traversal limit or name length".into(),
            ));
        }
        let head = self
            .profile
            .symbol("modules")
            .ok_or_else(|| VmiError::Backend("profile does not contain modules".into()))?
            .address;
        let mut node = self.read_u64(head)?;
        let mut seen = HashSet::new();
        let mut output = Vec::new();
        while node != head {
            reserve_seen(&mut seen, "Linux module cycle detector")?;
            if !seen.insert(node) {
                return Err(VmiError::Backend(format!(
                    "Linux module list looped at unexpected node {node:#x}"
                )));
            }
            let module = node.checked_sub(offsets.list).ok_or_else(|| {
                VmiError::Backend(format!("invalid Linux module list pointer {node:#x}"))
            })?;
            reserve_one(&mut output, "Linux module list")?;
            output.push(self.read_module(module, offsets)?);
            node = self.read_u64(node)?;
            if node != head && output.len() >= limit {
                return Err(VmiError::Backend(format!(
                    "Linux module list exceeded limit {limit}"
                )));
            }
        }
        Ok(output)
    }

    pub fn open_files(
        &self,
        task: Gva,
        offsets: LinuxFileOffsets,
        descriptor_limit: usize,
    ) -> Result<Vec<LinuxOpenFile>> {
        if descriptor_limit == 0
            || offsets.maximum_name_bytes == 0
            || offsets.maximum_name_bytes > 65_536
        {
            return Err(VmiError::Backend(
                "invalid Linux file descriptor or name limit".into(),
            ));
        }
        let files = self.read_u64(add(task.raw(), offsets.task_files, "task files")?)?;
        if files == 0 {
            return Ok(Vec::new());
        }
        let fdt = self.read_u64(add(files, offsets.files_fdt, "files fdt")?)?;
        if fdt == 0 {
            return Err(VmiError::Backend("Linux files_struct has null fdt".into()));
        }
        let max_fds = usize::try_from(self.read_u32(add(
            fdt,
            offsets.fdtable_max_fds,
            "fdtable max_fds",
        )?)?)
        .map_err(|_| VmiError::Backend("Linux fdtable size does not fit this host".into()))?;
        if max_fds > descriptor_limit {
            return Err(VmiError::Backend(format!(
                "Linux fdtable size {max_fds} exceeds limit {descriptor_limit}"
            )));
        }
        let fd_array = self.read_u64(add(fdt, offsets.fdtable_fd, "fdtable fd")?)?;
        if max_fds != 0 && fd_array == 0 {
            return Err(VmiError::Backend(
                "non-empty Linux fdtable has null fd array".into(),
            ));
        }
        let mut output = Vec::new();
        for descriptor in 0..max_fds {
            let descriptor_u32 = u32::try_from(descriptor)
                .map_err(|_| VmiError::Backend("Linux file descriptor exceeds u32".into()))?;
            let slot = fd_array
                .checked_add(u64::from(descriptor_u32).checked_mul(8).ok_or_else(|| {
                    VmiError::Backend("Linux file descriptor slot overflow".into())
                })?)
                .ok_or_else(|| VmiError::Backend("Linux fd array overflow".into()))?;
            let file = self.read_u64(slot)?;
            if file == 0 {
                continue;
            }
            let path = add(file, offsets.file_path, "file path")?;
            let dentry = self.read_u64(add(path, offsets.path_dentry, "path dentry")?)?;
            if dentry == 0 {
                return Err(VmiError::Backend(format!(
                    "Linux file descriptor {descriptor} has null dentry"
                )));
            }
            let qstr = add(dentry, offsets.dentry_name, "dentry name")?;
            let name_length =
                usize::try_from(self.read_u32(add(qstr, offsets.qstr_length, "qstr length")?)?)
                    .map_err(|_| {
                        VmiError::Backend("Linux dentry name length is too large".into())
                    })?;
            if name_length > offsets.maximum_name_bytes {
                return Err(VmiError::Backend(format!(
                    "Linux dentry name length {name_length} exceeds limit {}",
                    offsets.maximum_name_bytes
                )));
            }
            let name_pointer = self.read_u64(add(qstr, offsets.qstr_name, "qstr name")?)?;
            if name_length != 0 && name_pointer == 0 {
                return Err(VmiError::Backend(
                    "non-empty Linux dentry name has null pointer".into(),
                ));
            }
            let bytes = if name_length == 0 {
                Vec::new()
            } else {
                self.session.read_virtual(
                    self.translator,
                    self.root,
                    Gva::new(name_pointer),
                    name_length,
                )?
            };
            reserve_one(&mut output, "Linux open-file list")?;
            output.push(LinuxOpenFile {
                descriptor: descriptor_u32,
                file: Gva::new(file),
                dentry: Gva::new(dentry),
                name: decode_guest_bytes(bytes, "Linux open-file name")?,
            });
        }
        Ok(output)
    }

    pub fn dentry_path(
        &self,
        dentry: Gva,
        offsets: LinuxDentryOffsets,
        component_limit: usize,
    ) -> Result<String> {
        if dentry.raw() == 0
            || component_limit == 0
            || offsets.maximum_name_bytes == 0
            || offsets.maximum_name_bytes > 65_536
        {
            return Err(VmiError::Backend(
                "invalid Linux dentry path arguments".into(),
            ));
        }
        let mut current = dentry.raw();
        let mut seen = HashSet::new();
        let mut components = Vec::new();
        loop {
            reserve_seen(&mut seen, "Linux dentry cycle detector")?;
            if !seen.insert(current) {
                return Err(VmiError::Backend(format!(
                    "Linux dentry ancestry looped at {current:#x}"
                )));
            }
            let qstr = add(current, offsets.name, "dentry name")?;
            let length =
                usize::try_from(self.read_u32(add(qstr, offsets.qstr_length, "qstr length")?)?)
                    .map_err(|_| {
                        VmiError::Backend("Linux dentry name length is too large".into())
                    })?;
            if length > offsets.maximum_name_bytes {
                return Err(VmiError::Backend(format!(
                    "Linux dentry name length {length} exceeds limit {}",
                    offsets.maximum_name_bytes
                )));
            }
            let pointer = self.read_u64(add(qstr, offsets.qstr_name, "qstr name")?)?;
            if length != 0 && pointer == 0 {
                return Err(VmiError::Backend(
                    "non-empty Linux dentry name has null pointer".into(),
                ));
            }
            if length != 0 {
                let bytes = self.session.read_virtual(
                    self.translator,
                    self.root,
                    Gva::new(pointer),
                    length,
                )?;
                let name = decode_guest_bytes(bytes, "Linux dentry name")?;
                if name != "/" {
                    reserve_one(&mut components, "Linux dentry path")?;
                    components.push(name);
                }
            }
            let parent = self.read_u64(add(current, offsets.parent, "dentry parent")?)?;
            if parent == 0 {
                return Err(VmiError::Backend(format!(
                    "Linux dentry {current:#x} has null parent"
                )));
            }
            if parent == current {
                break;
            }
            if components.len() >= component_limit {
                return Err(VmiError::Backend(format!(
                    "Linux dentry path exceeded component limit {component_limit}"
                )));
            }
            current = parent;
        }
        components.reverse();
        join_absolute_path(&components, "Linux dentry path")
    }

    pub fn mounted_path(
        &self,
        dentry: Gva,
        mount: Gva,
        dentry_offsets: LinuxDentryOffsets,
        mount_offsets: LinuxMountOffsets,
        component_limit: usize,
    ) -> Result<String> {
        if dentry.raw() == 0
            || mount.raw() == 0
            || component_limit == 0
            || dentry_offsets.maximum_name_bytes == 0
            || dentry_offsets.maximum_name_bytes > 65_536
        {
            return Err(VmiError::Backend(
                "invalid Linux mounted path arguments".into(),
            ));
        }
        let mut current_dentry = dentry.raw();
        let mut current_mount = mount.raw();
        let mut seen = HashSet::new();
        let mut components = Vec::new();
        loop {
            reserve_seen(&mut seen, "Linux mount cycle detector")?;
            if !seen.insert((current_dentry, current_mount)) {
                return Err(VmiError::Backend(format!(
                    "Linux mounted path looped at dentry {current_dentry:#x}, mount {current_mount:#x}"
                )));
            }
            let mount_root =
                self.read_u64(add(current_mount, mount_offsets.root, "mount root")?)?;
            if mount_root == 0 {
                return Err(VmiError::Backend("Linux mount has null root".into()));
            }
            if current_dentry == mount_root {
                let parent_mount =
                    self.read_u64(add(current_mount, mount_offsets.parent, "mount parent")?)?;
                if parent_mount == 0 {
                    return Err(VmiError::Backend("Linux mount has null parent".into()));
                }
                if parent_mount == current_mount {
                    break;
                }
                current_dentry =
                    self.read_u64(add(current_mount, mount_offsets.mountpoint, "mountpoint")?)?;
                if current_dentry == 0 {
                    return Err(VmiError::Backend("Linux mount has null mountpoint".into()));
                }
                current_mount = parent_mount;
                continue;
            }
            reserve_one(&mut components, "Linux mounted path")?;
            components.push(self.read_dentry_name(current_dentry, dentry_offsets)?);
            if components.len() >= component_limit {
                return Err(VmiError::Backend(format!(
                    "Linux mounted path exceeded component limit {component_limit}"
                )));
            }
            let parent =
                self.read_u64(add(current_dentry, dentry_offsets.parent, "dentry parent")?)?;
            if parent == 0 {
                return Err(VmiError::Backend(format!(
                    "Linux dentry {current_dentry:#x} has null parent"
                )));
            }
            current_dentry = parent;
        }
        components.retain(|component| !component.is_empty() && component != "/");
        components.reverse();
        join_absolute_path(&components, "Linux mounted path")
    }

    pub fn socket_from_file(&self, file: Gva, offsets: LinuxSocketOffsets) -> Result<LinuxSocket> {
        if file.raw() == 0 {
            return Err(VmiError::Backend("Linux file pointer is null".into()));
        }
        let socket = self.read_u64(add(file.raw(), offsets.file_private_data, "socket")?)?;
        if socket == 0 {
            return Err(VmiError::Backend(
                "Linux file has null private socket data".into(),
            ));
        }
        let sock = self.read_u64(add(socket, offsets.socket_sk, "socket sk")?)?;
        if sock == 0 {
            return Err(VmiError::Backend("Linux socket has null sock".into()));
        }
        self.decode_sock(file, Gva::new(socket), Gva::new(sock), offsets)
    }

    pub fn sockets_from_list(
        &self,
        head: Gva,
        list_offsets: LinuxSocketListOffsets,
        socket_offsets: LinuxSocketOffsets,
        limit: usize,
    ) -> Result<Vec<LinuxSocket>> {
        if head.raw() == 0 || limit == 0 {
            return Err(VmiError::Backend(
                "invalid Linux socket-list arguments".into(),
            ));
        }
        let mut node = self.read_u64(head.raw())?;
        let mut seen = HashSet::new();
        let mut output = Vec::new();
        while node != head.raw() {
            reserve_seen(&mut seen, "Linux socket cycle detector")?;
            if !seen.insert(node) {
                return Err(VmiError::Backend(format!(
                    "Linux socket list looped at unexpected node {node:#x}"
                )));
            }
            let sock = self.read_u64(add(node, list_offsets.node_sock, "socket-list sock")?)?;
            if sock == 0 {
                return Err(VmiError::Backend(format!(
                    "Linux socket-list node {node:#x} has null sock"
                )));
            }
            reserve_one(&mut output, "Linux socket list")?;
            output.push(self.decode_sock(
                Gva::new(0),
                Gva::new(0),
                Gva::new(sock),
                socket_offsets,
            )?);
            node = self.read_u64(add(node, list_offsets.node_next, "socket-list next")?)?;
            if node != head.raw() && output.len() >= limit {
                return Err(VmiError::Backend(format!(
                    "Linux socket list exceeded limit {limit}"
                )));
            }
        }
        Ok(output)
    }

    pub fn sockets_from_hash_table(
        &self,
        table: Gva,
        bucket_count: usize,
        hash_offsets: LinuxSocketHashOffsets,
        socket_offsets: LinuxSocketOffsets,
        socket_limit: usize,
    ) -> Result<Vec<LinuxSocket>> {
        if table.raw() == 0
            || bucket_count == 0
            || bucket_count > 1_048_576
            || hash_offsets.bucket_stride == 0
            || socket_limit == 0
        {
            return Err(VmiError::Backend(
                "invalid Linux socket hash-table arguments".into(),
            ));
        }
        let mut seen = HashSet::new();
        let mut output = Vec::new();
        for bucket in 0..bucket_count {
            let bucket = u64::try_from(bucket)
                .map_err(|_| VmiError::Backend("Linux socket bucket index is too large".into()))?;
            let bucket_address =
                table
                    .raw()
                    .checked_add(bucket.checked_mul(hash_offsets.bucket_stride).ok_or_else(
                        || VmiError::Backend("Linux socket bucket offset overflow".into()),
                    )?)
                    .and_then(|address| address.checked_add(hash_offsets.bucket_first))
                    .ok_or_else(|| VmiError::Backend("Linux socket bucket overflow".into()))?;
            let mut node = self.read_u64(bucket_address)?;
            while node != 0 {
                reserve_seen(&mut seen, "Linux socket hash cycle detector")?;
                if !seen.insert(node) {
                    return Err(VmiError::Backend(format!(
                        "Linux socket hash table repeats node {node:#x}"
                    )));
                }
                let sock = self.read_u64(add(node, hash_offsets.node_sock, "hash-node sock")?)?;
                if sock == 0 {
                    return Err(VmiError::Backend(format!(
                        "Linux socket hash node {node:#x} has null sock"
                    )));
                }
                reserve_one(&mut output, "Linux socket hash table")?;
                output.push(self.decode_sock(
                    Gva::new(0),
                    Gva::new(0),
                    Gva::new(sock),
                    socket_offsets,
                )?);
                if output.len() > socket_limit {
                    return Err(VmiError::Backend(format!(
                        "Linux socket hash table exceeded limit {socket_limit}"
                    )));
                }
                node = self.read_u64(add(node, hash_offsets.node_next, "hash-node next")?)?;
            }
        }
        Ok(output)
    }

    fn decode_sock(
        &self,
        file: Gva,
        socket: Gva,
        sock_address: Gva,
        offsets: LinuxSocketOffsets,
    ) -> Result<LinuxSocket> {
        let sock = sock_address.raw();
        let family = self.read_u16(add(sock, offsets.sock_family, "socket family")?)?;
        let protocol = self.read_u8(add(sock, offsets.sock_protocol, "socket protocol")?)?;
        let state = self.read_u8(add(sock, offsets.sock_state, "socket state")?)?;
        let (source, destination) = match family {
            2 => (
                IpAddr::V4(Ipv4Addr::from(self.read_array::<4>(add(
                    sock,
                    offsets.ipv4_source,
                    "IPv4 source",
                )?)?)),
                IpAddr::V4(Ipv4Addr::from(self.read_array::<4>(add(
                    sock,
                    offsets.ipv4_destination,
                    "IPv4 destination",
                )?)?)),
            ),
            10 => (
                IpAddr::V6(Ipv6Addr::from(self.read_array::<16>(add(
                    sock,
                    offsets.ipv6_source,
                    "IPv6 source",
                )?)?)),
                IpAddr::V6(Ipv6Addr::from(self.read_array::<16>(add(
                    sock,
                    offsets.ipv6_destination,
                    "IPv6 destination",
                )?)?)),
            ),
            _ => {
                return Err(VmiError::Backend(format!(
                    "unsupported Linux socket family {family}"
                )))
            }
        };
        let source_port = u16::from_be_bytes(self.read_array::<2>(add(
            sock,
            offsets.source_port,
            "socket source port",
        )?)?);
        let destination_port = u16::from_be_bytes(self.read_array::<2>(add(
            sock,
            offsets.destination_port,
            "socket destination port",
        )?)?);
        Ok(LinuxSocket {
            file,
            socket,
            sock: sock_address,
            family,
            protocol,
            state,
            source,
            source_port,
            destination,
            destination_port,
        })
    }

    fn read_process(&self, task: u64) -> Result<LinuxProcess> {
        let pid = self.read_u32(
            task.checked_add(self.offsets.pid)
                .ok_or_else(|| VmiError::Backend("PID address overflow".into()))?,
        )?;
        let comm_address = task
            .checked_add(self.offsets.comm)
            .ok_or_else(|| VmiError::Backend("comm address overflow".into()))?;
        let bytes = self.session.read_virtual(
            self.translator,
            self.root,
            Gva::new(comm_address),
            self.offsets.comm_length,
        )?;
        let end = bytes
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(bytes.len());
        let mut bytes = bytes;
        bytes.truncate(end);
        let command = decode_guest_bytes(bytes, "Linux command name")?;
        Ok(LinuxProcess {
            task: Gva::new(task),
            pid,
            command,
        })
    }

    fn read_module(&self, module: u64, offsets: LinuxModuleOffsets) -> Result<LinuxModule> {
        let name_address = add(module, offsets.name, "module name")?;
        let bytes = self.session.read_virtual(
            self.translator,
            self.root,
            Gva::new(name_address),
            offsets.name_length,
        )?;
        let end = bytes
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(bytes.len());
        let mut bytes = bytes;
        bytes.truncate(end);
        let name = decode_guest_bytes(bytes, "Linux module name")?;
        let core_base = self.read_u64(add(module, offsets.core_base, "module core base")?)?;
        let core_size = self.read_u64(add(module, offsets.core_size, "module core size")?)?;
        Ok(LinuxModule {
            module: Gva::new(module),
            name,
            core_base: Gva::new(core_base),
            core_size,
        })
    }

    fn read_u32(&self, address: u64) -> Result<u32> {
        let bytes = self
            .session
            .read_virtual(self.translator, self.root, Gva::new(address), 4)?;
        Ok(u32::from_le_bytes(
            bytes
                .try_into()
                .map_err(|_| VmiError::Backend("short PID read".into()))?,
        ))
    }
    fn read_u8(&self, address: u64) -> Result<u8> {
        Ok(self.read_array::<1>(address)?[0])
    }
    fn read_dentry_name(&self, dentry: u64, offsets: LinuxDentryOffsets) -> Result<String> {
        let qstr = add(dentry, offsets.name, "dentry name")?;
        let length =
            usize::try_from(self.read_u32(add(qstr, offsets.qstr_length, "qstr length")?)?)
                .map_err(|_| VmiError::Backend("Linux dentry name length is too large".into()))?;
        if length > offsets.maximum_name_bytes {
            return Err(VmiError::Backend(format!(
                "Linux dentry name length {length} exceeds limit {}",
                offsets.maximum_name_bytes
            )));
        }
        let pointer = self.read_u64(add(qstr, offsets.qstr_name, "qstr name")?)?;
        if length != 0 && pointer == 0 {
            return Err(VmiError::Backend(
                "non-empty Linux dentry name has null pointer".into(),
            ));
        }
        if length == 0 {
            return Ok(String::new());
        }
        let bytes =
            self.session
                .read_virtual(self.translator, self.root, Gva::new(pointer), length)?;
        decode_guest_bytes(bytes, "Linux dentry name")
    }
    fn read_u16(&self, address: u64) -> Result<u16> {
        Ok(u16::from_le_bytes(self.read_array::<2>(address)?))
    }
    fn read_array<const N: usize>(&self, address: u64) -> Result<[u8; N]> {
        self.session
            .read_virtual(self.translator, self.root, Gva::new(address), N)?
            .try_into()
            .map_err(|_| VmiError::Backend(format!("short Linux {N}-byte read")))
    }
    fn read_u64(&self, address: u64) -> Result<u64> {
        let bytes = self
            .session
            .read_virtual(self.translator, self.root, Gva::new(address), 8)?;
        Ok(u64::from_le_bytes(bytes.try_into().map_err(|_| {
            VmiError::Backend("short pointer read".into())
        })?))
    }
}

fn decode_guest_bytes(bytes: Vec<u8>, description: &str) -> Result<String> {
    let bytes = match String::from_utf8(bytes) {
        Ok(text) => return Ok(text),
        Err(error) => error.into_bytes(),
    };
    let capacity = bytes
        .len()
        .checked_mul(3)
        .ok_or_else(|| VmiError::Backend(format!("{description} decoded length overflow")))?;
    let mut output = String::new();
    output.try_reserve_exact(capacity).map_err(|error| {
        VmiError::Backend(format!("failed to allocate decoded {description}: {error}"))
    })?;
    let mut offset = 0usize;
    while offset < bytes.len() {
        let remaining = bytes.get(offset..).ok_or_else(|| {
            VmiError::Backend(format!("{description} decoder offset is out of bounds"))
        })?;
        match std::str::from_utf8(remaining) {
            Ok(valid) => {
                output.push_str(valid);
                break;
            }
            Err(error) => {
                let valid_length = error.valid_up_to();
                let valid = remaining.get(..valid_length).ok_or_else(|| {
                    VmiError::Backend(format!("{description} valid prefix is out of bounds"))
                })?;
                let valid = std::str::from_utf8(valid).map_err(|error| {
                    VmiError::Backend(format!("{description} valid prefix failed: {error}"))
                })?;
                output.push_str(valid);
                output.push(char::REPLACEMENT_CHARACTER);
                let Some(error_length) = error.error_len() else {
                    break;
                };
                offset = offset
                    .checked_add(valid_length)
                    .and_then(|value| value.checked_add(error_length))
                    .ok_or_else(|| {
                        VmiError::Backend(format!("{description} decoder progress overflow"))
                    })?;
            }
        }
    }
    Ok(output)
}

fn join_absolute_path(components: &[String], description: &str) -> Result<String> {
    let separators = components.len().saturating_sub(1);
    let capacity = components.iter().try_fold(
        1usize
            .checked_add(separators)
            .ok_or_else(|| VmiError::Backend(format!("{description} length overflow")))?,
        |length, component| {
            length
                .checked_add(component.len())
                .ok_or_else(|| VmiError::Backend(format!("{description} length overflow")))
        },
    )?;
    let mut path = String::new();
    path.try_reserve_exact(capacity)
        .map_err(|error| VmiError::Backend(format!("failed to allocate {description}: {error}")))?;
    path.push('/');
    for (index, component) in components.iter().enumerate() {
        if index != 0 {
            path.push('/');
        }
        path.push_str(component);
    }
    Ok(path)
}

fn add(base: u64, offset: u64, field: &str) -> Result<u64> {
    base.checked_add(offset)
        .ok_or_else(|| VmiError::Backend(format!("Linux {field} address overflow")))
}

fn reserve_one<T>(values: &mut Vec<T>, description: &str) -> Result<()> {
    values
        .try_reserve(1)
        .map_err(|error| VmiError::Backend(format!("failed to grow {description}: {error}")))
}

fn reserve_seen<T: Eq + std::hash::Hash>(values: &mut HashSet<T>, description: &str) -> Result<()> {
    values
        .try_reserve(1)
        .map_err(|error| VmiError::Backend(format!("failed to grow {description}: {error}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use vmi_arch_api::Translation;
    use vmi_driver_api::MemoryAccess;
    use vmi_testkit::FakeConnector;
    use vmi_types::{AttachRequest, Capability, CapabilitySet, Gpa};

    #[test]
    fn guest_byte_decoder_matches_lossy_utf8_semantics() {
        for bytes in [
            b"plain".to_vec(),
            "snowman ☃".as_bytes().to_vec(),
            vec![0xf0, 0x28, 0x8c, 0x28],
            vec![0xe2, 0x82],
        ] {
            let expected = String::from_utf8_lossy(&bytes).into_owned();
            assert_eq!(decode_guest_bytes(bytes, "test").unwrap(), expected);
        }
    }

    #[test]
    fn absolute_path_join_is_exact_for_root_and_unicode_components() {
        assert_eq!(join_absolute_path(&[], "test").unwrap(), "/");
        assert_eq!(
            join_absolute_path(&["mnt".into(), "snowman-☃".into()], "test").unwrap(),
            "/mnt/snowman-☃"
        );
    }

    struct Identity;
    impl AddressTranslator for Identity {
        fn cache_tag(&self) -> u64 {
            0x4c49_4e55_585f_4944
        }

        fn translate(
            &self,
            _memory: &dyn MemoryAccess,
            _root: TranslationRoot,
            address: Gva,
        ) -> Result<Translation> {
            Ok(Translation::new(Gpa::new(address.raw() & !0xfff), 4096))
        }
    }

    fn task(next: u64, pid: u32, comm: &str) -> Vec<u8> {
        let mut data = vec![0u8; 0x40];
        data[0x10..0x18].copy_from_slice(&next.to_le_bytes());
        data[0x20..0x24].copy_from_slice(&pid.to_le_bytes());
        data[0x30..0x30 + comm.len()].copy_from_slice(comm.as_bytes());
        data
    }

    fn module(next: u64, name: &str, core_base: u64, core_size: u64) -> Vec<u8> {
        let mut data = vec![0u8; 0x60];
        data[0x10..0x18].copy_from_slice(&next.to_le_bytes());
        data[0x20..0x20 + name.len()].copy_from_slice(name.as_bytes());
        data[0x40..0x48].copy_from_slice(&core_base.to_le_bytes());
        data[0x48..0x50].copy_from_slice(&core_size.to_le_bytes());
        data
    }

    fn file(dentry: u64) -> Vec<u8> {
        let mut data = vec![0u8; 0x20];
        data[0x18..0x20].copy_from_slice(&dentry.to_le_bytes());
        data
    }

    fn dentry(name_address: u64, name_length: u32) -> Vec<u8> {
        let mut data = vec![0u8; 0x30];
        data[0x20..0x24].copy_from_slice(&name_length.to_le_bytes());
        data[0x28..0x30].copy_from_slice(&name_address.to_le_bytes());
        data
    }

    fn path_dentry(parent: u64, name_address: u64, name_length: u32) -> Vec<u8> {
        let mut data = dentry(name_address, name_length);
        data[..8].copy_from_slice(&parent.to_le_bytes());
        data
    }

    fn dentry_offsets() -> LinuxDentryOffsets {
        LinuxDentryOffsets {
            parent: 0,
            name: 0x20,
            qstr_length: 0,
            qstr_name: 0x8,
            maximum_name_bytes: 255,
        }
    }

    fn socket_offsets() -> LinuxSocketOffsets {
        LinuxSocketOffsets {
            file_private_data: 0,
            socket_sk: 0,
            sock_family: 0,
            sock_protocol: 2,
            sock_state: 3,
            ipv4_source: 4,
            ipv4_destination: 8,
            ipv6_source: 0x10,
            ipv6_destination: 0x20,
            source_port: 0x30,
            destination_port: 0x32,
        }
    }

    fn mount(parent: u64, mountpoint: u64, root: u64) -> Vec<u8> {
        let mut data = vec![0u8; 0x18];
        data[..8].copy_from_slice(&parent.to_le_bytes());
        data[8..16].copy_from_slice(&mountpoint.to_le_bytes());
        data[16..24].copy_from_slice(&root.to_le_bytes());
        data
    }

    fn mount_offsets() -> LinuxMountOffsets {
        LinuxMountOffsets {
            parent: 0,
            mountpoint: 8,
            root: 16,
        }
    }

    fn file_offsets() -> LinuxFileOffsets {
        LinuxFileOffsets {
            task_files: 0x8,
            files_fdt: 0,
            fdtable_max_fds: 0,
            fdtable_fd: 0x8,
            file_path: 0x10,
            path_dentry: 0x8,
            dentry_name: 0x20,
            qstr_length: 0,
            qstr_name: 0x8,
            maximum_name_bytes: 255,
        }
    }

    #[test]
    fn walks_linux_task_list() {
        let connector = FakeConnector::default()
            .with_segment(0x1000_u64, task(0x2010, 0, "swapper"))
            .with_segment(0x2000_u64, task(0x1010, 42, "worker"));
        let session = VmiSession::attach(
            &connector,
            AttachRequest::any(CapabilitySet::from_caps([Capability::MemoryRead])),
        )
        .unwrap();
        let profile = SymbolTable::from_system_map("0000000000001000 D init_task\n").unwrap();
        let offsets = LinuxTaskOffsets {
            tasks: 0x10,
            pid: 0x20,
            comm: 0x30,
            comm_length: 16,
        };
        let processes = LinuxIntrospector::new(
            &session,
            &Identity,
            TranslationRoot::new(0),
            &profile,
            offsets,
        )
        .processes(16)
        .unwrap();
        assert_eq!(
            processes,
            vec![
                LinuxProcess {
                    task: Gva::new(0x1000),
                    pid: 0,
                    command: "swapper".into()
                },
                LinuxProcess {
                    task: Gva::new(0x2000),
                    pid: 42,
                    command: "worker".into()
                }
            ]
        );
    }

    #[test]
    fn rejects_corrupt_task_list_and_limits() {
        let connector = FakeConnector::default().with_segment(0x1000_u64, task(0x1008, 0, "bad"));
        let session = VmiSession::attach(&connector, AttachRequest::default()).unwrap();
        let profile = SymbolTable::from_system_map("1000 D init_task\n").unwrap();
        let offsets = LinuxTaskOffsets {
            tasks: 0x10,
            pid: 0x20,
            comm: 0x30,
            comm_length: 16,
        };
        assert!(LinuxIntrospector::new(
            &session,
            &Identity,
            TranslationRoot::new(0),
            &profile,
            offsets
        )
        .processes(4)
        .is_err());
        assert!(LinuxIntrospector::new(
            &session,
            &Identity,
            TranslationRoot::new(0),
            &profile,
            offsets
        )
        .processes(0)
        .is_err());
    }

    #[test]
    fn walks_linux_module_list() {
        let connector = FakeConnector::default()
            .with_segment(0x800_u64, 0x1010u64.to_le_bytes().to_vec())
            .with_segment(0x1000_u64, module(0x2010, "netfilter", 0xa000, 0x800))
            .with_segment(0x2000_u64, module(0x800, "virtio", 0xb000, 0x500));
        let session = VmiSession::attach(&connector, AttachRequest::default()).unwrap();
        let profile = SymbolTable::from_system_map("800 D modules\n1000 D init_task\n").unwrap();
        let inspector = LinuxIntrospector::new(
            &session,
            &Identity,
            TranslationRoot::new(0),
            &profile,
            LinuxTaskOffsets {
                tasks: 0x10,
                pid: 0x20,
                comm: 0x30,
                comm_length: 16,
            },
        );
        let modules = inspector
            .modules(
                LinuxModuleOffsets {
                    list: 0x10,
                    name: 0x20,
                    name_length: 16,
                    core_base: 0x40,
                    core_size: 0x48,
                },
                8,
            )
            .unwrap();
        assert_eq!(
            modules,
            vec![
                LinuxModule {
                    module: Gva::new(0x1000),
                    name: "netfilter".into(),
                    core_base: Gva::new(0xa000),
                    core_size: 0x800,
                },
                LinuxModule {
                    module: Gva::new(0x2000),
                    name: "virtio".into(),
                    core_base: Gva::new(0xb000),
                    core_size: 0x500,
                },
            ]
        );
    }

    #[test]
    fn rejects_corrupt_module_lists_and_limits() {
        let connector = FakeConnector::default()
            .with_segment(0x800_u64, 0x1010u64.to_le_bytes().to_vec())
            .with_segment(0x1000_u64, module(0x1010, "bad", 0, 0));
        let session = VmiSession::attach(&connector, AttachRequest::default()).unwrap();
        let profile = SymbolTable::from_system_map("800 D modules\n1000 D init_task\n").unwrap();
        let inspector = LinuxIntrospector::new(
            &session,
            &Identity,
            TranslationRoot::new(0),
            &profile,
            LinuxTaskOffsets {
                tasks: 0x10,
                pid: 0x20,
                comm: 0x30,
                comm_length: 16,
            },
        );
        let offsets = LinuxModuleOffsets {
            list: 0x10,
            name: 0x20,
            name_length: 16,
            core_base: 0x40,
            core_size: 0x48,
        };
        assert!(inspector.modules(offsets, 4).is_err());
        assert!(inspector.modules(offsets, 0).is_err());
    }

    #[test]
    fn walks_linux_open_file_descriptors() {
        let mut task_data = vec![0u8; 0x20];
        task_data[0x8..0x10].copy_from_slice(&0x2000u64.to_le_bytes());
        let mut fdt = vec![0u8; 0x10];
        fdt[..4].copy_from_slice(&4u32.to_le_bytes());
        fdt[8..16].copy_from_slice(&0x4000u64.to_le_bytes());
        let mut descriptors = vec![0u8; 32];
        descriptors[..8].copy_from_slice(&0x5000u64.to_le_bytes());
        descriptors[16..24].copy_from_slice(&0x6000u64.to_le_bytes());
        let connector = FakeConnector::default()
            .with_segment(0x1000_u64, task_data)
            .with_segment(0x2000_u64, 0x3000u64.to_le_bytes().to_vec())
            .with_segment(0x3000_u64, fdt)
            .with_segment(0x4000_u64, descriptors)
            .with_segment(0x5000_u64, file(0x7000))
            .with_segment(0x6000_u64, file(0x8000))
            .with_segment(0x7000_u64, dentry(0x9000, 10))
            .with_segment(0x8000_u64, dentry(0xa000, 6))
            .with_segment(0x9000_u64, b"config.tom".to_vec())
            .with_segment(0xa000_u64, b"socket".to_vec());
        let session = VmiSession::attach(&connector, AttachRequest::default()).unwrap();
        let profile = SymbolTable::from_system_map("1000 D init_task\n").unwrap();
        let files = LinuxIntrospector::new(
            &session,
            &Identity,
            TranslationRoot::new(0),
            &profile,
            LinuxTaskOffsets {
                tasks: 0x10,
                pid: 0,
                comm: 0,
                comm_length: 16,
            },
        )
        .open_files(Gva::new(0x1000), file_offsets(), 16)
        .unwrap();
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].descriptor, 0);
        assert_eq!(files[0].name, "config.tom");
        assert_eq!(files[1].descriptor, 2);
        assert_eq!(files[1].name, "socket");
    }

    #[test]
    fn rejects_excessive_or_corrupt_fdtables() {
        let mut task_data = vec![0u8; 0x10];
        task_data[8..16].copy_from_slice(&0x2000u64.to_le_bytes());
        let mut fdt = vec![0u8; 0x10];
        fdt[..4].copy_from_slice(&100u32.to_le_bytes());
        fdt[8..16].copy_from_slice(&0u64.to_le_bytes());
        let connector = FakeConnector::default()
            .with_segment(0x1000_u64, task_data)
            .with_segment(0x2000_u64, 0x3000u64.to_le_bytes().to_vec())
            .with_segment(0x3000_u64, fdt);
        let session = VmiSession::attach(&connector, AttachRequest::default()).unwrap();
        let profile = SymbolTable::from_system_map("1000 D init_task\n").unwrap();
        let inspector = LinuxIntrospector::new(
            &session,
            &Identity,
            TranslationRoot::new(0),
            &profile,
            LinuxTaskOffsets {
                tasks: 0,
                pid: 0,
                comm: 0,
                comm_length: 16,
            },
        );
        assert!(inspector
            .open_files(Gva::new(0x1000), file_offsets(), 16)
            .is_err());
        assert!(inspector
            .open_files(Gva::new(0x1000), file_offsets(), 0)
            .is_err());
    }

    #[test]
    fn reconstructs_linux_dentry_paths() {
        let connector = FakeConnector::default()
            .with_segment(0x7000_u64, path_dentry(0x7000, 0x9000, 1))
            .with_segment(0x8000_u64, path_dentry(0x7000, 0xa000, 3))
            .with_segment(0xb000_u64, path_dentry(0x8000, 0xc000, 5))
            .with_segment(0x9000_u64, b"/".to_vec())
            .with_segment(0xa000_u64, b"etc".to_vec())
            .with_segment(0xc000_u64, b"hosts".to_vec());
        let session = VmiSession::attach(&connector, AttachRequest::default()).unwrap();
        let profile = SymbolTable::from_system_map("1000 D init_task\n").unwrap();
        let inspector = LinuxIntrospector::new(
            &session,
            &Identity,
            TranslationRoot::new(0),
            &profile,
            LinuxTaskOffsets {
                tasks: 0,
                pid: 0,
                comm: 0,
                comm_length: 16,
            },
        );
        assert_eq!(
            inspector
                .dentry_path(Gva::new(0xb000), dentry_offsets(), 8)
                .unwrap(),
            "/etc/hosts"
        );
        assert_eq!(
            inspector
                .dentry_path(Gva::new(0x7000), dentry_offsets(), 8)
                .unwrap(),
            "/"
        );
    }

    #[test]
    fn rejects_cyclic_and_overlong_dentry_paths() {
        let connector = FakeConnector::default()
            .with_segment(0x7000_u64, path_dentry(0x8000, 0x9000, 1))
            .with_segment(0x8000_u64, path_dentry(0x7000, 0xa000, 1))
            .with_segment(0x9000_u64, b"a".to_vec())
            .with_segment(0xa000_u64, b"b".to_vec());
        let session = VmiSession::attach(&connector, AttachRequest::default()).unwrap();
        let profile = SymbolTable::from_system_map("1000 D init_task\n").unwrap();
        let inspector = LinuxIntrospector::new(
            &session,
            &Identity,
            TranslationRoot::new(0),
            &profile,
            LinuxTaskOffsets {
                tasks: 0,
                pid: 0,
                comm: 0,
                comm_length: 16,
            },
        );
        assert!(inspector
            .dentry_path(Gva::new(0x7000), dentry_offsets(), 8)
            .is_err());
        assert!(inspector
            .dentry_path(Gva::new(0x7000), dentry_offsets(), 1)
            .is_err());
    }

    #[test]
    fn extracts_ipv4_and_ipv6_sockets_from_files() {
        let mut ipv4 = vec![0u8; 0x40];
        ipv4[..2].copy_from_slice(&2u16.to_le_bytes());
        ipv4[2] = 6;
        ipv4[3] = 1;
        ipv4[4..8].copy_from_slice(&[127, 0, 0, 1]);
        ipv4[8..12].copy_from_slice(&[10, 0, 0, 2]);
        ipv4[0x30..0x32].copy_from_slice(&8080u16.to_be_bytes());
        ipv4[0x32..0x34].copy_from_slice(&443u16.to_be_bytes());
        let mut ipv6 = vec![0u8; 0x40];
        ipv6[..2].copy_from_slice(&10u16.to_le_bytes());
        ipv6[2] = 17;
        ipv6[3] = 7;
        ipv6[0x10..0x20].copy_from_slice(&Ipv6Addr::LOCALHOST.octets());
        ipv6[0x20..0x30].copy_from_slice(&"2001:db8::1".parse::<Ipv6Addr>().unwrap().octets());
        ipv6[0x30..0x32].copy_from_slice(&53u16.to_be_bytes());
        ipv6[0x32..0x34].copy_from_slice(&5353u16.to_be_bytes());
        let connector = FakeConnector::default()
            .with_segment(0x1000_u64, 0x2000u64.to_le_bytes().to_vec())
            .with_segment(0x2000_u64, 0x3000u64.to_le_bytes().to_vec())
            .with_segment(0x3000_u64, ipv4)
            .with_segment(0x4000_u64, 0x5000u64.to_le_bytes().to_vec())
            .with_segment(0x5000_u64, 0x6000u64.to_le_bytes().to_vec())
            .with_segment(0x6000_u64, ipv6);
        let session = VmiSession::attach(&connector, AttachRequest::default()).unwrap();
        let profile = SymbolTable::from_system_map("1000 D init_task\n").unwrap();
        let inspector = LinuxIntrospector::new(
            &session,
            &Identity,
            TranslationRoot::new(0),
            &profile,
            LinuxTaskOffsets {
                tasks: 0,
                pid: 0,
                comm: 0,
                comm_length: 16,
            },
        );
        let tcp = inspector
            .socket_from_file(Gva::new(0x1000), socket_offsets())
            .unwrap();
        assert_eq!(tcp.source, "127.0.0.1".parse::<IpAddr>().unwrap());
        assert_eq!((tcp.source_port, tcp.destination_port), (8080, 443));
        assert_eq!((tcp.protocol, tcp.state), (6, 1));
        let udp = inspector
            .socket_from_file(Gva::new(0x4000), socket_offsets())
            .unwrap();
        assert_eq!(udp.destination, "2001:db8::1".parse::<IpAddr>().unwrap());
        assert_eq!((udp.source_port, udp.destination_port), (53, 5353));
    }

    #[test]
    fn rejects_null_and_unsupported_linux_sockets() {
        let mut unsupported = vec![0u8; 0x40];
        unsupported[..2].copy_from_slice(&1u16.to_le_bytes());
        let connector = FakeConnector::default()
            .with_segment(0x1000_u64, 0u64.to_le_bytes().to_vec())
            .with_segment(0x2000_u64, 0x3000u64.to_le_bytes().to_vec())
            .with_segment(0x3000_u64, 0x4000u64.to_le_bytes().to_vec())
            .with_segment(0x4000_u64, unsupported);
        let session = VmiSession::attach(&connector, AttachRequest::default()).unwrap();
        let profile = SymbolTable::from_system_map("1000 D init_task\n").unwrap();
        let inspector = LinuxIntrospector::new(
            &session,
            &Identity,
            TranslationRoot::new(0),
            &profile,
            LinuxTaskOffsets {
                tasks: 0,
                pid: 0,
                comm: 0,
                comm_length: 16,
            },
        );
        assert!(inspector
            .socket_from_file(Gva::new(0x1000), socket_offsets())
            .is_err());
        assert!(inspector
            .socket_from_file(Gva::new(0x2000), socket_offsets())
            .is_err());
    }

    #[test]
    fn reconstructs_paths_across_mount_boundaries() {
        let connector = FakeConnector::default()
            .with_segment(0x7000_u64, path_dentry(0x7000, 0x9000, 1))
            .with_segment(0x8000_u64, path_dentry(0x7000, 0x9100, 3))
            .with_segment(0xa000_u64, path_dentry(0xa000, 0x9200, 1))
            .with_segment(0xb000_u64, path_dentry(0xa000, 0x9300, 4))
            .with_segment(0x9000_u64, b"/".to_vec())
            .with_segment(0x9100_u64, b"mnt".to_vec())
            .with_segment(0x9200_u64, b"/".to_vec())
            .with_segment(0x9300_u64, b"data".to_vec())
            .with_segment(0xd000_u64, mount(0xd000, 0x7000, 0x7000))
            .with_segment(0xe000_u64, mount(0xd000, 0x8000, 0xa000));
        let session = VmiSession::attach(&connector, AttachRequest::default()).unwrap();
        let profile = SymbolTable::from_system_map("1000 D init_task\n").unwrap();
        let inspector = LinuxIntrospector::new(
            &session,
            &Identity,
            TranslationRoot::new(0),
            &profile,
            LinuxTaskOffsets {
                tasks: 0,
                pid: 0,
                comm: 0,
                comm_length: 16,
            },
        );
        assert_eq!(
            inspector
                .mounted_path(
                    Gva::new(0xb000),
                    Gva::new(0xe000),
                    dentry_offsets(),
                    mount_offsets(),
                    8,
                )
                .unwrap(),
            "/mnt/data"
        );
    }

    #[test]
    fn rejects_corrupt_mount_ancestry() {
        let connector = FakeConnector::default()
            .with_segment(0x7000_u64, path_dentry(0x7000, 0x9000, 1))
            .with_segment(0x9000_u64, b"/".to_vec())
            .with_segment(0xd000_u64, mount(0xe000, 0x7000, 0x7000))
            .with_segment(0xe000_u64, mount(0xd000, 0x7000, 0x7000));
        let session = VmiSession::attach(&connector, AttachRequest::default()).unwrap();
        let profile = SymbolTable::from_system_map("1000 D init_task\n").unwrap();
        let inspector = LinuxIntrospector::new(
            &session,
            &Identity,
            TranslationRoot::new(0),
            &profile,
            LinuxTaskOffsets {
                tasks: 0,
                pid: 0,
                comm: 0,
                comm_length: 16,
            },
        );
        assert!(inspector
            .mounted_path(
                Gva::new(0x7000),
                Gva::new(0xd000),
                dentry_offsets(),
                mount_offsets(),
                8,
            )
            .is_err());
    }

    #[test]
    fn enumerates_profile_configured_socket_lists() {
        let mut first = vec![0u8; 0x40];
        first[..2].copy_from_slice(&2u16.to_le_bytes());
        first[2] = 6;
        first[4..8].copy_from_slice(&[10, 0, 0, 1]);
        first[8..12].copy_from_slice(&[10, 0, 0, 2]);
        first[0x30..0x32].copy_from_slice(&80u16.to_be_bytes());
        first[0x32..0x34].copy_from_slice(&8080u16.to_be_bytes());
        let mut second = first.clone();
        second[2] = 17;
        second[0x30..0x32].copy_from_slice(&53u16.to_be_bytes());
        let mut node_one = vec![0u8; 16];
        node_one[..8].copy_from_slice(&0x9000u64.to_le_bytes());
        node_one[8..16].copy_from_slice(&0xa000u64.to_le_bytes());
        let mut node_two = vec![0u8; 16];
        node_two[..8].copy_from_slice(&0x7000u64.to_le_bytes());
        node_two[8..16].copy_from_slice(&0xb000u64.to_le_bytes());
        let connector = FakeConnector::default()
            .with_segment(0x7000_u64, 0x8000u64.to_le_bytes().to_vec())
            .with_segment(0x8000_u64, node_one)
            .with_segment(0x9000_u64, node_two)
            .with_segment(0xa000_u64, first)
            .with_segment(0xb000_u64, second);
        let session = VmiSession::attach(&connector, AttachRequest::default()).unwrap();
        let profile = SymbolTable::from_system_map("1000 D init_task\n").unwrap();
        let sockets = LinuxIntrospector::new(
            &session,
            &Identity,
            TranslationRoot::new(0),
            &profile,
            LinuxTaskOffsets {
                tasks: 0,
                pid: 0,
                comm: 0,
                comm_length: 16,
            },
        )
        .sockets_from_list(
            Gva::new(0x7000),
            LinuxSocketListOffsets {
                node_next: 0,
                node_sock: 8,
            },
            socket_offsets(),
            8,
        )
        .unwrap();
        assert_eq!(sockets.len(), 2);
        assert_eq!((sockets[0].protocol, sockets[0].source_port), (6, 80));
        assert_eq!((sockets[1].protocol, sockets[1].source_port), (17, 53));
        assert_eq!(sockets[0].file, Gva::new(0));
        assert_eq!(sockets[0].sock, Gva::new(0xa000));
    }

    #[test]
    fn rejects_corrupt_profile_configured_socket_lists() {
        let mut node = vec![0u8; 16];
        node[..8].copy_from_slice(&0x8000u64.to_le_bytes());
        node[8..16].copy_from_slice(&0xa000u64.to_le_bytes());
        let connector = FakeConnector::default()
            .with_segment(0x7000_u64, 0x8000u64.to_le_bytes().to_vec())
            .with_segment(0x8000_u64, node)
            .with_segment(0xa000_u64, vec![0u8; 0x40]);
        let session = VmiSession::attach(&connector, AttachRequest::default()).unwrap();
        let profile = SymbolTable::from_system_map("1000 D init_task\n").unwrap();
        let inspector = LinuxIntrospector::new(
            &session,
            &Identity,
            TranslationRoot::new(0),
            &profile,
            LinuxTaskOffsets {
                tasks: 0,
                pid: 0,
                comm: 0,
                comm_length: 16,
            },
        );
        assert!(inspector
            .sockets_from_list(
                Gva::new(0x7000),
                LinuxSocketListOffsets {
                    node_next: 0,
                    node_sock: 8,
                },
                socket_offsets(),
                8,
            )
            .is_err());
    }

    #[test]
    fn enumerates_profile_configured_socket_hash_buckets() {
        let mut table = vec![0u8; 16];
        table[..8].copy_from_slice(&0x8000u64.to_le_bytes());
        table[8..16].copy_from_slice(&0x9000u64.to_le_bytes());
        let mut first_node = vec![0u8; 16];
        first_node[8..16].copy_from_slice(&0xa000u64.to_le_bytes());
        let mut second_node = vec![0u8; 16];
        second_node[8..16].copy_from_slice(&0xb000u64.to_le_bytes());
        let mut first_sock = vec![0u8; 0x40];
        first_sock[..2].copy_from_slice(&2u16.to_le_bytes());
        first_sock[2] = 6;
        first_sock[4..8].copy_from_slice(&[192, 0, 2, 1]);
        first_sock[8..12].copy_from_slice(&[192, 0, 2, 2]);
        first_sock[0x30..0x32].copy_from_slice(&22u16.to_be_bytes());
        first_sock[0x32..0x34].copy_from_slice(&50000u16.to_be_bytes());
        let mut second_sock = first_sock.clone();
        second_sock[2] = 17;
        second_sock[0x30..0x32].copy_from_slice(&123u16.to_be_bytes());
        let connector = FakeConnector::default()
            .with_segment(0x7000_u64, table)
            .with_segment(0x8000_u64, first_node)
            .with_segment(0x9000_u64, second_node)
            .with_segment(0xa000_u64, first_sock)
            .with_segment(0xb000_u64, second_sock);
        let session = VmiSession::attach(&connector, AttachRequest::default()).unwrap();
        let profile = SymbolTable::from_system_map("1000 D init_task\n").unwrap();
        let sockets = LinuxIntrospector::new(
            &session,
            &Identity,
            TranslationRoot::new(0),
            &profile,
            LinuxTaskOffsets {
                tasks: 0,
                pid: 0,
                comm: 0,
                comm_length: 16,
            },
        )
        .sockets_from_hash_table(
            Gva::new(0x7000),
            2,
            LinuxSocketHashOffsets {
                bucket_first: 0,
                bucket_stride: 8,
                node_next: 0,
                node_sock: 8,
            },
            socket_offsets(),
            8,
        )
        .unwrap();
        assert_eq!(sockets.len(), 2);
        assert_eq!((sockets[0].protocol, sockets[0].source_port), (6, 22));
        assert_eq!((sockets[1].protocol, sockets[1].source_port), (17, 123));
    }

    #[test]
    fn rejects_duplicate_nodes_across_socket_hash_buckets() {
        let mut table = vec![0u8; 16];
        table[..8].copy_from_slice(&0x8000u64.to_le_bytes());
        table[8..16].copy_from_slice(&0x8000u64.to_le_bytes());
        let mut node = vec![0u8; 16];
        node[8..16].copy_from_slice(&0xa000u64.to_le_bytes());
        let mut sock = vec![0u8; 0x40];
        sock[..2].copy_from_slice(&2u16.to_le_bytes());
        let connector = FakeConnector::default()
            .with_segment(0x7000_u64, table)
            .with_segment(0x8000_u64, node)
            .with_segment(0xa000_u64, sock);
        let session = VmiSession::attach(&connector, AttachRequest::default()).unwrap();
        let profile = SymbolTable::from_system_map("1000 D init_task\n").unwrap();
        let inspector = LinuxIntrospector::new(
            &session,
            &Identity,
            TranslationRoot::new(0),
            &profile,
            LinuxTaskOffsets {
                tasks: 0,
                pid: 0,
                comm: 0,
                comm_length: 16,
            },
        );
        assert!(inspector
            .sockets_from_hash_table(
                Gva::new(0x7000),
                2,
                LinuxSocketHashOffsets {
                    bucket_first: 0,
                    bucket_stride: 8,
                    node_next: 0,
                    node_sock: 8,
                },
                socket_offsets(),
                8,
            )
            .is_err());
    }
}
