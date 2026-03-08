use std::ffi::c_void;
use std::sync::Once;

type HMODULE = *mut c_void;
type FARPROC = unsafe extern "system" fn() -> isize;

extern "system" {
    fn LoadLibraryA(name: *const u8) -> HMODULE;
    fn GetProcAddress(module: HMODULE, name: *const u8) -> FARPROC;
}

static INIT: Once = Once::new();
static mut SPDLOG_ORIG: HMODULE = std::ptr::null_mut();

/// Load spdlog_orig.dll (real exports) and frida-gadget.dll (instrumentation).
/// Called lazily on first game call to spdlog::default_logger_raw().
unsafe fn ensure_init() {
    INIT.call_once(|| unsafe {
        SPDLOG_ORIG = LoadLibraryA(b"spdlog_orig.dll\0".as_ptr());
        LoadLibraryA(b"frida-gadget.dll\0".as_ptr());
    });
}

/// Hijacked export: spdlog::default_logger_raw()
/// Signature: spdlog::logger* spdlog::default_logger_raw() — no args, returns pointer.
/// All other 391 exports are forwarded via the .def file.
#[export_name = "?default_logger_raw@spdlog@@YAPEAVlogger@1@XZ"]
pub unsafe extern "C" fn hijacked_default_logger_raw() -> *mut c_void {
    unsafe {
        ensure_init();
        let proc = GetProcAddress(
            SPDLOG_ORIG,
            b"?default_logger_raw@spdlog@@YAPEAVlogger@1@XZ\0".as_ptr(),
        );
        let f: unsafe extern "C" fn() -> *mut c_void = std::mem::transmute(proc);
        f()
    }
}
