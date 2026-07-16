use std::{env, ffi::OsString, path::PathBuf, process::ExitCode, sync::Arc, time::Duration};

use vmi_arch_amd64::Amd64Translator;
use vmi_artifact::SnapshotBundle;
use vmi_core::VmiSession;
use vmi_driver_dump::DumpConnector;
use vmi_driver_qemu::QemuConnector;
use vmi_driver_snapshot::SnapshotConnector;
use vmi_driver_virtualbox::{ProcessTransport, VirtualBoxConnector};
use vmi_os_linux::{LinuxIntrospector, LinuxTaskOffsets};
use vmi_os_windows::{WindowsIntrospector, WindowsProcessOffsets};
use vmi_profile::{Profile, SymbolTable};
use vmi_types::{
    AttachRequest, Capability, CapabilitySet, Gpa, GuestArchitecture, TranslationRoot,
};

const MAX_ARGUMENT_COUNT: usize = 64;
const MAX_ARGUMENT_BYTES: usize = 32 * 1024;

fn parse_number(value: &str) -> Result<u64, String> {
    value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
        .map(|hex| u64::from_str_radix(hex, 16))
        .unwrap_or_else(|| value.parse())
        .map_err(|error| format!("invalid number {value}: {error}"))
}

fn run() -> Result<(), String> {
    let args = collect_args(env::args_os())?;
    run_args(&args)
}

fn collect_args(arguments: impl IntoIterator<Item = OsString>) -> Result<Vec<String>, String> {
    let mut output = Vec::new();
    for (index, argument) in arguments.into_iter().enumerate() {
        if index >= MAX_ARGUMENT_COUNT {
            return Err(format!(
                "CLI argument count exceeds the limit of {MAX_ARGUMENT_COUNT}"
            ));
        }
        let argument = argument
            .into_string()
            .map_err(|_| format!("CLI argument {index} is not valid Unicode"))?;
        if argument.len() > MAX_ARGUMENT_BYTES {
            return Err(format!(
                "CLI argument {index} exceeds the limit of {MAX_ARGUMENT_BYTES} bytes"
            ));
        }
        output
            .try_reserve(1)
            .map_err(|error| format!("failed to grow CLI argument list: {error}"))?;
        output.push(argument);
    }
    Ok(output)
}

fn run_args(args: &[String]) -> Result<(), String> {
    match args.get(1).map(String::as_str) {
        Some("read-raw") if args.len() == 5 => read_raw(args),
        Some("read-elf") if args.len() == 5 => read_elf(args),
        Some("read-xen-core") if args.len() == 5 => read_xen_core(args),
        Some("read-kdmp") if args.len() == 5 => read_kdmp(args),
        Some("read-lime") if args.len() == 5 => read_lime(args),
        Some("read-manifest") if args.len() == 5 => read_manifest(args),
        Some("read-vmware-vmem") if args.len() == 6 => read_vmware_vmem(args),
        Some("read-vmware-core") if args.len() == 5 => read_vmware_core(args),
        Some("qemu-status") if args.len() == 3 => qemu_control(&args[2], "status"),
        Some("qemu-pause") if args.len() == 3 => qemu_control(&args[2], "pause"),
        Some("qemu-resume") if args.len() == 3 => qemu_control(&args[2], "resume"),
        Some("qemu-read") if args.len() == 5 => qemu_read(args),
        Some("qemu-reg-read") if args.len() == 5 => qemu_register(args),
        Some("qemu-event") if args.len() == 4 || args.len() == 5 => qemu_event(args),
        Some("qemu-gdb-reg-write") if args.len() == 6 || args.len() == 7 => {
            qemu_gdb_register_write(args)
        }
        Some("qemu-acquire") if args.len() == 6 => qemu_acquire(args),
        Some("qemu-dump") if args.len() == 4 => qemu_dump(args),
        Some("vbox-status") if args.len() == 3 => vbox_status(&args[2]),
        Some("vbox-reg-read") if args.len() == 5 => vbox_register(args, false),
        Some("vbox-reg-write") if args.len() == 6 => vbox_register(args, true),
        Some("profile-symbol") if args.len() == 4 => profile_symbol(args),
        Some("profile-nearest") if args.len() == 4 => profile_nearest(args),
        Some("profile-json-symbol") if args.len() == 4 => profile_json_symbol(args),
        Some("profile-json-offset") if args.len() == 4 => profile_json_offset(args),
        Some("profile-pdb-symbol") if args.len() == 5 => profile_pdb_symbol(args),
        Some("profile-pdb-offset") if args.len() == 5 => profile_pdb_offset(args),
        Some("linux-processes-elf") if args.len() == 10 => linux_processes_elf(args),
        Some("windows-processes-elf") if args.len() == 11 => windows_processes_elf(args),
        _ => {
            let program = args.first().map(String::as_str).unwrap_or("vmi-cli");
            Err(format!(
                "usage:\n  {program} read-raw|read-elf|read-xen-core|read-kdmp|read-lime|read-manifest <file> <gpa> <length>\n  {program} read-vmware-vmem <file.vmem> <physical-base> <gpa> <length>\n  {program} read-vmware-core <vmss2core-output> <gpa> <length>\n  {program} qemu-status|qemu-pause|qemu-resume <host:port>\n  {program} qemu-read <host:port> <gpa> <length>\n  {program} qemu-reg-read <host:port> <vcpu> <register>\n  {program} qemu-event <host:port> <timeout-ms> [event-kind]\n  {program} qemu-gdb-reg-write <qmp-host:port> <gdb-host:port> [vcpu] <register> <value>\n  {program} qemu-acquire <host:port> <output> <gpa> <length>\n  {program} qemu-dump <host:port> <output.elf>\n  {program} vbox-status <vm>\n  {program} vbox-reg-read <vm> <vcpu> <register>\n  {program} vbox-reg-write <vm> <vcpu> <register> <value>\n  {program} profile-symbol|profile-nearest <System.map> <name-or-address>\n  {program} profile-json-symbol|profile-json-offset <profile.json> <name>\n  {program} profile-pdb-symbol|profile-pdb-offset <file.pdb> <image-base> <name>\n  {program} linux-processes-elf <vmcore> <System.map> <cr3> <tasks_off> <pid_off> <comm_off> <comm_len> <limit>\n  {program} windows-processes-elf <vmcore> <symbols> <cr3> <links_off> <pid_off> <image_off> <image_len> <dtb_off> <limit>"
            ))
        }
    }
}

fn vbox_session(vm: &str, capability: Capability) -> Result<VmiSession, String> {
    let executable = env::var("VBOXMANAGE").unwrap_or_else(|_| "VBoxManage".into());
    let connector = VirtualBoxConnector::with_transport(
        vm,
        GuestArchitecture::Amd64,
        Arc::new(ProcessTransport::new(executable)),
    );
    let connector = if capability == Capability::RegisterWrite {
        connector.with_register_write()
    } else {
        connector
    };
    VmiSession::attach(
        &connector,
        AttachRequest::any(CapabilitySet::from_caps([capability])),
    )
    .map_err(|error| error.to_string())
}

fn vbox_status(vm: &str) -> Result<(), String> {
    let session = vbox_session(vm, Capability::Control)?;
    println!(
        "{:?}",
        session
            .session()
            .control()
            .map_err(|error| error.to_string())?
            .execution_state()
            .map_err(|error| error.to_string())?
    );
    Ok(())
}

fn vbox_register(args: &[String], write: bool) -> Result<(), String> {
    let vcpu = u32::try_from(parse_number(&args[3])?)
        .map_err(|_| "vCPU index is too large".to_string())?;
    let capability = if write {
        Capability::RegisterWrite
    } else {
        Capability::RegisterRead
    };
    let session = vbox_session(&args[2], capability)?;
    let cpu = session.session().cpu().map_err(|error| error.to_string())?;
    if write {
        cpu.write_register(vcpu, &args[4], parse_number(&args[5])?)
            .map_err(|error| error.to_string())?;
    }
    let value = cpu
        .read_register(vcpu, &args[4])
        .map_err(|error| error.to_string())?;
    println!("vCPU {vcpu} {}={value:#018x}", args[4].to_ascii_uppercase());
    Ok(())
}

fn read_raw(args: &[String]) -> Result<(), String> {
    let address = parse_number(&args[3])?;
    let length = usize::try_from(parse_number(&args[4])?)
        .map_err(|_| "length does not fit this platform".to_string())?;
    let bundle = SnapshotBundle::raw_file(&args[2], Gpa::new(0)).map_err(|e| e.to_string())?;
    let session = VmiSession::attach(
        &DumpConnector::new(bundle, GuestArchitecture::Amd64),
        AttachRequest::any(CapabilitySet::from_caps([Capability::MemoryRead])),
    )
    .map_err(|e| e.to_string())?;
    let bytes = session
        .read_bytes(Gpa::new(address), length)
        .map_err(|e| e.to_string())?;
    print_bytes(address, &bytes)?;
    Ok(())
}

fn read_vmware_vmem(args: &[String]) -> Result<(), String> {
    let physical_base = parse_number(&args[3])?;
    let address = parse_number(&args[4])?;
    let length = usize::try_from(parse_number(&args[5])?)
        .map_err(|_| "length does not fit this platform".to_string())?;
    let connector = SnapshotConnector::open_vmware_vmem(
        GuestArchitecture::Amd64,
        &args[2],
        Gpa::new(physical_base),
    )
    .map_err(|error| error.to_string())?;
    read_snapshot_connector(&connector, address, length)
}

fn read_vmware_core(args: &[String]) -> Result<(), String> {
    let address = parse_number(&args[3])?;
    let length = usize::try_from(parse_number(&args[4])?)
        .map_err(|_| "length does not fit this platform".to_string())?;
    let connector =
        SnapshotConnector::open_vmware_converted_core(GuestArchitecture::Amd64, &args[2])
            .map_err(|error| error.to_string())?;
    read_snapshot_connector(&connector, address, length)
}

fn read_snapshot_connector(
    connector: &SnapshotConnector,
    address: u64,
    length: usize,
) -> Result<(), String> {
    let session = VmiSession::attach(
        connector,
        AttachRequest::any(CapabilitySet::from_caps([Capability::MemoryRead])),
    )
    .map_err(|error| error.to_string())?;
    let bytes = session
        .read_bytes(Gpa::new(address), length)
        .map_err(|error| error.to_string())?;
    print_bytes(address, &bytes)
}

fn read_elf(args: &[String]) -> Result<(), String> {
    let address = parse_number(&args[3])?;
    let length = usize::try_from(parse_number(&args[4])?)
        .map_err(|_| "length does not fit this platform".to_string())?;
    let bundle = SnapshotBundle::elf_vmcore_file(&args[2]).map_err(|error| error.to_string())?;
    let session = VmiSession::attach(
        &DumpConnector::new(bundle, GuestArchitecture::Amd64),
        AttachRequest::any(CapabilitySet::from_caps([Capability::MemoryRead])),
    )
    .map_err(|error| error.to_string())?;
    let bytes = session
        .read_bytes(Gpa::new(address), length)
        .map_err(|error| error.to_string())?;
    print_bytes(address, &bytes)?;
    Ok(())
}

fn read_xen_core(args: &[String]) -> Result<(), String> {
    let address = parse_number(&args[3])?;
    let length = usize::try_from(parse_number(&args[4])?)
        .map_err(|_| "length does not fit this platform".to_string())?;
    let bundle = SnapshotBundle::xen_core_file(&args[2]).map_err(|error| error.to_string())?;
    let session = VmiSession::attach(
        &DumpConnector::new(bundle, GuestArchitecture::Amd64),
        AttachRequest::any(CapabilitySet::from_caps([Capability::MemoryRead])),
    )
    .map_err(|error| error.to_string())?;
    let bytes = session
        .read_bytes(Gpa::new(address), length)
        .map_err(|error| error.to_string())?;
    print_bytes(address, &bytes)?;
    Ok(())
}

fn read_kdmp(args: &[String]) -> Result<(), String> {
    let address = parse_number(&args[3])?;
    let length = usize::try_from(parse_number(&args[4])?)
        .map_err(|_| "length does not fit this platform".to_string())?;
    let bundle = SnapshotBundle::kdmp_file(&args[2]).map_err(|error| error.to_string())?;
    let session = VmiSession::attach(
        &DumpConnector::new(bundle, GuestArchitecture::Amd64),
        AttachRequest::any(CapabilitySet::from_caps([Capability::MemoryRead])),
    )
    .map_err(|error| error.to_string())?;
    let bytes = session
        .read_bytes(Gpa::new(address), length)
        .map_err(|error| error.to_string())?;
    print_bytes(address, &bytes)?;
    Ok(())
}

fn read_lime(args: &[String]) -> Result<(), String> {
    let address = parse_number(&args[3])?;
    let length = usize::try_from(parse_number(&args[4])?)
        .map_err(|_| "length does not fit this platform".to_string())?;
    let bundle = SnapshotBundle::lime_file(&args[2]).map_err(|error| error.to_string())?;
    let session = VmiSession::attach(
        &DumpConnector::new(bundle, GuestArchitecture::Amd64),
        AttachRequest::any(CapabilitySet::from_caps([Capability::MemoryRead])),
    )
    .map_err(|error| error.to_string())?;
    let bytes = session
        .read_bytes(Gpa::new(address), length)
        .map_err(|error| error.to_string())?;
    print_bytes(address, &bytes)?;
    Ok(())
}

fn read_manifest(args: &[String]) -> Result<(), String> {
    let address = parse_number(&args[3])?;
    let length = usize::try_from(parse_number(&args[4])?)
        .map_err(|_| "length does not fit this platform".to_string())?;
    let bundle = SnapshotBundle::manifest_file(&args[2]).map_err(|error| error.to_string())?;
    let session = VmiSession::attach(
        &DumpConnector::new(bundle, GuestArchitecture::Amd64),
        AttachRequest::any(CapabilitySet::from_caps([Capability::MemoryRead])),
    )
    .map_err(|error| error.to_string())?;
    let bytes = session
        .read_bytes(Gpa::new(address), length)
        .map_err(|error| error.to_string())?;
    print_bytes(address, &bytes)?;
    Ok(())
}

fn print_bytes(address: u64, bytes: &[u8]) -> Result<(), String> {
    for (index, chunk) in bytes.chunks(16).enumerate() {
        let offset = u64::try_from(index)
            .ok()
            .and_then(|index| index.checked_mul(16))
            .ok_or_else(|| "hex-dump line offset overflow".to_string())?;
        let line_address = address
            .checked_add(offset)
            .ok_or_else(|| "hex-dump address overflow".to_string())?;
        print!("{line_address:016x}:");
        for byte in chunk {
            print!(" {byte:02x}");
        }
        println!();
    }
    Ok(())
}

fn qemu_read(args: &[String]) -> Result<(), String> {
    let address = parse_number(&args[3])?;
    let length = usize::try_from(parse_number(&args[4])?)
        .map_err(|_| "length does not fit this platform".to_string())?;
    let session = qemu_session(&args[2], Capability::MemoryRead)?;
    let bytes = session
        .read_bytes(Gpa::new(address), length)
        .map_err(|error| error.to_string())?;
    print_bytes(address, &bytes)?;
    Ok(())
}

fn qemu_register(args: &[String]) -> Result<(), String> {
    let vcpu = u32::try_from(parse_number(&args[3])?)
        .map_err(|_| "vCPU index is too large".to_string())?;
    let session = qemu_session(&args[2], Capability::RegisterRead)?;
    let cpu = session.session().cpu().map_err(|error| error.to_string())?;
    let value = cpu
        .read_register(vcpu, &args[4])
        .map_err(|error| error.to_string())?;
    println!("vCPU {vcpu} {}={value:#018x}", args[4].to_ascii_uppercase());
    Ok(())
}

fn qemu_event(args: &[String]) -> Result<(), String> {
    let timeout = Duration::from_millis(parse_number(&args[3])?);
    let session = qemu_session(&args[2], Capability::Events)?;
    if let Some(operation) = args.get(4) {
        let control = session
            .session()
            .control()
            .map_err(|error| error.to_string())?;
        match operation.as_str() {
            "pause" => control.pause(),
            "resume" => control.resume(),
            _ => return Err("event trigger must be pause or resume".into()),
        }
        .map_err(|error| error.to_string())?;
    }
    match session
        .session()
        .events()
        .map_err(|error| error.to_string())?
        .next_event(timeout)
        .map_err(|error| error.to_string())?
    {
        Some(event) => println!(
            "event={} vcpu={} address={}",
            event.kind,
            event
                .vcpu
                .map_or_else(|| "-".into(), |value| value.to_string()),
            event
                .address
                .map_or_else(|| "-".into(), |value| format!("{:#x}", value.raw()))
        ),
        None => println!("timeout"),
    }
    Ok(())
}

fn qemu_gdb_register_write(args: &[String]) -> Result<(), String> {
    let (vcpu, register_index, value_index) = if args.len() == 7 {
        (
            u32::try_from(parse_number(&args[4])?)
                .map_err(|_| "vCPU index is too large".to_string())?,
            5,
            6,
        )
    } else {
        (0, 4, 5)
    };
    let connector = qemu_connector(&args[2])?.with_gdb(&args[3]);
    let session = VmiSession::attach(
        &connector,
        AttachRequest::any(CapabilitySet::from_caps([
            Capability::RegisterRead,
            Capability::RegisterWrite,
        ])),
    )
    .map_err(|error| error.to_string())?;
    let cpu = session.session().cpu().map_err(|error| error.to_string())?;
    let value = parse_number(&args[value_index])?;
    cpu.write_register(vcpu, &args[register_index], value)
        .map_err(|error| error.to_string())?;
    let actual = cpu
        .read_register(vcpu, &args[register_index])
        .map_err(|error| error.to_string())?;
    println!(
        "vCPU {vcpu} {}={actual:#018x}",
        args[register_index].to_ascii_uppercase()
    );
    Ok(())
}

fn qemu_session(address: &str, capability: Capability) -> Result<VmiSession, String> {
    VmiSession::attach(
        &qemu_connector(address)?,
        AttachRequest::any(CapabilitySet::from_caps([capability])),
    )
    .map_err(|error| error.to_string())
}

fn qemu_connector(endpoint: &str) -> Result<QemuConnector, String> {
    if let Some(path) = endpoint.strip_prefix("unix:") {
        if path.is_empty() {
            return Err("QMP Unix socket path must not be empty".into());
        }
        #[cfg(unix)]
        return Ok(QemuConnector::unix(path));
        #[cfg(not(unix))]
        return Err("QMP Unix sockets are not supported on this platform".into());
    }
    Ok(QemuConnector::tcp(endpoint))
}

fn qemu_control(address: &str, operation: &str) -> Result<(), String> {
    let session = qemu_session(address, Capability::Control)?;
    let control = session
        .session()
        .control()
        .map_err(|error| error.to_string())?;
    if operation == "pause" {
        control.pause().map_err(|error| error.to_string())?;
    }
    if operation == "resume" {
        control.resume().map_err(|error| error.to_string())?;
    }
    println!(
        "{:?}",
        control
            .execution_state()
            .map_err(|error| error.to_string())?
    );
    Ok(())
}

fn qemu_acquire(args: &[String]) -> Result<(), String> {
    let start = parse_number(&args[4])?;
    let length = parse_number(&args[5])?;
    let path = PathBuf::from(&args[3]);
    let path = if path.is_absolute() {
        path
    } else {
        env::current_dir().map_err(|e| e.to_string())?.join(path)
    };
    let session = qemu_session(&args[2], Capability::Acquisition)?;
    session
        .session()
        .acquisition()
        .map_err(|error| error.to_string())?
        .save_physical_range(&path, Gpa::new(start), length)
        .map_err(|error| error.to_string())?;
    println!(
        "saved {length} bytes from GPA {start:#x} to {}",
        path.display()
    );
    Ok(())
}

fn qemu_dump(args: &[String]) -> Result<(), String> {
    let path = PathBuf::from(&args[3]);
    let path = if path.is_absolute() {
        path
    } else {
        env::current_dir().map_err(|e| e.to_string())?.join(path)
    };
    let session = qemu_session(&args[2], Capability::Acquisition)?;
    session
        .session()
        .acquisition()
        .map_err(|error| error.to_string())?
        .save_snapshot(&path)
        .map_err(|error| error.to_string())?;
    println!("saved QEMU ELF VM core to {}", path.display());
    Ok(())
}

fn profile_symbol(args: &[String]) -> Result<(), String> {
    let table = SymbolTable::from_system_map_file(&args[2]).map_err(|error| error.to_string())?;
    let symbol = table
        .symbol(&args[3])
        .ok_or_else(|| format!("symbol {} not found", args[3]))?;
    println!(
        "{} {:#018x} {}",
        symbol.kind.unwrap_or('?'),
        symbol.address,
        symbol.name
    );
    Ok(())
}

fn profile_nearest(args: &[String]) -> Result<(), String> {
    let address = parse_number(&args[3])?;
    let table = SymbolTable::from_system_map_file(&args[2]).map_err(|error| error.to_string())?;
    let (symbol, offset) = table
        .nearest_symbol(address)
        .ok_or_else(|| format!("no symbol at or below {address:#x}"))?;
    println!("{address:#018x} = {}+{offset:#x}", symbol.name);
    Ok(())
}

fn profile_json_symbol(args: &[String]) -> Result<(), String> {
    let profile = Profile::from_json_file(&args[2]).map_err(|error| error.to_string())?;
    let symbol = profile
        .symbols()
        .symbol(&args[3])
        .ok_or_else(|| format!("symbol {} not found", args[3]))?;
    println!("{}={:#018x}", symbol.name, symbol.address);
    Ok(())
}

fn profile_json_offset(args: &[String]) -> Result<(), String> {
    let profile = Profile::from_json_file(&args[2]).map_err(|error| error.to_string())?;
    let offset = profile
        .require_offset(&args[3])
        .map_err(|error| error.to_string())?;
    println!("{}={offset:#x}", args[3]);
    Ok(())
}

fn profile_pdb_symbol(args: &[String]) -> Result<(), String> {
    let image_base = parse_number(&args[3])?;
    let table =
        SymbolTable::from_pdb_file(&args[2], image_base).map_err(|error| error.to_string())?;
    let symbol = table
        .symbol(&args[4])
        .ok_or_else(|| format!("symbol {} not found", args[4]))?;
    println!("{}={:#018x}", symbol.name, symbol.address);
    Ok(())
}

fn profile_pdb_offset(args: &[String]) -> Result<(), String> {
    let image_base = parse_number(&args[3])?;
    let profile =
        Profile::from_pdb_file(&args[2], image_base).map_err(|error| error.to_string())?;
    let offset = profile
        .require_offset(&args[4])
        .map_err(|error| error.to_string())?;
    println!("{}={offset:#x}", args[4]);
    Ok(())
}

fn linux_processes_elf(args: &[String]) -> Result<(), String> {
    let bundle = SnapshotBundle::elf_vmcore_file(&args[2]).map_err(|error| error.to_string())?;
    let session = VmiSession::attach(
        &DumpConnector::new(bundle, GuestArchitecture::Amd64),
        AttachRequest::any(CapabilitySet::from_caps([Capability::MemoryRead])),
    )
    .map_err(|error| error.to_string())?;
    let profile = SymbolTable::from_system_map_file(&args[3]).map_err(|error| error.to_string())?;
    let root = TranslationRoot::new(parse_number(&args[4])?);
    let offsets = LinuxTaskOffsets {
        tasks: parse_number(&args[5])?,
        pid: parse_number(&args[6])?,
        comm: parse_number(&args[7])?,
        comm_length: usize::try_from(parse_number(&args[8])?)
            .map_err(|_| "comm length is too large".to_string())?,
    };
    let limit = usize::try_from(parse_number(&args[9])?)
        .map_err(|_| "process limit is too large".to_string())?;
    for process in LinuxIntrospector::new(&session, &Amd64Translator, root, &profile, offsets)
        .processes(limit)
        .map_err(|error| error.to_string())?
    {
        println!(
            "{:>6} {} task={}",
            process.pid, process.command, process.task
        );
    }
    Ok(())
}

fn windows_processes_elf(args: &[String]) -> Result<(), String> {
    let bundle = SnapshotBundle::elf_vmcore_file(&args[2]).map_err(|error| error.to_string())?;
    let session = VmiSession::attach(
        &DumpConnector::new(bundle, GuestArchitecture::Amd64),
        AttachRequest::any(CapabilitySet::from_caps([Capability::MemoryRead])),
    )
    .map_err(|error| error.to_string())?;
    let profile = SymbolTable::from_system_map_file(&args[3]).map_err(|error| error.to_string())?;
    let offsets = WindowsProcessOffsets {
        active_process_links: parse_number(&args[5])?,
        unique_process_id: parse_number(&args[6])?,
        image_file_name: parse_number(&args[7])?,
        image_file_name_length: usize::try_from(parse_number(&args[8])?)
            .map_err(|_| "image length is too large".to_string())?,
        directory_table_base: parse_number(&args[9])?,
    };
    let limit = usize::try_from(parse_number(&args[10])?)
        .map_err(|_| "process limit is too large".to_string())?;
    for process in WindowsIntrospector::new(
        &session,
        &Amd64Translator,
        TranslationRoot::new(parse_number(&args[4])?),
        &profile,
        offsets,
    )
    .processes(limit)
    .map_err(|error| error.to_string())?
    {
        println!(
            "{:>6} {} eprocess={} dtb={:#x}",
            process.pid, process.image, process.eprocess, process.directory_table_base
        );
    }
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_decimal_and_both_hex_prefixes() {
        assert_eq!(parse_number("42").unwrap(), 42);
        assert_eq!(parse_number("0x2a").unwrap(), 42);
        assert_eq!(parse_number("0X2A").unwrap(), 42);
        assert_eq!(parse_number("18446744073709551615").unwrap(), u64::MAX);
    }

    #[test]
    fn rejects_empty_negative_and_overflowing_numbers() {
        for value in ["", "0x", "0X", "-1", "18446744073709551616", "0xg"] {
            let error = parse_number(value).unwrap_err();
            assert!(error.contains("invalid number"), "{value}: {error}");
        }
    }

    #[test]
    fn usage_is_complete_and_handles_an_empty_argument_vector() {
        let error = run_args(&[]).unwrap_err();
        for command in [
            "read-kdmp",
            "read-vmware-vmem",
            "read-vmware-core",
            "qemu-event",
            "qemu-gdb-reg-write",
            "vbox-status",
            "vbox-reg-read",
            "vbox-reg-write",
            "profile-pdb-offset",
            "linux-processes-elf",
            "windows-processes-elf",
        ] {
            assert!(error.contains(command), "usage omitted {command}");
        }
        assert!(error.contains("vmi-cli read-raw"));
    }

    #[test]
    fn collects_unicode_operating_system_arguments() {
        assert_eq!(
            collect_args([OsString::from("vmi-cli"), OsString::from("테스트")]).unwrap(),
            ["vmi-cli", "테스트"]
        );
    }

    #[test]
    fn collect_args_enforces_count_and_size_limits() {
        let excessive_count =
            (0..=MAX_ARGUMENT_COUNT).map(|index| OsString::from(index.to_string()));
        assert!(collect_args(excessive_count)
            .unwrap_err()
            .contains("argument count exceeds"));

        let excessive_length = OsString::from("x".repeat(MAX_ARGUMENT_BYTES + 1));
        assert!(collect_args([OsString::from("vmi-cli"), excessive_length])
            .unwrap_err()
            .contains("argument 1 exceeds"));

        let maximum_length = OsString::from("x".repeat(MAX_ARGUMENT_BYTES));
        assert_eq!(
            collect_args([OsString::from("vmi-cli"), maximum_length])
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn hex_dump_rejects_line_address_overflow() {
        assert!(print_bytes(u64::MAX, &[0; 17])
            .unwrap_err()
            .contains("address overflow"));
        assert!(print_bytes(u64::MAX, &[0]).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn rejects_non_unicode_operating_system_arguments_without_panicking() {
        use std::os::unix::ffi::OsStringExt;

        let error = collect_args([OsString::from_vec(vec![0xff])]).unwrap_err();
        assert!(error.contains("argument 0"));
    }
}
