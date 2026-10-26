mod api;
mod config;
#[cfg(feature = "dev")]
mod dump;
mod process;
mod rules;
mod runtime;
mod text;
mod textures;
mod tick;
mod unity;

use std::ffi::c_void;
use std::ptr::null_mut;

use api::{api, log_error, log_info, GetApiFn, InitResult, Ptr};

#[no_mangle]
pub extern "C" fn hachimi_init_v3(get_api: GetApiFn, version: i32) -> InitResult {
    if version < 3 {
        return InitResult::Error;
    }
    if let Err(missing) = api::init(get_api) {
        // Can't log through Hachimi without the API; nothing else to do.
        let _ = missing;
        return InitResult::Error;
    }

    config::load();
    rules::load();

    #[cfg(feature = "dev")]
    if config::get().dump_methods {
        dump::dump_assemblies(&[
            "umamusume.dll",
            "_Cyan.dll",
            "Cute.UI.Assembly.dll",
            "Cute.Core.Assembly.dll",
            "UnityEngine.CoreModule.dll",
            "UnityEngine.UI.dll",
            "UnityEngine.AssetBundleModule.dll",
        ]);
        config::get().dump_methods = false;
        config::save();
    }

    if !unity::init() {
        log_error!("Some required Unity symbols are missing, plugin disabled");
        return InitResult::Error;
    }
    textures::install_hooks();
    api::run_on_main_thread(textures::scan_loaded_atlases);

    text::set_enabled(config::get().remap_text);
    text::install_hooks();
    api::run_on_main_thread(text::fix_all_texts);

    tick::install_hooks();

    unsafe { (api().gui_register_menu_section)(Some(menu_section), null_mut()) };

    log_info!("Dark mode loaded, data dir: {}", config::dir().display());
    InitResult::Ok
}

extern "C" fn menu_section(ui: Ptr, _userdata: *mut c_void) {
    let a = api();
    unsafe {
        (a.gui_ui_heading)(ui, c"Dark Mode".as_ptr());

        let mut changed = false;
        let text_changed;
        {
            let mut c = config::get();
            changed |= (a.gui_ui_checkbox)(ui, c"Enabled".as_ptr(), &mut c.enabled);
            text_changed = (a.gui_ui_checkbox)(ui, c"Light text".as_ptr(), &mut c.remap_text);
            changed |= text_changed;
            text::set_enabled(c.remap_text);
            if cfg!(feature = "dev") {
                changed |= (a.gui_ui_checkbox)(ui, c"Log loaded textures".as_ptr(), &mut c.log_assets);
                changed |= (a.gui_ui_checkbox)(ui, c"Dump original textures".as_ptr(), &mut c.dump_textures);
            }
        }
        if changed {
            config::save();
        }
        if text_changed {
            api::run_on_main_thread(text::fix_all_texts);
        }
        (a.gui_ui_label)(ui, c"Changes apply to textures loaded afterwards.".as_ptr());
        if (a.gui_ui_button)(ui, c"Reload overrides.json & re-apply".as_ptr()) {
            api::run_on_main_thread(textures::reapply_all);
        }

        if cfg!(feature = "dev") {
            (a.gui_ui_label)(ui, c"Dev tools".as_ptr());
            if (a.gui_ui_button)(ui, c"Rescan loaded atlases".as_ptr()) {
                api::run_on_main_thread(textures::scan_loaded_atlases);
            }
            if (a.gui_ui_button)(ui, c"Dump UI sprite usage".as_ptr()) {
                api::run_on_main_thread(textures::dump_ui_usage);
            }
        }
    }
}
