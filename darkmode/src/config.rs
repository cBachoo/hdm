use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, OnceLock};

use serde::{Deserialize, Serialize};

use crate::api::{self, log_warn};

#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
pub struct Config {
    /// Master switch. Turning it off only affects textures loaded afterwards.
    pub enabled: bool,
    /// Append every texture / atlas the game loads to `loaded_assets.txt` (once per session).
    #[cfg_attr(not(feature = "dev"), serde(skip_serializing))]
    pub log_assets: bool,
    /// Save the original of every texture / atlas the game loads into `dump/`.
    #[cfg_attr(not(feature = "dev"), serde(skip_serializing))]
    pub dump_textures: bool,
    /// Turn dark text colours light so labels stay readable on dark UI.
    pub remap_text: bool,
    /// Debug: GameObjects whose hierarchy path contains any of these are deactivated (re-read by
    /// "Reload overrides.json & re-apply"). Used to find what draws something on screen.
    #[cfg_attr(not(feature = "dev"), serde(skip_serializing))]
    pub debug_hide: Vec<String>,
    /// Development: write all game classes/methods to `methods_*.txt` on next start (resets itself).
    #[cfg_attr(not(feature = "dev"), serde(skip_serializing))]
    pub dump_methods: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self { enabled: true, log_assets: true, dump_textures: false, remap_text: true, debug_hide: Vec::new(), dump_methods: false }
    }
}

static DIR: OnceLock<PathBuf> = OnceLock::new();
/// Mirror of `Config::enabled` for hot paths (hooks run on every UI rebuild).
static ENABLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

pub fn enabled() -> bool {
    ENABLED.load(std::sync::atomic::Ordering::Relaxed)
}

fn sync_enabled(c: &Config) {
    ENABLED.store(c.enabled, std::sync::atomic::Ordering::Relaxed);
}
static CONFIG: OnceLock<Mutex<Config>> = OnceLock::new();

/// `<game>/hachimi/darkmode`
pub fn dir() -> &'static PathBuf {
    DIR.get_or_init(|| api::base_dir().join("darkmode"))
}

fn path() -> PathBuf {
    dir().join("config.json")
}

pub fn load() {
    let _ = std::fs::create_dir_all(dir());
    let config = match std::fs::read_to_string(path()) {
        Ok(s) => serde_json::from_str(&s).unwrap_or_else(|e| {
            log_warn!("Invalid config.json, using defaults: {}", e);
            Config::default()
        }),
        Err(_) => Config::default(),
    };
    sync_enabled(&config);
    let _ = CONFIG.set(Mutex::new(config));
    save();
}

/// Dev-only settings: always off in the user build, whatever config.json says.
pub fn log_assets() -> bool {
    cfg!(feature = "dev") && get().log_assets
}

pub fn dump_textures() -> bool {
    cfg!(feature = "dev") && get().dump_textures
}

pub fn debug_hide() -> Vec<String> {
    if cfg!(feature = "dev") { get().debug_hide.clone() } else { Vec::new() }
}

pub fn get() -> MutexGuard<'static, Config> {
    CONFIG.get().expect("darkmode: config not loaded").lock().unwrap()
}

pub fn save() {
    sync_enabled(&get());
    let json = serde_json::to_string_pretty(&*get()).unwrap();
    if let Err(e) = std::fs::write(path(), json) {
        log_warn!("Failed to save config: {}", e);
    }
}

/// Re-reads config.json from disk (keeps the current config if it's missing/invalid).
pub fn reload() {
    if let Ok(text) = std::fs::read_to_string(path()) {
        match serde_json::from_str::<Config>(&text) {
            Ok(c) => {
                sync_enabled(&c);
                *get() = c;
            }
            Err(e) => log_warn!("Invalid config.json, keeping current settings: {}", e),
        }
    }
}
