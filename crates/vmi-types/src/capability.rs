use core::fmt;

use bitflags::bitflags;

bitflags! {
    #[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash)]
    pub struct CapabilitySet: u64 {
        const MEMORY_READ = 1 << 0;
        const MEMORY_WRITE = 1 << 1;
        const REGISTER_READ = 1 << 2;
        const REGISTER_WRITE = 1 << 3;
        const CONTROL = 1 << 4;
        const EVENTS = 1 << 5;
        const MEMORY_VIEW = 1 << 6;
        const ACQUISITION = 1 << 7;
        const LIFECYCLE = 1 << 8;
    }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
pub enum Capability {
    MemoryRead,
    MemoryWrite,
    RegisterRead,
    RegisterWrite,
    Control,
    Events,
    MemoryView,
    Acquisition,
    Lifecycle,
}

impl Capability {
    pub const ALL: [Capability; 9] = [
        Capability::MemoryRead,
        Capability::MemoryWrite,
        Capability::RegisterRead,
        Capability::RegisterWrite,
        Capability::Control,
        Capability::Events,
        Capability::MemoryView,
        Capability::Acquisition,
        Capability::Lifecycle,
    ];

    pub const fn bit(self) -> CapabilitySet {
        match self {
            Capability::MemoryRead => CapabilitySet::MEMORY_READ,
            Capability::MemoryWrite => CapabilitySet::MEMORY_WRITE,
            Capability::RegisterRead => CapabilitySet::REGISTER_READ,
            Capability::RegisterWrite => CapabilitySet::REGISTER_WRITE,
            Capability::Control => CapabilitySet::CONTROL,
            Capability::Events => CapabilitySet::EVENTS,
            Capability::MemoryView => CapabilitySet::MEMORY_VIEW,
            Capability::Acquisition => CapabilitySet::ACQUISITION,
            Capability::Lifecycle => CapabilitySet::LIFECYCLE,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Capability::MemoryRead => "memory_read",
            Capability::MemoryWrite => "memory_write",
            Capability::RegisterRead => "register_read",
            Capability::RegisterWrite => "register_write",
            Capability::Control => "control",
            Capability::Events => "events",
            Capability::MemoryView => "memory_view",
            Capability::Acquisition => "acquisition",
            Capability::Lifecycle => "lifecycle",
        }
    }
}

impl CapabilitySet {
    pub fn from_caps(caps: impl IntoIterator<Item = Capability>) -> Self {
        let mut set = Self::empty();
        for cap in caps {
            set.insert(cap.bit());
        }
        set
    }

    pub fn contains_capability(self, capability: Capability) -> bool {
        self.contains(capability.bit())
    }

    pub fn insert_capability(&mut self, capability: Capability) {
        self.insert(capability.bit());
    }

    pub fn difference_of(self, other: Self) -> Self {
        self.difference(other)
    }

    pub fn iter_capabilities(self) -> impl Iterator<Item = Capability> {
        Capability::ALL
            .into_iter()
            .filter(move |cap| self.contains_capability(*cap))
    }
}

impl fmt::Display for Capability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl fmt::Display for CapabilitySet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut iter = self.iter_capabilities();
        if let Some(first) = iter.next() {
            f.write_str(first.as_str())?;
            for cap in iter {
                f.write_str(",")?;
                f.write_str(cap.as_str())?;
            }
            Ok(())
        } else {
            f.write_str("<empty>")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capability_set_iterates_in_declaration_order() {
        let set = CapabilitySet::from_caps([Capability::MemoryRead, Capability::Control]);
        let collected: Vec<_> = set.iter_capabilities().collect();
        assert_eq!(collected, vec![Capability::MemoryRead, Capability::Control]);
    }
}
