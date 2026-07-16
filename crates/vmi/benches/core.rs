use std::{
    env,
    hint::black_box,
    time::{Duration, Instant},
};

use vmi::{
    arch::{AddressTranslator, Translation},
    driver::MemoryAccess,
    AttachRequest, Capability, CapabilitySet, Gpa, Gva, Result, TranslationRoot, VmiSession,
};
use vmi_testkit::FakeConnector;

const BUFFER_SIZE: usize = 16 * 1024 * 1024;

struct IdentityTranslator;

impl AddressTranslator for IdentityTranslator {
    fn cache_tag(&self) -> u64 {
        0x4245_4e43_485f_4944
    }

    fn translate(
        &self,
        _memory: &dyn MemoryAccess,
        _root: TranslationRoot,
        address: Gva,
    ) -> Result<Translation> {
        Ok(Translation::new(Gpa::new(address.raw()), 4096))
    }
}

fn iterations() -> u64 {
    env::var("VMI_BENCH_ITERATIONS")
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|value| *value > 0)
        .unwrap_or(2_000)
}

fn measure(mut operation: impl FnMut(), iterations: u64) -> Duration {
    for _ in 0..iterations.min(100) {
        operation();
    }
    let started = Instant::now();
    for _ in 0..iterations {
        operation();
    }
    started.elapsed()
}

fn main() {
    let iterations = iterations();
    let connector = FakeConnector::default().with_segment(0_u64, vec![0x5a; BUFFER_SIZE]);
    let session = VmiSession::attach(
        &connector,
        AttachRequest::any(CapabilitySet::from_caps([Capability::MemoryRead])),
    )
    .expect("benchmark session must attach");

    let mut offset = 0usize;
    let raw_elapsed = measure(
        || {
            let address = offset & (BUFFER_SIZE - 4096);
            black_box(
                session
                    .read_bytes(Gpa::new(address as u64), 4096)
                    .expect("benchmark read must succeed"),
            );
            offset = offset.wrapping_add(4096);
        },
        iterations,
    );

    let translator = IdentityTranslator;
    let translated_elapsed = measure(
        || {
            black_box(
                session
                    .translate(
                        &translator,
                        TranslationRoot::new(0),
                        Gva::new(black_box(0x1234)),
                    )
                    .expect("benchmark translation must succeed"),
            );
        },
        iterations,
    );

    let raw_ns = raw_elapsed.as_nanos() as f64 / iterations as f64;
    let translation_ns = translated_elapsed.as_nanos() as f64 / iterations as f64;
    let raw_mib_s = (4096.0 * iterations as f64 / (1024.0 * 1024.0)) / raw_elapsed.as_secs_f64();

    println!(
        "{{\"schema\":1,\"iterations\":{iterations},\"metrics\":{{\"raw_read_4k_ns\":{raw_ns:.3},\"raw_read_mib_s\":{raw_mib_s:.3},\"cached_translation_ns\":{translation_ns:.3}}}}}"
    );
}
