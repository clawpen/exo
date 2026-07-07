use std::os::raw::{c_char, c_int};

/// Opaque handle to the Objective-C VM object.
#[repr(C)]
pub struct ExoVmHandle {
    _private: [u8; 0],
}

#[cfg(target_os = "macos")]
extern "C" {
    pub fn exo_vm_create(
        kernel_path: *const c_char,
        initrd_path: *const c_char,
        disk_path: *const c_char,
        console_log_path: *const c_char,
        memory_bytes: u64,
        cpu_count: u32,
        vm_name: *const c_char,
    ) -> *mut ExoVmHandle;

    pub fn exo_vm_free(vm: *mut ExoVmHandle);
    pub fn exo_vm_start(vm: *mut ExoVmHandle) -> c_int;
    pub fn exo_vm_stop(vm: *mut ExoVmHandle) -> c_int;
    pub fn exo_vm_is_running(vm: *mut ExoVmHandle) -> c_int;
    pub fn exo_vm_last_error(vm: *mut ExoVmHandle) -> *const c_char;
    pub fn exo_vm_request(
        vm: *mut ExoVmHandle,
        port: u32,
        json_in: *const c_char,
        json_out: *mut *mut c_char,
        timeout_ms: u32,
    ) -> c_int;
    pub fn exo_vm_rpc_fds(vm: *mut ExoVmHandle, read_fd: *mut c_int, write_fd: *mut c_int)
        -> c_int;
    pub fn exo_vm_free_string(s: *mut c_char);
}

/// Stubs for non-macOS builds so the crate compiles on Linux/Windows.
#[cfg(not(target_os = "macos"))]
pub mod stub {
    use super::*;
    use std::ptr;

    #[no_mangle]
    pub extern "C" fn exo_vm_create(
        _kernel_path: *const c_char,
        _initrd_path: *const c_char,
        _disk_path: *const c_char,
        _console_log_path: *const c_char,
        _memory_bytes: u64,
        _cpu_count: u32,
        _vm_name: *const c_char,
    ) -> *mut ExoVmHandle {
        ptr::null_mut()
    }

    #[no_mangle]
    pub extern "C" fn exo_vm_free(_vm: *mut ExoVmHandle) {}

    #[no_mangle]
    pub extern "C" fn exo_vm_start(_vm: *mut ExoVmHandle) -> c_int {
        -1
    }

    #[no_mangle]
    pub extern "C" fn exo_vm_stop(_vm: *mut ExoVmHandle) -> c_int {
        -1
    }

    #[no_mangle]
    pub extern "C" fn exo_vm_is_running(_vm: *mut ExoVmHandle) -> c_int {
        0
    }

    #[no_mangle]
    pub extern "C" fn exo_vm_last_error(_vm: *mut ExoVmHandle) -> *const c_char {
        "exo vm is only supported on macOS\0".as_ptr() as *const c_char
    }

    #[no_mangle]
    pub extern "C" fn exo_vm_request(
        _vm: *mut ExoVmHandle,
        _port: u32,
        _json_in: *const c_char,
        _json_out: *mut *mut c_char,
        _timeout_ms: u32,
    ) -> c_int {
        -1
    }

    #[no_mangle]
    pub extern "C" fn exo_vm_rpc_fds(
        _vm: *mut ExoVmHandle,
        _read_fd: *mut c_int,
        _write_fd: *mut c_int,
    ) -> c_int {
        -1
    }

    #[no_mangle]
    pub extern "C" fn exo_vm_free_string(_s: *mut c_char) {}
}

#[cfg(not(target_os = "macos"))]
pub use stub::*;
