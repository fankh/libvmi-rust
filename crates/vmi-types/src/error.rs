use thiserror::Error;

use crate::{Capability, CapabilitySet};

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ScalarDecodeError {
    #[error("expected {expected} bytes, got {actual}")]
    WrongWidth { expected: usize, actual: usize },
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum VmiErrorKind {
    Attach,
    Capability,
    Unsupported,
    Read,
    Translation,
    Decode,
    Timeout,
    Cancelled,
    Backend,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum VmiError {
    #[error(
        "provider {provider} rejected attach because required capabilities are missing: {missing}"
    )]
    AttachRejected {
        provider: String,
        missing: CapabilitySet,
    },
    #[error("provider {provider} does not expose capability {capability}")]
    CapabilityMissing {
        provider: String,
        capability: Capability,
    },
    #[error("operation {operation} is not supported by provider {provider}")]
    UnsupportedOperation {
        provider: String,
        operation: &'static str,
    },
    #[error("read failed at {address} for {length} bytes")]
    ReadFailed { address: u64, length: usize },
    #[error("virtual address {address:#x} is not canonical for {bits}-bit addressing")]
    NonCanonicalAddress { address: u64, bits: u8 },
    #[error("page-table entry is not present at level {level} for virtual address {address:#x}")]
    PageNotPresent { address: u64, level: u8 },
    #[error("invalid page-table entry {entry:#x} at level {level}")]
    InvalidPageTableEntry { entry: u64, level: u8 },
    #[error("scalar decode failed: {0}")]
    ScalarDecode(ScalarDecodeError),
    #[error("operation {operation} timed out")]
    Timeout { operation: &'static str },
    #[error("operation {operation} was cancelled")]
    Cancelled { operation: &'static str },
    #[error("backend error: {0}")]
    Backend(String),
}

impl VmiError {
    #[must_use]
    pub const fn kind(&self) -> VmiErrorKind {
        match self {
            Self::AttachRejected { .. } => VmiErrorKind::Attach,
            Self::CapabilityMissing { .. } => VmiErrorKind::Capability,
            Self::UnsupportedOperation { .. } => VmiErrorKind::Unsupported,
            Self::ReadFailed { .. } => VmiErrorKind::Read,
            Self::NonCanonicalAddress { .. }
            | Self::PageNotPresent { .. }
            | Self::InvalidPageTableEntry { .. } => VmiErrorKind::Translation,
            Self::ScalarDecode(_) => VmiErrorKind::Decode,
            Self::Timeout { .. } => VmiErrorKind::Timeout,
            Self::Cancelled { .. } => VmiErrorKind::Cancelled,
            Self::Backend(_) => VmiErrorKind::Backend,
        }
    }
}

impl From<ScalarDecodeError> for VmiError {
    fn from(value: ScalarDecodeError) -> Self {
        Self::ScalarDecode(value)
    }
}

pub type Result<T> = core::result::Result<T, VmiError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_categories_are_stable_and_exhaustive() {
        let cases = [
            (
                VmiError::AttachRejected {
                    provider: "p".into(),
                    missing: CapabilitySet::empty(),
                },
                VmiErrorKind::Attach,
            ),
            (
                VmiError::CapabilityMissing {
                    provider: "p".into(),
                    capability: Capability::Events,
                },
                VmiErrorKind::Capability,
            ),
            (
                VmiError::UnsupportedOperation {
                    provider: "p".into(),
                    operation: "operation",
                },
                VmiErrorKind::Unsupported,
            ),
            (
                VmiError::ReadFailed {
                    address: 0,
                    length: 1,
                },
                VmiErrorKind::Read,
            ),
            (
                VmiError::NonCanonicalAddress {
                    address: 0,
                    bits: 48,
                },
                VmiErrorKind::Translation,
            ),
            (
                VmiError::ScalarDecode(ScalarDecodeError::WrongWidth {
                    expected: 1,
                    actual: 0,
                }),
                VmiErrorKind::Decode,
            ),
            (
                VmiError::Timeout {
                    operation: "operation",
                },
                VmiErrorKind::Timeout,
            ),
            (
                VmiError::Cancelled {
                    operation: "operation",
                },
                VmiErrorKind::Cancelled,
            ),
            (VmiError::Backend("failure".into()), VmiErrorKind::Backend),
        ];
        for (error, expected) in cases {
            assert_eq!(error.kind(), expected);
        }
    }
}
