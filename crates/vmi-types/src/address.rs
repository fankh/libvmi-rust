use core::fmt;

#[derive(Copy, Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
#[repr(transparent)]
/// A guest-physical address.
///
/// Physical and virtual addresses are intentionally distinct types. The
/// compiler rejects accidentally passing a [`Gva`] to an API that requires a
/// `Gpa`:
///
/// ```compile_fail
/// use vmi_types::{Gpa, Gva};
///
/// fn read_physical(_address: Gpa) {}
/// read_physical(Gva::new(0x1000));
/// ```
pub struct Gpa(u64);

impl Gpa {
    pub const fn new(raw: u64) -> Self {
        Self(raw)
    }

    pub const fn raw(self) -> u64 {
        self.0
    }
}

impl From<u64> for Gpa {
    fn from(value: u64) -> Self {
        Self::new(value)
    }
}

impl From<Gpa> for u64 {
    fn from(value: Gpa) -> Self {
        value.raw()
    }
}

impl fmt::Display for Gpa {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "0x{:x}", self.0)
    }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
#[repr(transparent)]
/// A guest-virtual address.
///
/// Translation roots are also kept separate from virtual addresses:
///
/// ```compile_fail
/// use vmi_types::{Gva, TranslationRoot};
///
/// fn inspect_virtual(_address: Gva) {}
/// inspect_virtual(TranslationRoot::new(0x1000));
/// ```
pub struct Gva(u64);

impl Gva {
    pub const fn new(raw: u64) -> Self {
        Self(raw)
    }

    pub const fn raw(self) -> u64 {
        self.0
    }
}

impl From<u64> for Gva {
    fn from(value: u64) -> Self {
        Self::new(value)
    }
}

impl From<Gva> for u64 {
    fn from(value: Gva) -> Self {
        value.raw()
    }
}

impl fmt::Display for Gva {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "0x{:x}", self.0)
    }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
#[repr(transparent)]
pub struct TranslationRoot(u64);

impl TranslationRoot {
    pub const fn new(raw: u64) -> Self {
        Self(raw)
    }

    pub const fn raw(self) -> u64 {
        self.0
    }
}

impl From<u64> for TranslationRoot {
    fn from(value: u64) -> Self {
        Self::new(value)
    }
}

impl From<TranslationRoot> for u64 {
    fn from(value: TranslationRoot) -> Self {
        value.raw()
    }
}

impl fmt::Display for TranslationRoot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "0x{:x}", self.0)
    }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
pub struct MemoryRange {
    pub start: Gpa,
    pub length: u64,
}

impl MemoryRange {
    pub const fn new(start: Gpa, length: u64) -> Self {
        Self { start, length }
    }

    pub fn end(self) -> Option<Gpa> {
        self.start.raw().checked_add(self.length).map(Gpa::from)
    }

    pub fn contains(self, address: Gpa) -> bool {
        let start = u128::from(self.start.raw());
        let end = start.saturating_add(u128::from(self.length));
        let value = u128::from(address.raw());
        value >= start && value < end
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn address_newtypes_round_trip(raw in any::<u64>()) {
            prop_assert_eq!(Gpa::new(raw).raw(), raw);
            prop_assert_eq!(Gva::new(raw).raw(), raw);
            prop_assert_eq!(TranslationRoot::new(raw).raw(), raw);
            prop_assert_eq!(u64::from(Gpa::from(raw)), raw);
            prop_assert_eq!(u64::from(Gva::from(raw)), raw);
            prop_assert_eq!(u64::from(TranslationRoot::from(raw)), raw);
        }

        #[test]
        fn memory_range_contains_exact_half_open_interval(
            start in any::<u64>(),
            length in any::<u64>(),
            address in any::<u64>(),
        ) {
            let range = MemoryRange::new(Gpa::new(start), length);
            let relative = address.checked_sub(start);
            let expected = relative.is_some_and(|offset| offset < length);
            prop_assert_eq!(range.contains(Gpa::new(address)), expected);
            prop_assert_eq!(range.end().map(Gpa::raw), start.checked_add(length));
        }
    }

    #[test]
    fn memory_range_contains_the_last_address_without_wrapping() {
        let final_byte = MemoryRange::new(Gpa::new(u64::MAX), 1);
        assert!(final_byte.contains(Gpa::new(u64::MAX)));
        assert_eq!(final_byte.end(), None);

        let final_two = MemoryRange::new(Gpa::new(u64::MAX - 1), 2);
        assert!(final_two.contains(Gpa::new(u64::MAX - 1)));
        assert!(final_two.contains(Gpa::new(u64::MAX)));
        assert!(!MemoryRange::new(Gpa::new(u64::MAX), 0).contains(Gpa::new(u64::MAX)));
    }
}
