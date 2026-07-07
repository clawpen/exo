#ifndef EXO_VMM_H
#define EXO_VMM_H

#include <stdint.h>
#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct exo_vm *exo_vm_t;

// Create a new VM configuration. Returns NULL on error.
// The returned handle must be freed with exo_vm_free.
exo_vm_t exo_vm_create(const char *kernel_path,
                       const char *initrd_path,
                       const char *disk_path,
                       const char *console_log_path,
                       uint64_t memory_bytes,
                       uint32_t cpu_count,
                       const char *vm_name);

// Free a VM handle. Stops the VM if running and tears down the runloop thread.
void exo_vm_free(exo_vm_t vm);

// Start the VM. Returns 0 on success, -1 on error.
int exo_vm_start(exo_vm_t vm);

// Request a graceful stop. Returns 0 on success, -1 on error.
int exo_vm_stop(exo_vm_t vm);

// Returns 1 if the VM is currently running, 0 otherwise.
int exo_vm_is_running(exo_vm_t vm);

// Return a human-readable description of the last error. The returned string
// is owned by the VM handle and must not be freed by the caller.
const char* exo_vm_last_error(exo_vm_t vm);

// Send a JSON request to the guest agent on the given vsock port and wait for
// a single newline-terminated JSON response. On success, *json_out is set to a
// malloc'd C string that the caller must free with exo_vm_free_string.
// Returns 0 on success, -1 on error.
int exo_vm_request(exo_vm_t vm, uint32_t port, const char *json_in,
                   char **json_out, uint32_t timeout_ms);

// Free a C string returned by exo_vm_request.
void exo_vm_free_string(char *s);

// Return the host-side file descriptors for the RPC serial port.
// *read_fd receives data written by the guest; *write_fd is for writing data to the guest.
// Returns 0 on success, -1 if the VM has no RPC serial port.
int exo_vm_rpc_fds(exo_vm_t vm, int *read_fd, int *write_fd);

#ifdef __cplusplus
}
#endif

#endif // EXO_VMM_H
