#include "vmi.h"

#include <stdio.h>
#include <string.h>

static int fail(const char *operation, vmi_status_t status) {
    char error[512] = {0};
    const size_t required = vmi_last_error(error, sizeof(error));
    fprintf(stderr, "%s failed with status %d (error bytes %zu): %s\n",
            operation, (int)status, required, error);
    return 1;
}

int main(int argc, char **argv) {
    if (argc != 2) {
        fprintf(stderr, "usage: %s <raw-memory-file>\n", argv[0]);
        return 2;
    }
    if (vmi_abi_version() != 1) {
        fprintf(stderr, "unexpected ABI version %u\n", vmi_abi_version());
        return 1;
    }

    vmi_snapshot_t *snapshot = NULL;
    vmi_status_t status = vmi_snapshot_open(
        argv[1], VMI_ARTIFACT_RAW, UINT64_C(0x1000), &snapshot);
    if (status != VMI_STATUS_OK) {
        return fail("vmi_snapshot_open", status);
    }

    size_t count = 0;
    status = vmi_snapshot_segment_count(snapshot, &count);
    if (status != VMI_STATUS_OK) {
        vmi_snapshot_close(snapshot);
        return fail("vmi_snapshot_segment_count", status);
    }
    if (count != 1) {
        fprintf(stderr, "unexpected segment count %zu\n", count);
        vmi_snapshot_close(snapshot);
        return 1;
    }

    uint64_t start = 0;
    uint64_t length = 0;
    status = vmi_snapshot_segment(snapshot, 0, &start, &length);
    if (status != VMI_STATUS_OK) {
        vmi_snapshot_close(snapshot);
        return fail("vmi_snapshot_segment", status);
    }
    if (start != UINT64_C(0x1000) || length != 4) {
        fprintf(stderr, "unexpected segment 0x%llx+%llu\n",
                (unsigned long long)start, (unsigned long long)length);
        vmi_snapshot_close(snapshot);
        return 1;
    }

    uint8_t bytes[4] = {0};
    status = vmi_snapshot_read(snapshot, start, bytes, sizeof(bytes));
    if (status != VMI_STATUS_OK) {
        vmi_snapshot_close(snapshot);
        return fail("vmi_snapshot_read", status);
    }
    const uint8_t expected[4] = {'V', 'M', 'I', '!'};
    if (memcmp(bytes, expected, sizeof(bytes)) != 0) {
        fprintf(stderr, "unexpected memory contents\n");
        vmi_snapshot_close(snapshot);
        return 1;
    }

    vmi_snapshot_close(snapshot);
    puts("C ABI smoke test passed");
    return 0;
}
