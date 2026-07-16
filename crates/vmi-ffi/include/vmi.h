#ifndef LIBVMI_RUST_VMI_H
#define LIBVMI_RUST_VMI_H

#include <stddef.h>
#include <stdint.h>

#if defined(_WIN32) && defined(VMI_FFI_EXPORTS)
#define VMI_API __declspec(dllexport)
#elif defined(_WIN32) && !defined(VMI_STATIC)
#define VMI_API __declspec(dllimport)
#else
#define VMI_API
#endif

#ifdef __cplusplus
extern "C" {
#endif

typedef struct vmi_snapshot vmi_snapshot_t;

typedef enum vmi_status {
    VMI_STATUS_OK = 0,
    VMI_STATUS_INVALID_ARGUMENT = 1,
    VMI_STATUS_ARTIFACT_ERROR = 2,
    VMI_STATUS_READ_ERROR = 3,
    VMI_STATUS_BUFFER_TOO_SMALL = 4,
    VMI_STATUS_PANIC = 255
} vmi_status_t;

typedef enum vmi_artifact_format {
    VMI_ARTIFACT_RAW = 0,
    VMI_ARTIFACT_ELF_VMCORE = 1,
    VMI_ARTIFACT_LIME = 2,
    VMI_ARTIFACT_MANIFEST = 3,
    VMI_ARTIFACT_XEN_CORE = 4,
    VMI_ARTIFACT_KDMP = 5
} vmi_artifact_format_t;

VMI_API uint32_t vmi_abi_version(void);
VMI_API vmi_status_t vmi_snapshot_open(
    const char *path,
    vmi_artifact_format_t format,
    uint64_t raw_base,
    vmi_snapshot_t **out_snapshot);
VMI_API vmi_status_t vmi_snapshot_read(
    const vmi_snapshot_t *snapshot,
    uint64_t guest_physical_address,
    uint8_t *output,
    size_t output_length);
/* out_count is zeroed before handle validation. */
VMI_API vmi_status_t vmi_snapshot_segment_count(
    const vmi_snapshot_t *snapshot,
    size_t *out_count);
/* out_start and out_length must be distinct. Valid scalar outputs are zeroed
 * before handle/index validation, so failures never expose stale values. */
VMI_API vmi_status_t vmi_snapshot_segment(
    const vmi_snapshot_t *snapshot,
    size_t index,
    uint64_t *out_start,
    uint64_t *out_length);
VMI_API void vmi_snapshot_close(vmi_snapshot_t *snapshot);
/* Returns the calling thread's required error-buffer size including NUL.
 * Writes only when output_length is large enough; undersized buffers remain
 * untouched. Passing NULL is a size query when output_length is zero. */
VMI_API size_t vmi_last_error(char *output, size_t output_length);

#ifdef __cplusplus
}
#endif

#endif
