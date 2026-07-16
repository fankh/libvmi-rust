use std::{
    cell::RefCell,
    collections::HashMap,
    ffi::CStr,
    os::raw::c_char,
    panic::{catch_unwind, AssertUnwindSafe},
    path::Path,
    ptr, slice,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex, OnceLock,
    },
};

use vmi_artifact::SnapshotBundle;
use vmi_types::Gpa;

pub const ABI_VERSION: u32 = 1;
const MAX_PATH_BYTES: usize = 32 * 1024;

#[repr(i32)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum VmiStatus {
    Ok = 0,
    InvalidArgument = 1,
    ArtifactError = 2,
    ReadError = 3,
    BufferTooSmall = 4,
    Panic = 255,
}

#[repr(u32)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum VmiArtifactFormat {
    Raw = 0,
    ElfVmcore = 1,
    Lime = 2,
    Manifest = 3,
    XenCore = 4,
    Kdmp = 5,
}

#[repr(C)]
pub struct VmiSnapshot {
    bundle: SnapshotBundle,
}

thread_local! {
    static LAST_ERROR: RefCell<String> = const { RefCell::new(String::new()) };
}

static LIVE_SNAPSHOTS: OnceLock<Mutex<HashMap<usize, Arc<VmiSnapshot>>>> = OnceLock::new();
static NEXT_SNAPSHOT: AtomicUsize = AtomicUsize::new(1);

fn live_snapshots() -> &'static Mutex<HashMap<usize, Arc<VmiSnapshot>>> {
    LIVE_SNAPSHOTS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn next_snapshot_token(counter: &AtomicUsize) -> Option<usize> {
    let mut current = counter.load(Ordering::Relaxed);
    loop {
        let next = current.checked_add(1)?;
        match counter.compare_exchange_weak(current, next, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return Some(current),
            Err(observed) => current = observed,
        }
    }
}

fn validate_snapshot(snapshot: *const VmiSnapshot) -> Result<Arc<VmiSnapshot>, VmiStatus> {
    if snapshot.is_null() {
        set_error("snapshot must not be null");
        return Err(VmiStatus::InvalidArgument);
    }
    let handles = live_snapshots().lock().map_err(|error| {
        set_error(format!("snapshot handle registry failed: {error}"));
        VmiStatus::Panic
    })?;
    handles.get(&snapshot.addr()).cloned().ok_or_else(|| {
        set_error("snapshot handle is not live");
        VmiStatus::InvalidArgument
    })
}

fn set_error(message: impl Into<String>) {
    LAST_ERROR.with(|error| *error.borrow_mut() = message.into());
}

fn clear_snapshot_output(output: *mut *mut VmiSnapshot) {
    // SAFETY: callers validate that `output` is a writable non-null pointer
    // before invoking this helper.
    unsafe { *output = ptr::null_mut() };
}

fn guarded(operation: impl FnOnce() -> VmiStatus) -> VmiStatus {
    match catch_unwind(AssertUnwindSafe(operation)) {
        Ok(status) => status,
        Err(_) => {
            set_error("panic crossed the C ABI boundary");
            VmiStatus::Panic
        }
    }
}

#[no_mangle]
pub extern "C" fn vmi_abi_version() -> u32 {
    ABI_VERSION
}

/// Opens an immutable memory artifact.
///
/// # Safety
/// `path` must point to a NUL-terminated string and `out_snapshot` must be a
/// writable pointer. The returned handle should be closed when no longer used.
#[no_mangle]
pub unsafe extern "C" fn vmi_snapshot_open(
    path: *const c_char,
    format: u32,
    raw_base: u64,
    out_snapshot: *mut *mut VmiSnapshot,
) -> VmiStatus {
    guarded(|| {
        if path.is_null() || out_snapshot.is_null() {
            set_error("path and out_snapshot must not be null");
            return VmiStatus::InvalidArgument;
        }
        // SAFETY: guaranteed by this function's contract.
        let path = match unsafe { CStr::from_ptr(path) }.to_str() {
            Ok(path) if !path.is_empty() => path,
            Ok(_) => {
                clear_snapshot_output(out_snapshot);
                set_error("path must not be empty");
                return VmiStatus::InvalidArgument;
            }
            Err(error) => {
                clear_snapshot_output(out_snapshot);
                set_error(format!("path is not valid UTF-8: {error}"));
                return VmiStatus::InvalidArgument;
            }
        };
        if path.len() > MAX_PATH_BYTES {
            clear_snapshot_output(out_snapshot);
            set_error(format!("path exceeds {MAX_PATH_BYTES} bytes"));
            return VmiStatus::InvalidArgument;
        }
        let mut owned_path = String::new();
        if let Err(error) = owned_path.try_reserve_exact(path.len()) {
            clear_snapshot_output(out_snapshot);
            set_error(format!("failed to allocate artifact path: {error}"));
            return VmiStatus::Panic;
        }
        owned_path.push_str(path);
        // SAFETY: the caller guarantees a writable output pointer. The path is
        // owned before this write, so overlapping C input/output storage cannot
        // invalidate a borrowed Rust string.
        clear_snapshot_output(out_snapshot);
        let result = match format {
            0 => SnapshotBundle::raw_file(Path::new(&owned_path), Gpa::new(raw_base)),
            1 => SnapshotBundle::elf_vmcore_file(Path::new(&owned_path)),
            2 => SnapshotBundle::lime_file(Path::new(&owned_path)),
            3 => SnapshotBundle::manifest_file(Path::new(&owned_path)),
            4 => SnapshotBundle::xen_core_file(Path::new(&owned_path)),
            5 => SnapshotBundle::kdmp_file(Path::new(&owned_path)),
            _ => {
                set_error(format!("unknown artifact format {format}"));
                return VmiStatus::InvalidArgument;
            }
        };
        match result {
            Ok(bundle) => {
                let mut handles = match live_snapshots().lock() {
                    Ok(handles) => handles,
                    Err(error) => {
                        set_error(format!("snapshot handle registry failed: {error}"));
                        return VmiStatus::Panic;
                    }
                };
                if let Err(error) = handles.try_reserve(1) {
                    set_error(format!("failed to grow snapshot handle registry: {error}"));
                    return VmiStatus::Panic;
                }
                let Some(token) = next_snapshot_token(&NEXT_SNAPSHOT) else {
                    set_error("snapshot handle space exhausted");
                    return VmiStatus::Panic;
                };
                handles.insert(token, Arc::new(VmiSnapshot { bundle }));
                // SAFETY: the output pointer was validated. The non-dereferenceable
                // token is used only as an opaque C handle.
                unsafe { *out_snapshot = ptr::with_exposed_provenance_mut(token) };
                set_error("");
                VmiStatus::Ok
            }
            Err(error) => {
                set_error(error.to_string());
                VmiStatus::ArtifactError
            }
        }
    })
}

/// Reads guest-physical bytes from an artifact.
///
/// # Safety
/// `snapshot` must be a live handle returned by `vmi_snapshot_open`; `output`
/// must be writable for `output_length` bytes.
#[no_mangle]
pub unsafe extern "C" fn vmi_snapshot_read(
    snapshot: *const VmiSnapshot,
    guest_physical_address: u64,
    output: *mut u8,
    output_length: usize,
) -> VmiStatus {
    guarded(|| {
        if output.is_null() && output_length != 0 {
            set_error("snapshot and non-empty output must not be null");
            return VmiStatus::InvalidArgument;
        }
        if output_length > usize::try_from(isize::MAX).unwrap_or(usize::MAX) {
            set_error("snapshot output length exceeds Rust slice limits");
            return VmiStatus::InvalidArgument;
        }
        let handle = match validate_snapshot(snapshot) {
            Ok(handle) => handle,
            Err(status) => return status,
        };
        // SAFETY: guaranteed by this function's contract; a null pointer is
        // accepted only for the zero-length slice.
        let output = if output_length == 0 {
            &mut []
        } else {
            // SAFETY: the caller guarantees a writable non-null buffer and the
            // length was checked against Rust's maximum slice length above.
            unsafe { slice::from_raw_parts_mut(output, output_length) }
        };
        let bundle = &handle.bundle;
        match bundle.read_into(Gpa::new(guest_physical_address), output) {
            Ok(()) => {
                set_error("");
                VmiStatus::Ok
            }
            Err(error) => {
                set_error(error.to_string());
                VmiStatus::ReadError
            }
        }
    })
}

/// Returns the number of physical segments in an artifact.
///
/// # Safety
/// Both pointers must be valid, and `snapshot` must refer to a live handle.
#[no_mangle]
pub unsafe extern "C" fn vmi_snapshot_segment_count(
    snapshot: *const VmiSnapshot,
    out_count: *mut usize,
) -> VmiStatus {
    guarded(|| {
        if out_count.is_null() {
            set_error("snapshot and out_count must not be null");
            return VmiStatus::InvalidArgument;
        }
        // SAFETY: the caller guarantees a writable non-null output pointer.
        unsafe { *out_count = 0 };
        let handle = match validate_snapshot(snapshot) {
            Ok(handle) => handle,
            Err(status) => return status,
        };
        // SAFETY: guaranteed by this function's contract.
        unsafe { *out_count = handle.bundle.segments().len() };
        set_error("");
        VmiStatus::Ok
    })
}

/// Returns one physical segment's start address and length.
///
/// # Safety
/// All pointers must be valid, and `snapshot` must refer to a live handle.
#[no_mangle]
pub unsafe extern "C" fn vmi_snapshot_segment(
    snapshot: *const VmiSnapshot,
    index: usize,
    out_start: *mut u64,
    out_length: *mut u64,
) -> VmiStatus {
    guarded(|| {
        if out_start.is_null() || out_length.is_null() {
            set_error("snapshot and segment outputs must not be null");
            return VmiStatus::InvalidArgument;
        }
        if ptr::eq(out_start, out_length) {
            // SAFETY: the shared destination is writable by contract.
            unsafe { *out_start = 0 };
            set_error("segment start and length outputs must not alias");
            return VmiStatus::InvalidArgument;
        }
        // SAFETY: both pointers are writable and were verified distinct.
        unsafe {
            *out_start = 0;
            *out_length = 0;
        }
        let handle = match validate_snapshot(snapshot) {
            Ok(handle) => handle,
            Err(status) => return status,
        };
        let segments = handle.bundle.segments();
        let Some(segment) = segments.get(index) else {
            set_error(format!("segment index {index} is out of range"));
            return VmiStatus::InvalidArgument;
        };
        // SAFETY: output pointers are guaranteed writable by the caller.
        unsafe {
            *out_start = segment.range.start.raw();
            *out_length = segment.range.length;
        }
        set_error("");
        VmiStatus::Ok
    })
}

/// Releases an artifact handle. Passing null is allowed.
///
/// # Safety
/// Foreign, stale, and already-closed handles are rejected without dereferencing them.
#[no_mangle]
pub unsafe extern "C" fn vmi_snapshot_close(snapshot: *mut VmiSnapshot) {
    if catch_unwind(AssertUnwindSafe(|| {
        if snapshot.is_null() {
            set_error("");
            return;
        }
        let mut handles = match live_snapshots().lock() {
            Ok(handles) => handles,
            Err(error) => {
                set_error(format!("snapshot handle registry failed: {error}"));
                return;
            }
        };
        if handles.remove(&snapshot.addr()).is_none() {
            set_error("snapshot handle is not live");
            return;
        }
        set_error("");
    }))
    .is_err()
    {
        set_error("panic crossed the C ABI boundary while closing snapshot");
    }
}

/// Copies the calling thread's last error, including a trailing NUL when the
/// supplied buffer is large enough. The return value is the required size.
///
/// # Safety
/// A non-null `output` must be writable for `output_length` bytes.
#[no_mangle]
pub unsafe extern "C" fn vmi_last_error(output: *mut c_char, output_length: usize) -> usize {
    LAST_ERROR.with(|error| {
        let error = error.borrow();
        let required = error.len().saturating_add(1);
        if !output.is_null() && output_length >= required {
            // SAFETY: guaranteed by this function's contract and size check.
            unsafe {
                ptr::copy_nonoverlapping(error.as_ptr(), output.cast::<u8>(), error.len());
                *output.add(error.len()) = 0;
            }
        }
        required
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        ffi::CString,
        fs,
        sync::Barrier,
        thread,
        time::{SystemTime, UNIX_EPOCH},
    };

    fn last_error_text() -> String {
        // SAFETY: a null output with zero length is the documented size query.
        let required = unsafe { vmi_last_error(ptr::null_mut(), 0) };
        let mut bytes = vec![0i8; required];
        // SAFETY: `bytes` is writable for the supplied length.
        unsafe { vmi_last_error(bytes.as_mut_ptr(), bytes.len()) };
        // SAFETY: the exactly sized call writes a trailing NUL.
        unsafe { CStr::from_ptr(bytes.as_ptr()) }
            .to_str()
            .unwrap()
            .to_owned()
    }

    #[test]
    fn snapshot_tokens_never_wrap() {
        let counter = AtomicUsize::new(usize::MAX - 1);
        assert_eq!(next_snapshot_token(&counter), Some(usize::MAX - 1));
        assert_eq!(next_snapshot_token(&counter), None);
        assert_eq!(next_snapshot_token(&counter), None);
        assert_eq!(counter.load(Ordering::Relaxed), usize::MAX);
    }

    #[test]
    fn validated_snapshot_outlives_registry_removal_without_holding_the_lock() {
        let token = next_snapshot_token(&NEXT_SNAPSHOT).unwrap();
        let snapshot = Arc::new(VmiSnapshot {
            bundle: SnapshotBundle::from_raw(
                "retained.raw",
                Gpa::new(0),
                Arc::<[u8]>::from([0x56, 0x4d, 0x49, 0x21]),
            ),
        });
        live_snapshots()
            .lock()
            .unwrap()
            .insert(token, Arc::clone(&snapshot));
        let handle = ptr::with_exposed_provenance_mut::<VmiSnapshot>(token);
        let retained = validate_snapshot(handle).unwrap();

        // SAFETY: `handle` is the live opaque token inserted above. Successful
        // removal while `retained` exists proves validation released the lock.
        unsafe { vmi_snapshot_close(handle) };
        assert!(validate_snapshot(handle).is_err());
        let mut output = [0; 4];
        retained.bundle.read_into(Gpa::new(0), &mut output).unwrap();
        assert_eq!(&output, b"VMI!");
    }

    #[test]
    fn concurrent_reads_and_close_have_safe_lifetime_semantics() {
        let token = next_snapshot_token(&NEXT_SNAPSHOT).unwrap();
        live_snapshots().lock().unwrap().insert(
            token,
            Arc::new(VmiSnapshot {
                bundle: SnapshotBundle::from_raw(
                    "concurrent.raw",
                    Gpa::new(0),
                    Arc::<[u8]>::from([0x56, 0x4d, 0x49, 0x21]),
                ),
            }),
        );
        let barrier = Arc::new(Barrier::new(17));
        let readers = (0..16)
            .map(|_| {
                let barrier = Arc::clone(&barrier);
                thread::spawn(move || {
                    barrier.wait();
                    let handle = ptr::with_exposed_provenance::<VmiSnapshot>(token);
                    let mut output = [0; 4];
                    // SAFETY: the opaque token is either live or concurrently
                    // removed; validation precedes all snapshot access and the
                    // output buffer is writable for four bytes.
                    let status = unsafe { vmi_snapshot_read(handle, 0, output.as_mut_ptr(), 4) };
                    assert!(matches!(status, VmiStatus::Ok | VmiStatus::InvalidArgument));
                    if status == VmiStatus::Ok {
                        assert_eq!(&output, b"VMI!");
                    }
                })
            })
            .collect::<Vec<_>>();
        barrier.wait();
        let handle = ptr::with_exposed_provenance_mut::<VmiSnapshot>(token);
        // SAFETY: the opaque token was inserted above; racing readers either
        // retained an `Arc` during validation or will reject the removed token.
        unsafe { vmi_snapshot_close(handle) };
        for reader in readers {
            reader.join().unwrap();
        }
        assert!(validate_snapshot(handle).is_err());
    }

    #[test]
    fn opens_reads_describes_and_closes_raw_artifact() {
        let path = std::env::temp_dir().join(format!(
            "vmi-ffi-{}.raw",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::write(&path, [1, 2, 3, 4]).unwrap();
        let path_c = CString::new(path.to_str().unwrap()).unwrap();
        let mut handle = ptr::null_mut();
        // SAFETY: the C string and output pointer remain valid for the call.
        assert_eq!(
            // SAFETY: the C string and output pointer remain valid for the call.
            unsafe { vmi_snapshot_open(path_c.as_ptr(), 0, 0x1000, &mut handle) },
            VmiStatus::Ok
        );
        let mut output = [0; 2];
        // SAFETY: `handle` is live and `output` is writable for its full length.
        assert_eq!(
            // SAFETY: `handle` is live and `output` is writable for its full length.
            unsafe { vmi_snapshot_read(handle, 0x1001, output.as_mut_ptr(), output.len()) },
            VmiStatus::Ok
        );
        assert_eq!(output, [2, 3]);
        let (mut start, mut length) = (0, 0);
        // SAFETY: `handle` is live and both scalar outputs are writable.
        assert_eq!(
            // SAFETY: `handle` is live and both scalar outputs are writable.
            unsafe { vmi_snapshot_segment(handle, 0, &mut start, &mut length) },
            VmiStatus::Ok
        );
        assert_eq!((start, length), (0x1000, 4));

        start = 0xaaaa;
        length = 0xbbbb;
        // SAFETY: the handle is live and outputs are distinct and writable;
        // the deliberately invalid index is rejected after zeroing them.
        assert_eq!(
            // SAFETY: all pointers remain valid for the call.
            unsafe { vmi_snapshot_segment(handle, 1, &mut start, &mut length) },
            VmiStatus::InvalidArgument
        );
        assert_eq!((start, length), (0, 0));

        let mut aliased = 0xcccc_u64;
        let aliased_pointer = &mut aliased as *mut u64;
        // SAFETY: the handle is live and the writable aliased outputs are
        // intentionally supplied to exercise explicit alias rejection.
        assert_eq!(
            // SAFETY: the shared output is valid for either scalar write.
            unsafe { vmi_snapshot_segment(handle, 0, aliased_pointer, aliased_pointer) },
            VmiStatus::InvalidArgument
        );
        assert_eq!(aliased, 0);

        // SAFETY: `handle` is the live token returned above.
        unsafe { vmi_snapshot_close(handle) };
        let mut count = usize::MAX;
        // SAFETY: stale handles are validated without dereferencing and count
        // remains a valid writable output.
        assert_eq!(
            // SAFETY: the stale token is registry-checked; output is writable.
            unsafe { vmi_snapshot_segment_count(handle, &mut count) },
            VmiStatus::InvalidArgument
        );
        assert_eq!(count, 0);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn rejects_bad_arguments_and_reports_thread_local_error() {
        let mut handle = ptr::null_mut();
        // SAFETY: the deliberately null input is permitted to reach validation;
        // the output pointer itself is writable.
        assert_eq!(
            // SAFETY: null input reaches validation; the output pointer is writable.
            unsafe { vmi_snapshot_open(ptr::null(), 0, 0, &mut handle) },
            VmiStatus::InvalidArgument
        );
        // SAFETY: a null output with zero length is the documented size query.
        let required = unsafe { vmi_last_error(ptr::null_mut(), 0) };
        let mut error = vec![0i8; required];
        // SAFETY: `error` is writable for the supplied length.
        assert_eq!(
            // SAFETY: `error` is writable for the supplied length.
            unsafe { vmi_last_error(error.as_mut_ptr(), error.len()) },
            required
        );
        // SAFETY: `vmi_last_error` wrote a trailing NUL into the sized buffer.
        assert!(unsafe { CStr::from_ptr(error.as_ptr()) }
            .to_str()
            .unwrap()
            .contains("must not be null"));
        // SAFETY: this deliberately invalid length must be rejected before the
        // dangling output pointer is used.
        assert_eq!(
            // SAFETY: the oversized length is rejected before pointer use.
            unsafe {
                vmi_snapshot_read(
                    ptr::null(),
                    0,
                    ptr::dangling_mut::<u8>(),
                    isize::MAX as usize + 1,
                )
            },
            VmiStatus::InvalidArgument
        );
    }

    #[test]
    fn open_clears_output_on_every_post_pointer_validation_failure() {
        let empty = CString::new("").unwrap();
        let mut output = ptr::dangling_mut::<VmiSnapshot>();
        // SAFETY: the empty C string and writable output pointer are valid;
        // validation rejects the empty value.
        assert_eq!(
            // SAFETY: both pointers remain valid for the call.
            unsafe { vmi_snapshot_open(empty.as_ptr(), 0, 0, &mut output) },
            VmiStatus::InvalidArgument
        );
        assert!(output.is_null());

        let invalid_utf8 = [0xff_u8, 0];
        output = ptr::dangling_mut::<VmiSnapshot>();
        // SAFETY: the byte array is NUL-terminated and the output is writable;
        // UTF-8 validation rejects its contents.
        assert_eq!(
            // SAFETY: both pointers remain valid for the call.
            unsafe { vmi_snapshot_open(invalid_utf8.as_ptr().cast::<c_char>(), 0, 0, &mut output) },
            VmiStatus::InvalidArgument
        );
        assert!(output.is_null());

        let oversized = CString::new(vec![b'a'; MAX_PATH_BYTES + 1]).unwrap();
        output = ptr::dangling_mut::<VmiSnapshot>();
        // SAFETY: both pointers remain valid; length validation rejects the
        // oversized C string before filesystem access.
        assert_eq!(
            // SAFETY: both pointers remain valid for the call.
            unsafe { vmi_snapshot_open(oversized.as_ptr(), 0, 0, &mut output) },
            VmiStatus::InvalidArgument
        );
        assert!(output.is_null());

        let unused = CString::new("unused").unwrap();
        output = ptr::dangling_mut::<VmiSnapshot>();
        // SAFETY: both pointers remain valid; the unknown format is rejected.
        assert_eq!(
            // SAFETY: both pointers remain valid for the call.
            unsafe { vmi_snapshot_open(unused.as_ptr(), u32::MAX, 0, &mut output) },
            VmiStatus::InvalidArgument
        );
        assert!(output.is_null());

        let missing = CString::new("definitely-missing-vmi-artifact.raw").unwrap();
        output = ptr::dangling_mut::<VmiSnapshot>();
        // SAFETY: both pointers remain valid; artifact loading reports the
        // nonexistent path without initializing a handle.
        assert_eq!(
            // SAFETY: both pointers remain valid for the call.
            unsafe { vmi_snapshot_open(missing.as_ptr(), 0, 0, &mut output) },
            VmiStatus::ArtifactError
        );
        assert!(output.is_null());
    }

    #[test]
    fn last_error_is_thread_local_exact_sized_and_cleared_by_null_close() {
        set_error("main-thread-error");
        let worker = thread::spawn(|| {
            assert_eq!(last_error_text(), "");
            let mut handle = ptr::null_mut();
            // SAFETY: null path input reaches validation and the output pointer
            // remains valid for the call.
            assert_eq!(
                // SAFETY: null input is intentionally passed to validation and
                // `handle` remains a valid writable output pointer.
                unsafe { vmi_snapshot_open(ptr::null(), 0, 0, &mut handle) },
                VmiStatus::InvalidArgument
            );
            assert!(last_error_text().contains("must not be null"));
        });
        worker.join().unwrap();
        assert_eq!(last_error_text(), "main-thread-error");

        set_error("abc");
        let mut undersized = [0x7f_i8; 3];
        // SAFETY: `undersized` is writable for the supplied length; an
        // undersized destination must remain untouched.
        assert_eq!(
            // SAFETY: the destination is writable for its supplied length.
            unsafe { vmi_last_error(undersized.as_mut_ptr(), undersized.len()) },
            4
        );
        assert_eq!(undersized, [0x7f_i8; 3]);
        let mut exact = [0x7f_i8; 4];
        // SAFETY: `exact` is writable for the required four bytes.
        assert_eq!(
            // SAFETY: the destination is writable for its supplied length.
            unsafe { vmi_last_error(exact.as_mut_ptr(), exact.len()) },
            4
        );
        assert_eq!(exact.map(|byte| u8::try_from(byte).unwrap()), *b"abc\0");

        // SAFETY: closing null is a documented successful no-op.
        unsafe { vmi_snapshot_close(ptr::null_mut()) };
        assert_eq!(last_error_text(), "");
    }

    #[test]
    fn rejects_stale_foreign_and_double_closed_handles() {
        let path = std::env::temp_dir().join(format!(
            "vmi-ffi-stale-{}.raw",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::write(&path, [1]).unwrap();
        let path_c = CString::new(path.to_str().unwrap()).unwrap();
        let mut handle = ptr::null_mut();
        // SAFETY: the C string and output pointer remain valid for the call.
        assert_eq!(
            // SAFETY: the C string and output pointer remain valid for the call.
            unsafe { vmi_snapshot_open(path_c.as_ptr(), 0, 0, &mut handle) },
            VmiStatus::Ok
        );
        // SAFETY: `handle` is the live token returned above.
        unsafe { vmi_snapshot_close(handle) };
        // SAFETY: stale tokens are accepted as values and rejected without
        // dereferencing; this exercises the repeated-close boundary.
        unsafe { vmi_snapshot_close(handle) };
        let mut output = 0;
        // SAFETY: the stale token must be validated before use; output is valid.
        assert_eq!(
            // SAFETY: the stale token is registry-checked; output is valid.
            unsafe { vmi_snapshot_read(handle, 0, &mut output, 1) },
            VmiStatus::InvalidArgument
        );
        // SAFETY: foreign tokens are registry-checked without dereferencing;
        // output remains writable for one byte.
        assert_eq!(
            // SAFETY: the foreign token is registry-checked; output is valid.
            unsafe { vmi_snapshot_read(ptr::dangling::<VmiSnapshot>(), 0, &mut output, 1) },
            VmiStatus::InvalidArgument
        );
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn open_owns_bounded_path_before_writing_overlapping_output() {
        let path = std::env::temp_dir().join(format!(
            "vmi-ffi-alias-{}.raw",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::write(&path, [0x5a]).unwrap();
        let path_c = CString::new(path.to_str().unwrap()).unwrap();
        let storage_words = path_c
            .as_bytes_with_nul()
            .len()
            .div_ceil(std::mem::size_of::<usize>());
        let mut storage = vec![0usize; storage_words];
        // SAFETY: `storage` has enough writable bytes for the complete C string.
        unsafe {
            ptr::copy_nonoverlapping(
                path_c.as_ptr().cast::<u8>(),
                storage.as_mut_ptr().cast::<u8>(),
                path_c.as_bytes_with_nul().len(),
            );
        }
        let path_pointer = storage.as_ptr().cast::<c_char>();
        let output_pointer = storage.as_mut_ptr().cast::<*mut VmiSnapshot>();
        // SAFETY: the shared storage is aligned and large enough for both the
        // NUL-terminated path input and output handle. The API must own the
        // path before overwriting the intentionally overlapping output.
        assert_eq!(
            // SAFETY: both intentionally overlapping pointers refer to the
            // aligned, sufficiently large `storage` allocation described above.
            unsafe { vmi_snapshot_open(path_pointer, 0, 0, output_pointer) },
            VmiStatus::Ok
        );
        // SAFETY: the successful call initialized the aligned output slot.
        let handle = unsafe { *output_pointer };
        assert!(!handle.is_null());
        // SAFETY: `handle` is the live opaque token returned by the call.
        unsafe { vmi_snapshot_close(handle) };

        let oversized = CString::new(vec![b'a'; MAX_PATH_BYTES + 1]).unwrap();
        let mut rejected = ptr::null_mut();
        // SAFETY: both pointers remain valid; the oversized path is rejected
        // before filesystem access.
        assert_eq!(
            // SAFETY: both pointers are valid for the duration of the call.
            unsafe { vmi_snapshot_open(oversized.as_ptr(), 0, 0, &mut rejected) },
            VmiStatus::InvalidArgument
        );
        fs::remove_file(path).unwrap();
    }
}
