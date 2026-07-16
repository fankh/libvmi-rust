use crate::CapabilitySet;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProviderMaturity {
    Supported,
    Preview,
    Experimental,
    CompileOnly,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderDescriptor {
    pub id: String,
    pub display_name: String,
    pub version: Option<String>,
    pub maturity: ProviderMaturity,
    pub capabilities: CapabilitySet,
}

impl ProviderDescriptor {
    pub fn new(
        id: impl Into<String>,
        display_name: impl Into<String>,
        maturity: ProviderMaturity,
        capabilities: CapabilitySet,
    ) -> Self {
        Self {
            id: id.into(),
            display_name: display_name.into(),
            version: None,
            maturity,
            capabilities,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TargetSelector {
    Any,
    Named(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AttachRequest {
    pub selector: TargetSelector,
    pub required_capabilities: CapabilitySet,
}

impl AttachRequest {
    pub fn any(required_capabilities: CapabilitySet) -> Self {
        Self {
            selector: TargetSelector::Any,
            required_capabilities,
        }
    }

    pub fn named(name: impl Into<String>, required_capabilities: CapabilitySet) -> Self {
        Self {
            selector: TargetSelector::Named(name.into()),
            required_capabilities,
        }
    }
}

impl Default for AttachRequest {
    fn default() -> Self {
        Self::any(CapabilitySet::empty())
    }
}
