use std::env;

use vmi::{
    artifact::SnapshotBundle, driver::DumpConnector, AttachRequest, Capability, CapabilitySet, Gpa,
    GuestArchitecture, Result, VmiSession,
};

fn parse_number(value: &str) -> Result<u64> {
    let parsed = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
        .map_or_else(|| value.parse(), |digits| u64::from_str_radix(digits, 16))
        .map_err(|error| vmi::VmiError::Backend(format!("invalid number {value:?}: {error}")))?;
    Ok(parsed)
}

fn main() -> Result<()> {
    let mut arguments = env::args().skip(1);
    let path = arguments
        .next()
        .ok_or_else(|| vmi::VmiError::Backend("usage: inspect_raw <file> <gpa> <length>".into()))?;
    let address = parse_number(&arguments.next().ok_or_else(|| {
        vmi::VmiError::Backend("usage: inspect_raw <file> <gpa> <length>".into())
    })?)?;
    let length = usize::try_from(parse_number(&arguments.next().ok_or_else(|| {
        vmi::VmiError::Backend("usage: inspect_raw <file> <gpa> <length>".into())
    })?)?)
    .map_err(|_| vmi::VmiError::Backend("length does not fit this host".into()))?;
    if arguments.next().is_some() {
        return Err(vmi::VmiError::Backend(
            "usage: inspect_raw <file> <gpa> <length>".into(),
        ));
    }

    let bundle = SnapshotBundle::raw_file(path, Gpa::new(0))?;
    let connector = DumpConnector::new(bundle, GuestArchitecture::Amd64);
    let session = VmiSession::attach(
        &connector,
        AttachRequest::any(CapabilitySet::from_caps([Capability::MemoryRead])),
    )?;
    let inspected = session.read_bytes(Gpa::new(address), length)?;
    for byte in inspected {
        print!("{byte:02x}");
    }
    println!();
    Ok(())
}
