//! `overrides.json`: darkness levels, per-sprite and per-texture rules, and the fallback rules for
//! assets nobody has reviewed yet. Same format and matching as `tools/build_dark_atlases.py`:
//! keys are an exact name, `Atlas/name`, or a `*`/`?` glob (checked against `Atlas/name` when the
//! key contains a `/`); exact matches win, then the first matching glob in file order.

use std::collections::HashMap;
use std::sync::{OnceLock, RwLock};

use serde_json::{Map, Value};

use crate::api::{log_info, log_warn};
use crate::config;

#[derive(Clone, Copy, Debug)]
pub struct Level {
    pub base: [f32; 3],
    pub scale: f32,
}

impl Level {
    /// What pure white maps to.
    pub fn white(&self) -> [f32; 3] {
        [self.base[0] + 255.0 * self.scale, self.base[1] + 255.0 * self.scale, self.base[2] + 255.0 * self.scale]
    }

    fn from_value(v: &Value) -> Option<Level> {
        let a = v.as_array()?;
        Some(Level {
            base: [a.first()?.as_f64()? as f32, a.get(1)?.as_f64()? as f32, a.get(2)?.as_f64()? as f32],
            scale: a.get(3)?.as_f64()? as f32,
        })
    }
}

/// One rule object, e.g. `{ "mode": "card", "direction": "bottom" }`.
#[derive(Clone, Debug, Default)]
pub struct Rule(Map<String, Value>);

impl Rule {
    pub fn str(&self, key: &str) -> Option<&str> {
        self.0.get(key)?.as_str()
    }
    pub fn flag(&self, key: &str) -> bool {
        self.0.get(key).and_then(Value::as_bool).unwrap_or(false)
    }
    pub fn float(&self, key: &str) -> Option<f32> {
        self.0.get(key)?.as_f64().map(|f| f as f32)
    }
    pub fn int(&self, key: &str) -> Option<i64> {
        self.0.get(key)?.as_f64().map(|f| f as i64)
    }
    pub fn level_value(&self) -> Option<&Value> {
        self.0.get("level")
    }
    pub fn with_flag(mut self, key: &str) -> Self {
        self.0.insert(key.to_owned(), Value::Bool(true));
        self
    }
}

pub struct Rules {
    levels: HashMap<String, Level>,
    sprites: Vec<(String, Rule)>,
    textures: Vec<(String, Rule)>,
    include_prefixes: Vec<String>,
    exclude_contains: Vec<String>,
    /// Hash of the file contents, part of the processed-texture cache key.
    pub hash: u64,
}

impl Rules {
    fn defaults() -> Rules {
        let mut levels = HashMap::new();
        levels.insert("background".into(), Level { base: [12.0, 13.0, 17.0], scale: 0.08 });
        levels.insert("panel".into(), Level { base: [18.0, 19.0, 25.0], scale: 0.13 });
        levels.insert("surface".into(), Level { base: [24.0, 26.0, 34.0], scale: 0.24 });
        levels.insert("raised".into(), Level { base: [32.0, 35.0, 46.0], scale: 0.33 });
        Rules {
            levels,
            sprites: Vec::new(),
            textures: Vec::new(),
            include_prefixes: Vec::new(),
            exclude_contains: Vec::new(),
            hash: 0,
        }
    }

    pub(crate) fn parse(text: &str) -> Result<Rules, String> {
        let root: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
        let mut rules = Rules::defaults();
        rules.hash = fnv1a(text.as_bytes());
        if let Some(levels) = root.get("levels").and_then(Value::as_object) {
            for (name, v) in levels {
                match Level::from_value(v) {
                    Some(l) => { rules.levels.insert(name.clone(), l); }
                    None => log_warn!("overrides.json: bad level {name}"),
                }
            }
        }
        let entries = |key: &str| -> Vec<(String, Rule)> {
            root.get(key)
                .and_then(Value::as_object)
                .map(|m| {
                    m.iter()
                        .filter_map(|(k, v)| v.as_object().map(|o| (k.clone(), Rule(o.clone()))))
                        .collect()
                })
                .unwrap_or_default()
        };
        rules.sprites = entries("sprites");
        rules.textures = entries("textures");
        let strings = |key: &str| -> Vec<String> {
            root.get("fallback")
                .and_then(|f| f.get(key))
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(|s| s.as_str().map(str::to_owned)).collect())
                .unwrap_or_default()
        };
        rules.include_prefixes = strings("include_prefixes");
        rules.exclude_contains = strings("exclude_contains");
        Ok(rules)
    }

    fn find<'a>(entries: &'a [(String, Rule)], atlas: &str, name: &str) -> Option<&'a Rule> {
        let qualified = format!("{atlas}/{name}");
        for exact in [qualified.as_str(), name] {
            if let Some((_, r)) = entries.iter().find(|(k, _)| k == exact) {
                return Some(r);
            }
        }
        entries
            .iter()
            .filter(|(k, _)| k.contains('*') || k.contains('?'))
            .find(|(k, _)| glob_match(k, if k.contains('/') { &qualified } else { name }))
            .map(|(_, r)| r)
    }

    pub fn sprite(&self, atlas: &str, name: &str) -> Option<&Rule> {
        Self::find(&self.sprites, atlas, name)
    }

    pub fn texture(&self, name: &str) -> Option<&Rule> {
        Self::find(&self.textures, "textures", name)
    }

    /// Level named or given inline (`[r, g, b, scale]`) by a rule, defaulting to "surface".
    pub fn level(&self, rule: Option<&Rule>) -> Level {
        let surface = self.levels["surface"];
        match rule.and_then(Rule::level_value) {
            Some(Value::String(name)) => *self.levels.get(name).unwrap_or(&surface),
            Some(v @ Value::Array(_)) => Level::from_value(v).unwrap_or(surface),
            _ => surface,
        }
    }

    /// Fallback for unreviewed sprites: base UI piece names, minus artwork-ish ones.
    pub fn fallback_includes(&self, name: &str) -> bool {
        self.include_prefixes.iter().any(|p| name.starts_with(p.as_str()))
            && !self.exclude_contains.iter().any(|e| name.contains(e.as_str()))
    }
}

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// fnmatch-style `*` / `?` matching, case-sensitive.
pub fn glob_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let (mut pi, mut ti) = (0, 0);
    let (mut star, mut mark) = (None, 0);
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

static RULES: OnceLock<RwLock<Rules>> = OnceLock::new();

/// Loads `<data dir>/overrides.json` (or keeps the built-in defaults if missing/invalid).
/// The rules shipped with the plugin (tools/overrides.json at build time).
const BUILTIN: &str = include_str!("../../tools/overrides.json");

/// Loads `<data dir>/overrides.json` if the user made one (custom rules), otherwise the built-in
/// rules.
pub fn load() {
    let path = config::dir().join("overrides.json");
    let (source, text) = match std::fs::read_to_string(&path) {
        Ok(text) => ("custom overrides.json", text),
        Err(_) => ("built-in rules", BUILTIN.to_owned()),
    };
    let rules = match Rules::parse(&text) {
        Ok(r) => {
            log_info!("Loaded {source} ({} sprite rules, {} texture rules)", r.sprites.len(), r.textures.len());
            r
        }
        Err(e) => {
            log_warn!("Invalid {source}, falling back: {e}");
            Rules::parse(BUILTIN).unwrap_or_else(|_| Rules::defaults())
        }
    };
    match RULES.get() {
        Some(lock) => *lock.write().unwrap() = rules,
        None => { let _ = RULES.set(RwLock::new(rules)); }
    }
}

pub fn get() -> std::sync::RwLockReadGuard<'static, Rules> {
    RULES.get().expect("darkmode: rules not loaded").read().unwrap()
}
