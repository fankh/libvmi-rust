#![no_main]

use libfuzzer_sys::fuzz_target;
use vmi_artifact::SnapshotBundle;

fuzz_target!(|data: &[u8]| {
    let _ = SnapshotBundle::from_kdmp("fuzz", data);
    let _ = SnapshotBundle::from_lime("fuzz", data);
});
