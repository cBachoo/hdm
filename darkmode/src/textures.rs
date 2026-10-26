//! Intercepts textures and sprite atlases as the game loads them and darkens them: either from a
//! full replacement PNG (`darkmode/{atlas,textures}/<name>.png`, `darkmode/manual/textures/`), or
//! at runtime from the rules in `overrides.json` (see `runtime`).
//!
//! Hachimi already hooks the native `AssetBundle::LoadAsset_Internal` icall and
//! `AssetBundleRequest.GetResult`, and MinHook refuses to hook the same address twice. The managed
//! wrappers around those are either inlined or shared with other methods, so we hook the game's
//! own loader cache and the atlas asset instead.

use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use crate::api::{self, log_error, log_info, log_warn, Ptr};
use crate::config;
use crate::rules;
use crate::runtime::{self, SpriteRect};
use crate::unity::{self, unity};

/// Appended to a texture's name once we've darkened it, so repeated loads of the same object
/// (and atlases seen by both hooks) are skipped.
pub const PATCHED_MARK: &str = "#darkmode";

static ORIG_SET_OBJECT_CACHE: AtomicUsize = AtomicUsize::new(0);
static ORIG_ATLAS_ON_ENABLE: AtomicUsize = AtomicUsize::new(0);

static SEEN: Mutex<Option<HashSet<String>>> = Mutex::new(None);
/// Texture name -> what it belongs to, for re-applying when the game reloads it.
static TEX_OWNERS: Mutex<Option<HashMap<String, Owner>>> = Mutex::new(None);
/// "kind/name" of textures the rules leave untouched, so they aren't read back over and over.
/// Cleared by `reapply_all`.
static UNCHANGED: Mutex<Option<HashSet<String>>> = Mutex::new(None);

#[derive(Clone, Copy)]
enum Kind {
    Texture,
    Atlas,
}

impl Kind {
    fn dir_name(self) -> &'static str {
        match self {
            Kind::Texture => "textures",
            Kind::Atlas => "atlas",
        }
    }
}

#[derive(Clone)]
struct Owner {
    kind: Kind,
    name: String,
    sprites: Option<Vec<SpriteRect>>,
}

// Both targets have a unique address in GameAssembly (verified with the method dump), so they
// aren't shared with unrelated methods by identical-code folding, and Hachimi doesn't hook them.

/// `Cyan.Loader.AssetHandle::SetObjectCache(string name, Object obj, bool)`: the game's loader
/// caches every asset it pulls out of a bundle, sync or async.
type SetObjectCacheFn = extern "C-unwind" fn(this: Ptr, name: Ptr, obj: Ptr, flag: bool, method: Ptr);
extern "C-unwind" fn set_object_cache_hook(this: Ptr, name: Ptr, obj: Ptr, flag: bool, method: Ptr) {
    let orig: SetObjectCacheFn = unsafe { std::mem::transmute(ORIG_SET_OBJECT_CACHE.load(Ordering::Relaxed)) };
    orig(this, name, obj, flag, method);
    on_asset_loaded(obj);
}

/// `Cute.UI.AtlasReference::OnEnable()`: Unity calls this on every atlas asset once it's loaded.
type OnEnableFn = extern "C-unwind" fn(this: Ptr, method: Ptr);
extern "C-unwind" fn atlas_on_enable_hook(this: Ptr, method: Ptr) {
    let orig: OnEnableFn = unsafe { std::mem::transmute(ORIG_ATLAS_ON_ENABLE.load(Ordering::Relaxed)) };
    orig(this, method);
    on_asset_loaded(this);
}

pub fn install_hooks() {
    let u = unity();
    let hooks: [(&str, usize, usize, &AtomicUsize); 2] = [
        ("AssetHandle.SetObjectCache", u.asset_handle_set_object_cache, set_object_cache_hook as *const () as usize, &ORIG_SET_OBJECT_CACHE),
        ("AtlasReference.OnEnable", u.atlas_reference_on_enable, atlas_on_enable_hook as *const () as usize, &ORIG_ATLAS_ON_ENABLE),
    ];
    for (name, target, hook, orig) in hooks {
        match api::hook(target, hook) {
            Some(t) => { orig.store(t, Ordering::Relaxed); log_info!("Hooked {name}"); }
            None => log_error!("Failed to hook {name}"),
        }
    }
}

fn on_asset_loaded(asset: Ptr) {
    if asset.is_null() {
        return;
    }
    // Never let a panic unwind into game code.
    if std::panic::catch_unwind(|| handle_asset(asset)).is_err() {
        log_error!("Panic while processing asset");
    }
}

fn handle_asset(asset: Ptr) {
    let (enabled, log_assets, dump) = (config::enabled(), config::log_assets(), config::dump_textures());
    if !enabled {
        return;
    }

    let u = unity();
    let class = unity::class_of(asset);

    if class == u.texture2d_class {
        process_texture(asset, Kind::Texture, &unity::object_name(asset), log_assets, dump, None);
    } else if class == u.atlas_reference_class && class != 0 {
        let sprites = unity::atlas_sprites(asset);
        let Some(&first) = sprites.first() else { return };
        // Every sprite in an atlas shares one texture, so patching it once covers them all.
        let tex = unity::sprite_texture(first);
        if tex.is_null() {
            return;
        }
        let rects = sprite_rects(&sprites);
        process_texture(tex, Kind::Atlas, &unity::object_name(asset), log_assets, dump, Some(rects));
    }
}

/// Rects of the given sprites (packed sprites, which have no simple rect, are skipped).
fn sprite_rects(sprites: &[Ptr]) -> Vec<SpriteRect> {
    sprites
        .iter()
        .filter_map(|&s| {
            let r = unity::sprite_texture_rect(s)?;
            Some(SpriteRect { name: unity::object_name(s), x: r.x, y: r.y, w: r.width, h: r.height })
        })
        .collect()
}

/// `name@x,y,w,h;...` per sprite, for `loaded_assets.txt`. Unity texture space (origin bottom-left).
fn sprite_list(rects: &[SpriteRect]) -> String {
    rects.iter().map(|r| format!("{}@{},{},{},{}", r.name, r.x, r.y, r.w, r.h)).collect::<Vec<_>>().join(";")
}

fn process_texture(tex: Ptr, kind: Kind, name: &str, log_assets: bool, dump: bool, sprites: Option<Vec<SpriteRect>>) {
    if name.is_empty() {
        return;
    }
    let tex_name = unity::object_name(tex);
    if tex_name.ends_with(PATCHED_MARK) {
        return;
    }

    let key = format!("{}/{}", kind.dir_name(), name);
    let first_time = SEEN.lock().unwrap().get_or_insert_with(HashSet::new).insert(key);

    if first_time && log_assets {
        let (w, h) = unity::texture_size(tex);
        let list = sprites.as_deref().map(sprite_list).unwrap_or_default();
        log_asset(&format!("{}\t{}\t{}\t{}x{}\t{}", kind.dir_name(), name, tex_name, w, h, list));
    }

    if first_time && dump {
        dump_texture(tex, &file_path(&config::dir().join("dump"), kind.dir_name(), name));
    }

    let owner = Owner { kind, name: name.to_string(), sprites };
    TEX_OWNERS.lock().unwrap().get_or_insert_with(HashMap::new).insert(tex_name.clone(), owner.clone());
    apply_texture(tex, &owner, &tex_name);
}

/// Darkens `tex` for `owner`: a full replacement PNG if there is one, otherwise the runtime rules.
fn apply_texture(tex: Ptr, owner: &Owner, tex_name: &str) -> bool {
    let key = format!("{}/{}", owner.kind.dir_name(), owner.name);
    if UNCHANGED.lock().unwrap().get_or_insert_with(HashSet::new).contains(&key) {
        return false;
    }

    let full = [
        file_path(config::dir(), owner.kind.dir_name(), &owner.name),
        file_path(&config::dir().join("manual"), "textures", &owner.name),
    ];
    let applied = if let Some(path) = full.iter().find(|p| p.exists()) {
        match std::fs::read(path) {
            Ok(bytes) if unity::load_image(tex, &bytes, true) => {
                log_info!("Replaced {key} from {}", path.display());
                true
            }
            _ => {
                log_warn!("Failed to load replacement image: {}", path.display());
                false
            }
        }
    } else {
        runtime::apply(tex, owner.kind.dir_name(), &owner.name, owner.sprites.as_deref())
    };

    if applied {
        let base = tex_name.trim_end_matches(PATCHED_MARK);
        unity::set_object_name(tex, &format!("{base}{PATCHED_MARK}"));
    } else {
        UNCHANGED.lock().unwrap().get_or_insert_with(HashSet::new).insert(key);
    }
    applied
}

/// Reloads overrides.json, forgets which textures were left unchanged, and re-applies to every
/// loaded texture we know (restored to original first isn't possible, so already-darkened ones are
/// skipped; restart the game to see rule changes on those). Main thread only.
pub unsafe extern "C" fn reapply_all() {
    let _ = std::panic::catch_unwind(|| {
        rules::load();
        config::reload();
        *UNCHANGED.lock().unwrap() = None;
        let owners = TEX_OWNERS.lock().unwrap().clone().unwrap_or_default();
        let mut applied = 0;
        for tex in unity::all_texture2ds() {
            let tex_name = unity::object_name(tex);
            if tex_name.ends_with(PATCHED_MARK) {
                continue;
            }
            if let Some(owner) = owners.get(&tex_name) {
                if apply_texture(tex, owner, &tex_name) {
                    applied += 1;
                }
            }
        }
        log_info!("reapply: darkened {applied} textures");
    });
}

fn file_path(root: &PathBuf, dir: &str, name: &str) -> PathBuf {
    let safe: String = name
        .chars()
        .map(|c| if matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') { '_' } else { c })
        .collect();
    root.join(dir).join(format!("{safe}.png"))
}

fn dump_texture(tex: Ptr, path: &PathBuf) {
    if path.exists() {
        return;
    }
    let Some(copy) = unity::readable_copy(tex) else { return };
    let png = unity::encode_to_png(copy);
    unity::destroy(copy);
    if png.is_empty() {
        return;
    }
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(e) = std::fs::write(path, png) {
        log_warn!("Failed to dump {}: {}", path.display(), e);
    }
}

fn log_asset(line: &str) {
    let path = config::dir().join("loaded_assets.txt");
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{line}");
    }
}

/// Debug: writes which sprite/texture every UI Image and RawImage uses to `ui_usage.txt`, and the
/// names of all loaded textures to `textures_loaded.txt`. Main thread only.
pub unsafe extern "C" fn dump_ui_usage() {
    dump_ui_usage_to("ui_usage.txt".into());
}

/// `dump_ui_usage` into a given file in the data dir. Main thread only.
pub fn dump_ui_usage_to(file: String) {
    let _ = std::panic::catch_unwind(|| {
        let u = unity();
        let mut usage: HashMap<String, u32> = HashMap::new();

        for img in unity::find_all_of_class(u.image_class) {
            // Prefab templates can reference assets that were unloaded; touching those throws.
            let sprite = unity::image_sprite(img);
            if !sprite.is_null() && !unity::is_alive(sprite) {
                continue;
            }
            let c = unity::graphic_color(img);
            let active = if unity::is_active_and_enabled(img) { "ACTIVE " } else { "" };
            let color = format!("{active}color={:.2},{:.2},{:.2},{:.2}", c.r, c.g, c.b, c.a);
            let path = unity::hierarchy_path(img, 6);
            let line = if sprite.is_null() {
                format!("{}\tsprite=null\t{color}\t{path}", unity::class_name(unity::class_of(img)))
            } else {
                let tex = unity::sprite_texture(sprite);
                let tex = if unity::is_alive(tex) { tex } else { std::ptr::null_mut() };
                let (w, h) = if tex.is_null() { (0, 0) } else { unity::texture_size(tex) };
                let rect = match unity::sprite_texture_rect(sprite) {
                    Some(r) => format!("{},{},{},{}", r.x, r.y, r.width, r.height),
                    None => "packed".into(),
                };
                format!(
                    "{}\tsprite={}\ttexture={} {}x{}\trect={rect}\t{color}\t{path}",
                    unity::class_name(unity::class_of(img)),
                    unity::object_name(sprite),
                    if tex.is_null() { "null".into() } else { unity::object_name(tex) },
                    w, h,
                )
            };
            *usage.entry(line).or_default() += 1;
        }
        for raw in unity::find_all_of_class(u.raw_image_class) {
            let tex = unity::raw_image_texture(raw);
            if !unity::is_alive(tex) {
                continue;
            }
            let (w, h) = unity::texture_size(tex);
            let c = unity::graphic_color(raw);
            let active = if unity::is_active_and_enabled(raw) { "ACTIVE " } else { "" };
            let line = format!(
                "{}\traw texture={} {}x{}\t{active}color={:.2},{:.2},{:.2},{:.2}\t{}",
                unity::class_name(unity::class_of(raw)), unity::object_name(tex), w, h, c.r, c.g, c.b, c.a,
                unity::hierarchy_path(raw, 6),
            );
            *usage.entry(line).or_default() += 1;
        }

        // Non-UI renderers (particles, meshes) in loading screens.
        for r in unity::find_all_of_class(u.renderer_class) {
            let path = unity::hierarchy_path(r, 8);
            if !path.to_ascii_lowercase().contains("loading") {
                continue;
            }
            let (active, mat, tex, c) = unity::renderer_info(r);
            let line = format!(
                "{}\t{}material={mat} texture={tex} color={:.2},{:.2},{:.2},{:.2}\t{path}",
                unity::class_name(unity::class_of(r)), if active { "ACTIVE " } else { "" }, c.r, c.g, c.b, c.a,
            );
            *usage.entry(line).or_default() += 1;
        }

        for cam in unity::find_all_of_class(u.camera_class) {
            let (bg, flags) = unity::camera_background(cam);
            let active = if unity::is_active_and_enabled(cam) { "ACTIVE " } else { "" };
            let line = format!(
                "Camera\t{active}clear={flags} background={:.2},{:.2},{:.2},{:.2}\t{}",
                bg.r, bg.g, bg.b, bg.a, unity::hierarchy_path(cam, 6),
            );
            *usage.entry(line).or_default() += 1;
        }

        let mut lines: Vec<_> = usage.into_iter().map(|(l, n)| format!("{n}\t{l}")).collect();
        lines.sort();
        let _ = std::fs::write(config::dir().join(&file), lines.join("\n"));

        let mut texs: Vec<String> = unity::all_texture2ds()
            .into_iter()
            .map(|t| {
                let (w, h) = unity::texture_size(t);
                format!("{}\t{}x{}", unity::object_name(t), w, h)
            })
            .collect();
        texs.sort();
        let _ = std::fs::write(config::dir().join("textures_loaded.txt"), texs.join("\n"));

        log_info!("Wrote {file} ({} entries) and textures_loaded.txt ({} textures)", lines.len(), texs.len());
    });
}

/// Processes atlases that were loaded before the plugin initialized (e.g. `PreIn`, `SteamPreIn`,
/// which the game loads during boot). Main thread only.
pub unsafe extern "C" fn scan_loaded_atlases() {
    let _ = std::panic::catch_unwind(|| {
        let atlases = unity::find_all_of_class(unity().atlas_reference_class);
        log_info!("Scanning {} already-loaded atlases", atlases.len());
        for atlas in atlases {
            on_asset_loaded(atlas);
        }

        // Some atlas textures (e.g. SteamPreIn_tex) are referenced by sprites directly with no
        // AtlasReference loaded; handle those by texture name.
        let (enabled, log_assets, dump) = (config::enabled(), config::log_assets(), config::dump_textures());
        if !enabled {
            return;
        }
        let loose: Vec<(Ptr, String)> = unity::all_texture2ds()
            .into_iter()
            .map(|t| (t, unity::object_name(t)))
            .filter(|(_, n)| is_unowned_atlas_texture(n))
            .collect();
        process_loose_atlases(loose, log_assets, dump);
    });
}

/// An atlas texture (`*_tex`) not yet handled through an AtlasReference or an earlier scan.
pub fn is_unowned_atlas_texture(name: &str) -> bool {
    name.ends_with("_tex") && !TEX_OWNERS.lock().unwrap().as_ref().is_some_and(|m| m.contains_key(name))
}

/// Logs (with sprite rects), dumps and darkens atlas textures that have no AtlasReference.
/// Main thread only.
pub fn process_loose_atlases(loose: Vec<(Ptr, String)>, log_assets: bool, dump: bool) {
    if loose.is_empty() {
        return;
    }
    // Collect the sprites that live on each loose texture.
    let mut sprites_by_tex: HashMap<usize, Vec<Ptr>> = HashMap::new();
    for sprite in unity::find_all_of_class(unity().sprite_class) {
        let tex = unity::sprite_texture(sprite) as usize;
        if loose.iter().any(|(t, _)| *t as usize == tex) {
            sprites_by_tex.entry(tex).or_default().push(sprite);
        }
    }
    for (tex, name) in loose {
        let sprites = sprites_by_tex.remove(&(tex as usize)).unwrap_or_default();
        process_texture(tex, Kind::Texture, &name, log_assets, dump, Some(sprite_rects(&sprites)));
    }
}

/// Logs/dumps/darkens a standalone UI texture found by the per-frame scan. Main thread only.
pub fn process_loose_texture(tex: Ptr, name: &str) {
    let (enabled, log_assets, dump) = (config::enabled(), config::log_assets(), config::dump_textures());
    if enabled {
        process_texture(tex, Kind::Texture, name, log_assets, dump, None);
    }
}

/// If `tex` is a fresh (undarkened) copy of a texture we've seen before, e.g. an atlas the game
/// unloaded and loaded again, darken it again (cheap: the result is cached). Main thread only.
pub fn reapply_if_owned(tex: Ptr, name: &str) -> bool {
    let owner = TEX_OWNERS.lock().unwrap().as_ref().and_then(|m| m.get(name).cloned());
    let Some(owner) = owner else { return false };
    apply_texture(tex, &owner, name);
    true
}
