//! Hachimi plugin API (v3), resolved by name through `hachimi_get_api`.

use std::ffi::{c_char, c_void, CStr, CString};
use std::sync::OnceLock;

pub type Ptr = *mut c_void;
pub type GetApiFn = extern "C" fn(name: *const c_char) -> *mut c_void;
pub type GuiMenuSectionCallback = extern "C" fn(ui: Ptr, userdata: Ptr);

#[repr(i32)]
pub enum InitResult {
    Error = 0,
    Ok = 1,
}

pub struct Api {
    pub hachimi_instance: unsafe extern "C" fn() -> Ptr,
    pub hachimi_get_interceptor: unsafe extern "C" fn(hachimi: Ptr) -> Ptr,
    pub interceptor_hook: unsafe extern "C" fn(interceptor: Ptr, orig: Ptr, hook: Ptr) -> Ptr,
    pub il2cpp_resolve_symbol: unsafe extern "C" fn(name: *const c_char) -> Ptr,
    pub il2cpp_get_main_thread: unsafe extern "C" fn() -> Ptr,
    pub il2cpp_schedule_on_thread: unsafe extern "C" fn(thread: Ptr, callback: unsafe extern "C" fn()),
    pub il2cpp_resolve_icall: unsafe extern "C" fn(name: *const c_char) -> Ptr,
    pub il2cpp_get_assembly_image: unsafe extern "C" fn(name: *const c_char) -> Ptr,
    pub il2cpp_get_class: unsafe extern "C" fn(image: Ptr, namespace: *const c_char, name: *const c_char) -> Ptr,
    pub il2cpp_get_method_addr: unsafe extern "C" fn(class: Ptr, name: *const c_char, args: i32) -> Ptr,
    pub il2cpp_get_field_from_name: unsafe extern "C" fn(class: Ptr, name: *const c_char) -> Ptr,
    pub il2cpp_get_field_value: unsafe extern "C" fn(obj: Ptr, field: Ptr, out: *mut c_void),
    pub il2cpp_set_field_value: unsafe extern "C" fn(obj: Ptr, field: Ptr, value: *const c_void),
    pub il2cpp_create_array: unsafe extern "C" fn(class: Ptr, len: usize) -> Ptr,
    pub il2cpp_object_new: unsafe extern "C" fn(class: Ptr) -> Ptr,
    pub il2cpp_string_new: unsafe extern "C" fn(text: *const c_char) -> Ptr,
    pub il2cpp_string_chars: unsafe extern "C" fn(s: Ptr) -> *mut u16,
    pub il2cpp_string_length: unsafe extern "C" fn(s: Ptr) -> i32,
    pub log: unsafe extern "C" fn(level: i32, target: *const c_char, message: *const c_char),
    pub hachimi_get_base_dir: unsafe extern "C" fn() -> *const c_char,
    pub gui_register_menu_section: unsafe extern "C" fn(cb: Option<GuiMenuSectionCallback>, userdata: Ptr) -> bool,
    pub gui_ui_heading: unsafe extern "C" fn(ui: Ptr, text: *const c_char) -> bool,
    pub gui_ui_label: unsafe extern "C" fn(ui: Ptr, text: *const c_char) -> bool,
    pub gui_ui_checkbox: unsafe extern "C" fn(ui: Ptr, text: *const c_char, value: *mut bool) -> bool,
    pub gui_ui_button: unsafe extern "C" fn(ui: Ptr, text: *const c_char) -> bool,
}

unsafe impl Send for Api {}
unsafe impl Sync for Api {}

static API: OnceLock<Api> = OnceLock::new();

pub fn api() -> &'static Api {
    API.get().expect("darkmode: API not initialized")
}

/// Resolves every required symbol. Returns the name of the first missing one on failure.
pub fn init(get_api: GetApiFn) -> Result<(), &'static str> {
    macro_rules! resolve {
        ($($name:ident),* $(,)?) => {
            Api {
                $($name: {
                    let p = get_api(concat!(stringify!($name), "\0").as_ptr() as *const c_char);
                    if p.is_null() {
                        return Err(stringify!($name));
                    }
                    unsafe { std::mem::transmute(p) }
                },)*
            }
        };
    }

    let api = resolve!(
        hachimi_instance,
        hachimi_get_interceptor,
        interceptor_hook,
        il2cpp_resolve_symbol,
        il2cpp_get_main_thread,
        il2cpp_schedule_on_thread,
        il2cpp_resolve_icall,
        il2cpp_get_assembly_image,
        il2cpp_get_class,
        il2cpp_get_method_addr,
        il2cpp_get_field_from_name,
        il2cpp_get_field_value,
        il2cpp_set_field_value,
        il2cpp_create_array,
        il2cpp_object_new,
        il2cpp_string_new,
        il2cpp_string_chars,
        il2cpp_string_length,
        log,
        hachimi_get_base_dir,
        gui_register_menu_section,
        gui_ui_heading,
        gui_ui_label,
        gui_ui_checkbox,
        gui_ui_button,
    );
    let _ = API.set(api);
    Ok(())
}

pub fn cstring(s: &str) -> CString {
    CString::new(s.replace('\0', "")).unwrap()
}

pub fn log(level: i32, msg: &str) {
    let Some(api) = API.get() else { return };
    let msg = cstring(msg);
    unsafe { (api.log)(level, c"darkmode".as_ptr(), msg.as_ptr()) };
}

macro_rules! log_error { ($($t:tt)*) => { $crate::api::log(1, &format!($($t)*)) }; }
macro_rules! log_warn  { ($($t:tt)*) => { $crate::api::log(2, &format!($($t)*)) }; }
macro_rules! log_info  { ($($t:tt)*) => { $crate::api::log(3, &format!($($t)*)) }; }
#[allow(unused_macros)]
macro_rules! log_debug { ($($t:tt)*) => { $crate::api::log(4, &format!($($t)*)) }; }
#[allow(unused_imports)]
pub(crate) use {log_debug, log_error, log_info, log_warn};

/// Installs a hook through Hachimi's interceptor; returns the trampoline (original fn) address.
pub fn hook(orig: usize, hook: usize) -> Option<usize> {
    if orig == 0 {
        return None;
    }
    let api = api();
    unsafe {
        let interceptor = (api.hachimi_get_interceptor)((api.hachimi_instance)());
        let tramp = (api.interceptor_hook)(interceptor, orig as Ptr, hook as Ptr) as usize;
        (tramp != 0).then_some(tramp)
    }
}

pub fn base_dir() -> std::path::PathBuf {
    let p = unsafe { CStr::from_ptr((api().hachimi_get_base_dir)()) };
    std::path::PathBuf::from(p.to_string_lossy().into_owned())
}

/// Runs `callback` on Unity's main thread (posted to its SynchronizationContext, so it runs on a
/// later frame). Required for anything that touches Unity objects from the GUI.
pub fn run_on_main_thread(callback: unsafe extern "C" fn()) {
    let api = api();
    unsafe {
        let thread = (api.il2cpp_get_main_thread)();
        (api.il2cpp_schedule_on_thread)(thread, callback);
    }
}
