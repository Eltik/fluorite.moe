use std::ffi::c_void;

type HMODULE = *mut c_void;

extern "system" {
    fn LoadLibraryA(name: *const u8) -> HMODULE;
}

/// CRT initializer — tries to load frida-gadget.dll.
/// If the DLL isn't present, LoadLibraryA returns NULL and we continue
/// silently — the game runs normally.
unsafe extern "C" fn load_gadget() {
    unsafe {
        LoadLibraryA(b"frida-gadget.dll\0".as_ptr());
    }
}

#[used]
#[link_section = ".CRT$XCU"]
static INIT: unsafe extern "C" fn() = load_gadget;
