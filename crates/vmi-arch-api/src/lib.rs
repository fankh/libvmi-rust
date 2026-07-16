use vmi_driver_api::MemoryAccess;
use vmi_types::{Gpa, Gva, Result, TranslationRoot};

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct Translation {
    pub physical_address: Gpa,
    pub page_size: u64,
}

impl Translation {
    pub const fn new(physical_address: Gpa, page_size: u64) -> Self {
        Self {
            physical_address,
            page_size,
        }
    }
}

pub trait AddressTranslator: Send + Sync {
    fn cache_tag(&self) -> u64;

    fn translate(
        &self,
        memory: &dyn MemoryAccess,
        root: TranslationRoot,
        address: Gva,
    ) -> Result<Translation>;
}
