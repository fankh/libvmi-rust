#include "vmi.h"

#include <stdio.h>
#include <stdlib.h>

static void print_last_error(void) {
    size_t length = vmi_last_error(NULL, 0);
    char *message = malloc(length);
    if (message != NULL && vmi_last_error(message, length) == length) {
        fprintf(stderr, "libvmi-rust: %s\n", message);
    }
    free(message);
}

int main(int argc, char **argv) {
    if (argc != 4) {
        fprintf(stderr, "usage: %s RAW_FILE GPA LENGTH\n", argv[0]);
        return 2;
    }
    uint64_t address = strtoull(argv[2], NULL, 0);
    size_t length = (size_t)strtoull(argv[3], NULL, 0);
    uint8_t *bytes = malloc(length == 0 ? 1 : length);
    vmi_snapshot_t *snapshot = NULL;
    vmi_status_t status = vmi_snapshot_open(
        argv[1], VMI_ARTIFACT_RAW, 0, &snapshot);
    if (status != VMI_STATUS_OK) {
        print_last_error();
        free(bytes);
        return 1;
    }
    status = vmi_snapshot_read(snapshot, address, bytes, length);
    if (status != VMI_STATUS_OK) {
        print_last_error();
        vmi_snapshot_close(snapshot);
        free(bytes);
        return 1;
    }
    for (size_t index = 0; index < length; ++index) {
        printf("%02x%s", bytes[index], index + 1 == length ? "\n" : " ");
    }
    vmi_snapshot_close(snapshot);
    free(bytes);
    return 0;
}
