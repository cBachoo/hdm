//! Development aid: writes every class/method of selected assemblies to `methods_<assembly>.txt`,
//! including each method's address relative to GameAssembly.dll. Methods that share an address
//! were folded together by the compiler (ICF) and are unsafe to hook.

use std::collections::HashMap;
use std::ffi::{c_char, c_void, CStr};
use std::io::Write;

use crate::api::{api, log_info, log_warn, Ptr};
use crate::config;

extern "system" {
    fn GetModuleHandleA(name: *const c_char) -> *mut c_void;
}

struct Il2Cpp {
    image_get_class_count: unsafe extern "C" fn(Ptr) -> usize,
    image_get_class: unsafe extern "C" fn(Ptr, usize) -> Ptr,
    class_get_methods: unsafe extern "C" fn(Ptr, *mut Ptr) -> Ptr,
    class_get_name: unsafe extern "C" fn(Ptr) -> *const c_char,
    class_get_namespace: unsafe extern "C" fn(Ptr) -> *const c_char,
    class_get_declaring_type: unsafe extern "C" fn(Ptr) -> Ptr,
    method_get_name: unsafe extern "C" fn(Ptr) -> *const c_char,
    method_get_param_count: unsafe extern "C" fn(Ptr) -> u32,
    method_get_param: unsafe extern "C" fn(Ptr, u32) -> Ptr,
    method_get_return_type: unsafe extern "C" fn(Ptr) -> Ptr,
    method_get_flags: unsafe extern "C" fn(Ptr, *mut u32) -> u32,
    type_get_name: unsafe extern "C" fn(Ptr) -> *mut c_char,
}

fn resolve() -> Option<Il2Cpp> {
    macro_rules! sym {
        ($name:literal) => {{
            let p = unsafe { (api().il2cpp_resolve_symbol)(concat!($name, "\0").as_ptr() as *const c_char) };
            if p.is_null() {
                log_warn!("il2cpp symbol not found: {}", $name);
                return None;
            }
            unsafe { std::mem::transmute(p) }
        }};
    }
    Some(Il2Cpp {
        image_get_class_count: sym!("il2cpp_image_get_class_count"),
        image_get_class: sym!("il2cpp_image_get_class"),
        class_get_methods: sym!("il2cpp_class_get_methods"),
        class_get_name: sym!("il2cpp_class_get_name"),
        class_get_namespace: sym!("il2cpp_class_get_namespace"),
        class_get_declaring_type: sym!("il2cpp_class_get_declaring_type"),
        method_get_name: sym!("il2cpp_method_get_name"),
        method_get_param_count: sym!("il2cpp_method_get_param_count"),
        method_get_param: sym!("il2cpp_method_get_param"),
        method_get_return_type: sym!("il2cpp_method_get_return_type"),
        method_get_flags: sym!("il2cpp_method_get_flags"),
        type_get_name: sym!("il2cpp_type_get_name"),
    })
}

fn s(p: *const c_char) -> String {
    if p.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(p).to_string_lossy().into_owned() }
    }
}

fn type_name(il: &Il2Cpp, t: Ptr) -> String {
    if t.is_null() { "?".into() } else { s(unsafe { (il.type_get_name)(t) }) }
}

fn class_full_name(il: &Il2Cpp, class: Ptr) -> String {
    unsafe {
        let name = s((il.class_get_name)(class));
        let outer = (il.class_get_declaring_type)(class);
        if !outer.is_null() {
            return format!("{}/{}", class_full_name(il, outer), name);
        }
        let ns = s((il.class_get_namespace)(class));
        if ns.is_empty() { name } else { format!("{ns}.{name}") }
    }
}

pub fn dump_assemblies(assemblies: &[&str]) {
    let Some(il) = resolve() else { return };
    let base = unsafe { GetModuleHandleA(c"GameAssembly.dll".as_ptr()) } as usize;

    let mut addr_count: HashMap<usize, u32> = HashMap::new();
    let mut per_asm: Vec<(&str, Vec<(String, usize)>)> = Vec::new();

    for &asm in assemblies {
        let image = unsafe { (api().il2cpp_get_assembly_image)(crate::api::cstring(asm).as_ptr()) };
        if image.is_null() {
            log_warn!("dump: assembly not found: {asm}");
            continue;
        }

        let mut rows: Vec<(String, usize)> = Vec::new();
        let count = unsafe { (il.image_get_class_count)(image) };
        for i in 0..count {
            let class = unsafe { (il.image_get_class)(image, i) };
            if class.is_null() {
                continue;
            }
            let cname = class_full_name(&il, class);
            let mut iter: Ptr = std::ptr::null_mut();
            loop {
                let m = unsafe { (il.class_get_methods)(class, &mut iter) };
                if m.is_null() {
                    break;
                }
                unsafe {
                    let addr = *(m as *const usize); // MethodInfo::methodPointer
                    let mut iflags = 0u32;
                    let flags = (il.method_get_flags)(m, &mut iflags);
                    let is_static = flags & 0x10 != 0;
                    let n = (il.method_get_param_count)(m);
                    let params: Vec<String> = (0..n).map(|p| type_name(&il, (il.method_get_param)(m, p))).collect();
                    let line = format!(
                        "{}{} {}::{}({})",
                        if is_static { "static " } else { "" },
                        type_name(&il, (il.method_get_return_type)(m)),
                        cname,
                        s((il.method_get_name)(m)),
                        params.join(", "),
                    );
                    if addr != 0 {
                        *addr_count.entry(addr).or_default() += 1;
                    }
                    rows.push((line, addr));
                }
            }
        }

        per_asm.push((asm, rows));
    }

    for (asm, rows) in per_asm {
        let path = config::dir().join(format!("methods_{}.txt", asm.trim_end_matches(".dll")));
        let Ok(mut f) = std::fs::File::create(&path) else { continue };
        let mut w = std::io::BufWriter::new(&mut f);
        for (line, addr) in &rows {
            let rva = if *addr >= base && base != 0 { addr - base } else { *addr };
            let shared = addr_count.get(addr).copied().unwrap_or(0);
            let tag = if *addr == 0 { " [no code]".to_string() } else if shared > 1 { format!(" [SHARED x{shared}]") } else { String::new() };
            let _ = writeln!(w, "{rva:#010x}{tag}\t{line}");
        }
        log_info!("dump: wrote {} methods from {asm} to {}", rows.len(), path.display());
    }
}
