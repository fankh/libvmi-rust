#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum GuestArchitecture {
    Amd64,
    Aarch64,
    Unknown,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum ConsistencyMode {
    LiveBestEffort,
    Paused,
    ImmutableSnapshot,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TargetDescriptor {
    pub id: String,
    pub display_name: Option<String>,
    pub architecture: GuestArchitecture,
    pub consistency: ConsistencyMode,
}

impl TargetDescriptor {
    pub fn new(
        id: impl Into<String>,
        display_name: Option<impl Into<String>>,
        architecture: GuestArchitecture,
        consistency: ConsistencyMode,
    ) -> Self {
        Self {
            id: id.into(),
            display_name: display_name.map(Into::into),
            architecture,
            consistency,
        }
    }
}
