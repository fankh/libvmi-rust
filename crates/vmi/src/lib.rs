//! Public facade for the native Rust VMI workspace.
//!
//! The facade re-exports portable address types, provider contracts, artifact
//! readers, architecture translators, and supported provider connectors.
//!
//! ```
//! use std::sync::Arc;
//!
//! use vmi::{
//!     artifact::SnapshotBundle, driver::DumpConnector, AttachRequest, Capability,
//!     CapabilitySet, Gpa, GuestArchitecture, VmiSession,
//! };
//!
//! let bundle = SnapshotBundle::from_raw(
//!     "example.raw",
//!     Gpa::new(0),
//!     Arc::from([0x56, 0x4d, 0x49, 0x21]),
//! );
//! let connector = DumpConnector::new(bundle, GuestArchitecture::Amd64);
//! let session = VmiSession::attach(
//!     &connector,
//!     AttachRequest::any(CapabilitySet::from_caps([Capability::MemoryRead])),
//! )?;
//! assert_eq!(session.read_bytes(Gpa::new(0), 4)?, b"VMI!");
//! # Ok::<(), vmi::VmiError>(())
//! ```

pub mod arch {
    pub use vmi_arch_aarch64::Aarch64Translator;
    pub use vmi_arch_amd64::{Amd64La57Translator, Amd64Translator};
    pub use vmi_arch_api::{AddressTranslator, Translation};
}

pub mod artifact {
    pub use vmi_artifact::{ArtifactProvenance, KdmpMetadata, SnapshotBundle, SnapshotSegment};
}

pub mod driver {
    pub use vmi_driver_api::{
        AcquisitionAccess, CancellationToken, Connector, ControlAccess, CpuAccess, EventAccess,
        ExecutionState, LifecycleEvent, MemoryAccess, MemoryWriteAccess, Session, TargetLifecycle,
        ViewAccess, VmiEvent,
    };
    pub use vmi_driver_dump::DumpConnector;
    pub use vmi_driver_libvirt::LibvirtConnector;
    pub use vmi_driver_qemu::QemuConnector;
    pub use vmi_driver_snapshot::SnapshotConnector;
    pub use vmi_driver_virtualbox::VirtualBoxConnector;
    pub use vmi_driver_xen::XenConnector;
}

pub mod os {
    pub use vmi_os_linux::{
        LinuxDentryOffsets, LinuxFileOffsets, LinuxIntrospector, LinuxModule, LinuxModuleOffsets,
        LinuxMountOffsets, LinuxOpenFile, LinuxProcess, LinuxSocket, LinuxSocketHashOffsets,
        LinuxSocketListOffsets, LinuxSocketOffsets, LinuxTaskOffsets,
    };
    pub use vmi_os_windows::{
        WindowsFile, WindowsFileOffsets, WindowsHandle, WindowsHandleTableOffsets,
        WindowsIntrospector, WindowsModule, WindowsModuleOffsets, WindowsProcess,
        WindowsProcessOffsets,
    };
}

pub use vmi_core::{ProviderRegistry, VmiSession};
pub use vmi_events::EventQueue;
pub use vmi_profile::{Profile, Symbol, SymbolTable};
pub use vmi_types::{
    AttachRequest, ByteOrder, Capability, CapabilitySet, ConsistencyMode, Gpa, GuestArchitecture,
    Gva, MemoryRange, ProviderDescriptor, ProviderMaturity, Result, TargetDescriptor,
    TargetSelector, TranslationRoot, VmiError, VmiErrorKind,
};
pub use vmi_views::MemoryViewManager;

pub mod prelude {
    pub use crate::arch::{AddressTranslator, Translation};
    pub use crate::driver::{Connector, MemoryAccess, Session};
    pub use crate::{
        AttachRequest, Capability, CapabilitySet, Gpa, Gva, ProviderRegistry, Result,
        TranslationRoot, VmiSession,
    };
}

#[cfg(test)]
mod tests {
    use super::prelude::*;
    use vmi_testkit::FakeConnector;

    #[test]
    fn prelude_supports_portable_attach_and_read() {
        let connector = FakeConnector::default().with_segment(0x1000_u64, vec![1, 2, 3, 4]);
        let session = VmiSession::attach(
            &connector,
            AttachRequest::any(CapabilitySet::from_caps([Capability::MemoryRead])),
        )
        .unwrap();
        assert_eq!(
            session.read_bytes(Gpa::new(0x1000), 4).unwrap(),
            [1, 2, 3, 4]
        );
    }
}
