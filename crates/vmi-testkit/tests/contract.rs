use std::time::Duration;

use vmi_driver_api::{read_bytes, Connector, ExecutionState, VmiEvent};
use vmi_testkit::FakeConnector;
use vmi_types::{AttachRequest, ByteOrder, Capability, CapabilitySet, Gpa};

#[test]
fn attach_rejects_missing_capabilities_before_session_is_created() {
    let connector = FakeConnector::default();
    let request = AttachRequest::any(CapabilitySet::from_caps([
        Capability::MemoryRead,
        Capability::RegisterRead,
    ]));

    let error = match connector.connect(request) {
        Ok(_) => panic!("attach should have been rejected"),
        Err(error) => error,
    };
    match error {
        vmi_types::VmiError::AttachRejected { provider, missing } => {
            assert_eq!(provider, "fake-read-only");
            assert!(missing.contains_capability(Capability::RegisterRead));
            assert!(!missing.contains_capability(Capability::MemoryRead));
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[test]
fn memory_reads_work_for_supported_provider() {
    let connector = FakeConnector::default().with_segment(0x1000_u64, vec![0x34, 0x12]);
    let session = connector
        .connect(AttachRequest::any(CapabilitySet::from_caps([
            Capability::MemoryRead,
        ])))
        .unwrap();

    let memory = session.memory().unwrap();
    let value =
        vmi_driver_api::read_scalar::<u16>(memory, Gpa::from(0x1000), ByteOrder::LittleEndian)
            .unwrap();

    assert_eq!(value, 0x1234);
}

#[test]
fn unsupported_facets_fail_closed() {
    let connector = FakeConnector::default();
    let session = connector.connect(AttachRequest::default()).unwrap();

    assert!(matches!(
        session.cpu(),
        Err(vmi_types::VmiError::CapabilityMissing { .. })
    ));
    assert!(matches!(
        session.control(),
        Err(vmi_types::VmiError::CapabilityMissing { .. })
    ));
    assert!(matches!(
        session.events(),
        Err(vmi_types::VmiError::CapabilityMissing { .. })
    ));
}

#[test]
fn memory_writes_are_capability_gated_and_persist() {
    let capabilities = CapabilitySet::from_caps([Capability::MemoryRead, Capability::MemoryWrite]);
    let connector = FakeConnector::default()
        .with_capabilities(capabilities)
        .with_segment(0x1000_u64, vec![0u8; 4]);
    let session = connector.connect(AttachRequest::any(capabilities)).unwrap();
    session
        .memory_write()
        .unwrap()
        .write(Gpa::new(0x1001), &[7, 8])
        .unwrap();
    assert_eq!(
        read_bytes(session.memory().unwrap(), Gpa::new(0x1000), 4).unwrap(),
        [0, 7, 8, 0]
    );

    let read_only = FakeConnector::default()
        .connect(AttachRequest::default())
        .unwrap();
    assert!(matches!(
        read_only.memory_write(),
        Err(vmi_types::VmiError::CapabilityMissing { .. })
    ));
}

#[test]
fn every_advertised_fake_facet_operates_deterministically() {
    let capabilities = CapabilitySet::from_caps(Capability::ALL);
    let connector = FakeConnector::default()
        .with_capabilities(capabilities)
        .with_segment(0x1000_u64, vec![1, 2, 3, 4])
        .with_register(0, "rip", 0x1234)
        .with_event(VmiEvent {
            kind: "breakpoint".into(),
            vcpu: Some(0),
            address: Some(Gpa::new(0x1000)),
        });
    let session = connector.connect(AttachRequest::any(capabilities)).unwrap();

    let cpu = session.cpu().unwrap();
    assert_eq!(cpu.read_register(0, "rip").unwrap(), 0x1234);
    cpu.write_register(0, "rip", 0x5678).unwrap();
    assert_eq!(cpu.read_register(0, "rip").unwrap(), 0x5678);

    let control = session.control().unwrap();
    assert_eq!(control.execution_state().unwrap(), ExecutionState::Running);
    control.pause().unwrap();
    assert_eq!(control.execution_state().unwrap(), ExecutionState::Paused);
    control.resume().unwrap();

    let event = session
        .events()
        .unwrap()
        .next_event(Duration::ZERO)
        .unwrap()
        .unwrap();
    assert_eq!(event.kind, "breakpoint");
    assert!(session
        .events()
        .unwrap()
        .next_event(Duration::ZERO)
        .unwrap()
        .is_none());

    let views = session.views().unwrap();
    assert_eq!(views.active_view().unwrap(), 0);
    views.switch_view(7).unwrap();
    assert_eq!(views.active_view().unwrap(), 7);

    let directory = std::env::temp_dir().join(format!(
        "vmi-testkit-acquisition-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let range = directory.join("range.bin");
    let snapshot = directory.join("snapshot.bin");
    let acquisition = session.acquisition().unwrap();
    acquisition
        .save_physical_range(&range, Gpa::new(0x1001), 2)
        .unwrap();
    acquisition.save_snapshot(&snapshot).unwrap();
    assert_eq!(std::fs::read(&range).unwrap(), [2, 3]);
    assert_eq!(std::fs::read(&snapshot).unwrap().len(), 20);
    std::fs::remove_dir_all(directory).unwrap();

    let lifecycle = session.lifecycle().unwrap();
    assert_eq!(lifecycle.generation(), 1);
    assert!(lifecycle
        .next_lifecycle_event(Duration::ZERO)
        .unwrap()
        .is_none());
}

#[test]
fn read_and_register_directions_are_independently_gated() {
    let write_only = CapabilitySet::from_caps([Capability::MemoryWrite, Capability::RegisterWrite]);
    let session = FakeConnector::default()
        .with_capabilities(write_only)
        .with_segment(0_u64, vec![0])
        .connect(AttachRequest::any(write_only))
        .unwrap();
    assert!(session.memory().is_err());
    session
        .memory_write()
        .unwrap()
        .write(Gpa::new(0), &[1])
        .unwrap();
    assert!(session.cpu().unwrap().read_register(0, "rip").is_err());
    session.cpu().unwrap().write_register(0, "rip", 1).unwrap();
}

#[test]
fn injected_memory_faults_have_deterministic_partial_semantics() {
    let capabilities = CapabilitySet::from_caps([Capability::MemoryRead, Capability::MemoryWrite]);
    let session = FakeConnector::default()
        .with_capabilities(capabilities)
        .with_segment(0x1000_u64, vec![1, 2, 3, 4])
        .with_read_fault(0x1002_u64)
        .with_write_fault(0x1002_u64)
        .connect(AttachRequest::any(capabilities))
        .unwrap();

    let mut read = [0xaa; 4];
    let error = session
        .memory()
        .unwrap()
        .read_into(Gpa::new(0x1000), &mut read)
        .unwrap_err();
    assert!(error.to_string().contains("read fault at 0x1002"));
    assert_eq!(read, [1, 2, 0xaa, 0xaa]);

    let error = session
        .memory_write()
        .unwrap()
        .write(Gpa::new(0x1000), &[9, 8, 7, 6])
        .unwrap_err();
    assert!(error.to_string().contains("write fault at 0x1002"));

    let mut prefix = [0; 2];
    session
        .memory()
        .unwrap()
        .read_into(Gpa::new(0x1000), &mut prefix)
        .unwrap();
    assert_eq!(prefix, [9, 8]);
}

#[test]
fn sparse_memory_contract_covers_boundaries_holes_and_overflow() {
    let session = FakeConnector::default()
        .with_segment(0x1001_u64, [1, 2])
        .with_segment(0x1003_u64, [3, 4])
        .with_segment(u64::MAX, [0xff])
        .connect(AttachRequest::default())
        .unwrap();
    let memory = session.memory().unwrap();

    let mut empty = [];
    memory.read_into(Gpa::new(u64::MAX), &mut empty).unwrap();
    assert_eq!(
        read_bytes(memory, Gpa::new(0x1001), 4).unwrap(),
        [1, 2, 3, 4]
    );
    assert_eq!(read_bytes(memory, Gpa::new(u64::MAX), 1).unwrap(), [0xff]);

    let mut across_overflow = [0xaa; 2];
    assert!(matches!(
        memory.read_into(Gpa::new(u64::MAX), &mut across_overflow),
        Err(vmi_types::VmiError::ReadFailed { .. })
    ));
    assert_eq!(across_overflow, [0xff, 0xaa]);

    let hole = FakeConnector::default()
        .with_segment(0x2000_u64, [1])
        .with_segment(0x2002_u64, [3])
        .connect(AttachRequest::default())
        .unwrap();
    let mut sparse = [0xaa; 3];
    assert!(matches!(
        hole.memory()
            .unwrap()
            .read_into(Gpa::new(0x2000), &mut sparse),
        Err(vmi_types::VmiError::ReadFailed {
            address: 0x2001,
            length: 1
        })
    ));
    assert_eq!(sparse, [1, 0xaa, 0xaa]);
}

#[test]
fn fake_connector_rejects_ambiguous_sparse_maps_and_unknown_targets() {
    let overlapping = FakeConnector::default()
        .with_segment(0x1000_u64, [1, 2])
        .with_segment(0x1001_u64, [3, 4]);
    assert!(matches!(
        overlapping.connect(AttachRequest::default()),
        Err(vmi_types::VmiError::Backend(message)) if message.contains("overlap")
    ));

    let overflowing = FakeConnector::default().with_segment(u64::MAX, [1, 2]);
    assert!(matches!(
        overflowing.connect(AttachRequest::default()),
        Err(vmi_types::VmiError::Backend(message)) if message.contains("address space")
    ));

    let request = AttachRequest {
        selector: vmi_types::TargetSelector::Named("missing-target".into()),
        ..AttachRequest::default()
    };
    assert!(matches!(
        FakeConnector::default().connect(request),
        Err(vmi_types::VmiError::Backend(message)) if message.contains("not found")
    ));
}

#[test]
fn sparse_write_contract_covers_empty_holes_and_overflow() {
    let capabilities = CapabilitySet::from_caps([Capability::MemoryRead, Capability::MemoryWrite]);
    let session = FakeConnector::default()
        .with_capabilities(capabilities)
        .with_segment(0x3001_u64, [1, 2])
        .with_segment(u64::MAX, [3])
        .connect(AttachRequest::any(capabilities))
        .unwrap();
    let writer = session.memory_write().unwrap();

    writer.write(Gpa::new(u64::MAX), &[]).unwrap();
    writer.write(Gpa::new(0x3001), &[7, 8]).unwrap();
    assert_eq!(
        read_bytes(session.memory().unwrap(), Gpa::new(0x3001), 2).unwrap(),
        [7, 8]
    );

    assert!(matches!(
        writer.write(Gpa::new(0x3002), &[9, 10]),
        Err(vmi_types::VmiError::ReadFailed {
            address: 0x3003,
            length: 1
        })
    ));
    assert_eq!(
        read_bytes(session.memory().unwrap(), Gpa::new(0x3002), 1).unwrap(),
        [9]
    );

    assert!(matches!(
        writer.write(Gpa::new(u64::MAX), &[4, 5]),
        Err(vmi_types::VmiError::ReadFailed { .. })
    ));
    assert_eq!(
        read_bytes(session.memory().unwrap(), Gpa::new(u64::MAX), 1).unwrap(),
        [4]
    );
}

#[test]
fn acquisition_failure_preserves_destination_and_cleans_temporary_files() {
    let capabilities = CapabilitySet::from_caps([Capability::MemoryRead, Capability::Acquisition]);
    let session = FakeConnector::default()
        .with_capabilities(capabilities)
        .with_segment(0x4000_u64, [1, 2])
        .with_read_fault(0x4001_u64)
        .connect(AttachRequest::any(capabilities))
        .unwrap();
    let directory = std::env::temp_dir().join(format!(
        "vmi-testkit-atomic-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let destination = directory.join("range.bin");
    std::fs::write(&destination, b"original").unwrap();

    assert!(session
        .acquisition()
        .unwrap()
        .save_physical_range(&destination, Gpa::new(0x4000), 2)
        .is_err());
    assert_eq!(std::fs::read(&destination).unwrap(), b"original");
    assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 1);
    std::fs::remove_dir_all(directory).unwrap();
}
