#![no_main]

use libfuzzer_sys::fuzz_target;
use vmi_profile::{Profile, SymbolTable};

fuzz_target!(|data: &[u8]| {
    if let Ok(text) = std::str::from_utf8(data) {
        let _ = SymbolTable::from_system_map(text);
        let _ = Profile::from_json(text);
    }
});
