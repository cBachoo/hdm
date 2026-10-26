//! Per-frame work, driven by a hook on `Gallop.UIManager.LateUpdate` (main thread, unique address,
//! not hooked by Hachimi):
//! - picks up standalone UI textures (`utx_*`, `tx_uTex_*`, `vertical_bg_*`, `uieff_*`) and
//!   late-loaded atlas sheets without an AtlasReference (`*_tex`) that bypass the asset hooks,
//! - darkens plain white overlays (`WhiteMask` fades, the loading screen backdrop),
//! - darkens cameras that clear to white, and outlines set without a setter.
//!
//! Everything here runs on a budget: full searches over all objects happen rarely, and in between
//! only cached objects are touched.

use std::collections::HashSet;
use std::io::Write;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::Mutex;

use crate::api::{self, log_error, log_info, Ptr};
use crate::config;
use crate::textures;
use crate::unity::{self, unity, Color};

static ORIG_LATE_UPDATE: AtomicUsize = AtomicUsize::new(0);
static ORIG_DO_MESH_GENERATION: AtomicUsize = AtomicUsize::new(0);
static FRAME: AtomicU32 = AtomicU32::new(0);

/// New textures (reloaded atlases show their original look until picked up, so keep it short).
const TEXTURE_SCAN_INTERVAL: u32 = 15;
/// Re-darken the cached overlays.
const OVERLAY_REFRESH_INTERVAL: u32 = 20;
/// Full searches (all Images, cameras, outlines). Expensive, so rare.
const FULL_SCAN_INTERVAL: u32 = 120;

pub const OVERLAY_DARK: (f32, f32, f32) = (0.06, 0.065, 0.08);

/// RawImages that are solid white backdrops (name match). `LoadingBg`: the loading screen's
/// background (SystemCanvas/LoadingCanvas/BackObj/Loading/Bg/LoadingBg).
const BACKDROP_RAW_IMAGES: &[&str] = &["LoadingBg"];

/// Texture pointers seen by the last texture scan.
static SEEN_TEXTURES: Mutex<Option<HashSet<usize>>> = Mutex::new(None);
/// Overlay graphics found by the last full scan (re-darkened between scans and on mesh rebuild).
static OVERLAYS: Mutex<Option<HashSet<usize>>> = Mutex::new(None);
/// Overlays already written to `overlays.txt`.
static LOGGED_OVERLAYS: Mutex<Option<HashSet<usize>>> = Mutex::new(None);

type LateUpdateFn = extern "C-unwind" fn(this: Ptr, method: Ptr);
extern "C-unwind" fn late_update_hook(this: Ptr, method: Ptr) {
    let orig: LateUpdateFn = unsafe { std::mem::transmute(ORIG_LATE_UPDATE.load(Ordering::Relaxed)) };
    orig(this, method);

    let frame = FRAME.fetch_add(1, Ordering::Relaxed);
    if !config::enabled() {
        return;
    }
    let _ = std::panic::catch_unwind(|| {
        if frame % TEXTURE_SCAN_INTERVAL == 0 {
            scan_textures();
        }
        if frame % FULL_SCAN_INTERVAL == 0 {
            find_overlays();
            scan_cameras();
            scan_outlines();
            apply_debug_hide();
        } else if frame % OVERLAY_REFRESH_INTERVAL == 0 {
            refresh_overlays();
        }
    });
}

/// `Graphic.DoMeshGeneration`: runs whenever a UI element rebuilds its geometry, including after
/// an Animator writes `m_Color` directly (bypassing `set_color`). Overlays get their colours
/// rewritten before the mesh is built.
type DoMeshGenerationFn = extern "C-unwind" fn(this: Ptr, method: Ptr);
extern "C-unwind" fn do_mesh_generation_hook(this: Ptr, method: Ptr) {
    let orig: DoMeshGenerationFn = unsafe { std::mem::transmute(ORIG_DO_MESH_GENERATION.load(Ordering::Relaxed)) };
    if config::enabled() {
        if is_cached_overlay(this) {
            darken_overlay(this);
        } else if overlay_replacement(unity::graphic_color(this)).is_some()
            && unity::is_instance_of(this, unity().image_class)
            && is_overlay(this)
        {
            // An overlay created since the last full scan.
            cache_overlay(this);
            darken_overlay(this);
        }
    }
    orig(this, method);
}

pub fn install_hooks() {
    let u = unity();
    let hooks: [(&str, usize, usize, &AtomicUsize); 2] = [
        ("UIManager.LateUpdate", u.ui_manager_late_update, late_update_hook as *const () as usize, &ORIG_LATE_UPDATE),
        ("Graphic.DoMeshGeneration", u.graphic_do_mesh_generation, do_mesh_generation_hook as *const () as usize, &ORIG_DO_MESH_GENERATION),
    ];
    for (name, target, hook, orig) in hooks {
        match api::hook(target, hook) {
            Some(t) => { orig.store(t, Ordering::Relaxed); log_info!("Hooked {name}"); }
            None => log_error!("Failed to hook {name}"),
        }
    }
}

fn is_ui_texture_name(name: &str) -> bool {
    // uieff_*: UI effect textures (handled only if overrides.json has a rule for them).
    name.starts_with("utx_") || name.starts_with("tx_uTex_") || name.starts_with("vertical_bg_") || name.starts_with("uieff_")
}

/// Handles textures that appeared since the last scan. Names are only read for new objects.
fn scan_textures() {
    let textures = unity::all_texture2ds();
    let mut new = Vec::new();
    {
        let mut seen = SEEN_TEXTURES.lock().unwrap();
        let previous = seen.take().unwrap_or_default();
        let current: HashSet<usize> = textures.iter().map(|&t| t as usize).collect();
        for &t in &textures {
            if !previous.contains(&(t as usize)) {
                new.push(t);
            }
        }
        *seen = Some(current);
    }

    let mut new_atlases = Vec::new();
    for tex in new {
        let name = unity::object_name(tex);
        if name.ends_with(textures::PATCHED_MARK) {
            continue;
        }
        if textures::reapply_if_owned(tex, &name) {
            continue; // a reloaded copy of something handled before
        }
        if textures::is_unowned_atlas_texture(&name) {
            new_atlases.push((tex, name)); // atlas sheet without an AtlasReference
        } else if is_ui_texture_name(&name) {
            textures::process_loose_texture(tex, &name);
        }
    }
    if !new_atlases.is_empty() {
        let (log_assets, dump) = (config::log_assets(), config::dump_textures());
        textures::process_loose_atlases(new_atlases, log_assets, dump);
    }
}

/// Darkens an overlay's own colour, the colour it receives from a parent ColorSender, and its
/// CanvasRenderer tint, whatever their current alpha (fades often start at 0). Writes fields /
/// renderer state directly, so it's safe during mesh rebuilds. Returns true if anything changed.
pub fn darken_overlay(g: Ptr) -> bool {
    let (r, gr, b) = OVERLAY_DARK;
    let mut changed = false;
    let c = unity::graphic_color(g);
    if c.r >= 0.9 && c.g >= 0.9 && c.b >= 0.9 {
        unity::write_graphic_color_field(g, Color { r, g: gr, b, a: c.a });
        changed = true;
    }
    if let Some(rc) = unity::image_receive_color(g) {
        if rc.r >= 0.9 && rc.g >= 0.9 && rc.b >= 0.9 {
            unity::set_image_receive_color(g, Color { r, g: gr, b, a: rc.a });
            changed = true;
        }
    }
    if let Some(cr) = unity::canvas_renderer_color(g) {
        if cr.r > 0.5 || cr.g > 0.5 || cr.b > 0.5 {
            unity::set_canvas_renderer_color(g, Color { r, g: gr, b, a: cr.a });
            changed = true;
        }
    }
    changed
}

/// Dark colour for a white (or near-white) colour, keeping alpha. `None` if not white.
pub fn overlay_replacement(c: Color) -> Option<Color> {
    if c.r >= 0.9 && c.g >= 0.9 && c.b >= 0.9 && c.a > 0.0 {
        let (r, g, b) = OVERLAY_DARK;
        Some(Color { r, g, b, a: c.a })
    } else {
        None
    }
}

/// White fade/flash overlays are named like `WhiteMask`, `WhiteFadeLeftUIBg`, `Img_white`, `FadeBg`.
/// Matching by name matters: many other Images are white with no sprite only until `ImageCommon`
/// assigns their sprite at runtime, and darkening those tints the sprite black.
pub fn is_overlay_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    // "Loading": the loading screen's backdrop (SystemCanvas/LoadingCanvas/BackObj/Loading).
    lower.contains("white") || lower.contains("fade") || name == "Loading"
}

/// True for a sprite-less Image whose name marks it as a white overlay.
pub fn is_overlay(img: Ptr) -> bool {
    unity::image_sprite(img).is_null() && is_overlay_name(&unity::object_name(img))
}

fn is_cached_overlay(g: Ptr) -> bool {
    OVERLAYS.lock().unwrap().as_ref().is_some_and(|s| s.contains(&(g as usize)))
}

fn cache_overlay(g: Ptr) {
    OVERLAYS.lock().unwrap().get_or_insert_with(HashSet::new).insert(g as usize);
}

/// Full search: overlay Images (by name) and backdrop RawImages. Rebuilds the cache.
fn find_overlays() {
    let u = unity();
    let mut found = HashSet::new();
    for img in unity::find_all_of_class(u.image_class) {
        if is_overlay(img) {
            found.insert(img as usize);
        }
    }
    for raw in unity::find_all_of_class(u.raw_image_class) {
        if BACKDROP_RAW_IMAGES.contains(&unity::object_name(raw).as_str()) {
            found.insert(raw as usize);
        }
    }
    *OVERLAYS.lock().unwrap() = Some(found);
    refresh_overlays();
}

/// Re-darkens cached overlays (cheap: no searching).
fn refresh_overlays() {
    let cached: Vec<usize> = OVERLAYS.lock().unwrap().as_ref().map(|s| s.iter().copied().collect()).unwrap_or_default();
    for g in cached {
        let g = g as Ptr;
        if unity::is_alive(g) && darken_overlay(g) {
            log_overlay(g);
        }
    }
}

/// Cameras that clear to solid white (e.g. `UIManager/BGCamera`). Normally everything is drawn
/// over the clear colour, but during loading screens it can show.
fn scan_cameras() {
    const SOLID_COLOR: i32 = 2;
    for cam in unity::find_all_of_class(unity().camera_class) {
        let (bg, flags) = unity::camera_background(cam);
        if flags != SOLID_COLOR {
            continue;
        }
        if let Some(dark) = overlay_replacement(bg) {
            unity::set_camera_background(cam, dark);
        }
    }
}

/// Outlines whose colour was set without going through a setter (e.g. serialized in the prefab):
/// fixed here, outside the UI rebuild loop, through the hooked OutlineCommon.set_color.
fn scan_outlines() {
    if !config::get().remap_text {
        return;
    }
    for outline in unity::find_all_of_class(unity().outline_common_class) {
        if unity::is_active_and_enabled(outline) {
            crate::text::fix_outline(outline);
        }
    }
}

/// Debug: deactivate GameObjects matching `debug_hide` in config.json (no cost when empty).
fn apply_debug_hide() {
    let patterns = config::debug_hide();
    if patterns.is_empty() {
        return;
    }
    for t in unity::find_all_of_class(unity().transform_class) {
        let path = unity::hierarchy_path(t, 10);
        if patterns.iter().any(|p| path.contains(p.as_str())) {
            log_info!("debug_hide: deactivating {path}");
            unity::deactivate(t);
        }
    }
}

/// Records each darkened overlay once in `overlays.txt`.
pub fn log_overlay(g: Ptr) {
    if !LOGGED_OVERLAYS.lock().unwrap().get_or_insert_with(HashSet::new).insert(g as usize) {
        return;
    }
    let path = unity::hierarchy_path(g, 8);
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(config::dir().join("overlays.txt")) {
        let _ = writeln!(f, "{path}");
    }
}
