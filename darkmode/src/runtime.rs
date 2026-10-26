//! Runtime dark mode: decides from `overrides.json` what to do with a texture, reads it back from
//! the GPU, processes it (see `process`), and loads the result into the same texture object.
//! Results are cached on disk, keyed by texture name/size, the rules file and the game version,
//! so each texture is only processed once per game/rules version.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::api::{log_info, log_warn, Ptr};
use crate::config;
use crate::process::{self, CardOptions, Image};
use crate::rules::{self, Level, Rule, Rules};
use crate::unity;

/// Bump whenever processing output changes, so cached results are rebuilt.
const PROCESS_VERSION: u32 = 2;

/// A sprite's rect on its texture, in Unity space (origin bottom-left).
#[derive(Clone, Debug)]
pub struct SpriteRect {
    pub name: String,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

enum Action {
    Manual(PathBuf),
    Darken { level: Level, glyphs: bool },
    WhiteSwap(Level),
    Card(CardOptions),
    Regions(Level),
    /// Unreviewed sprite matched by the fallback rules: darken unless it looks like artwork.
    Fallback(Level),
}

enum Plan {
    Atlas(Vec<(SpriteRect, Action)>),
    Whole(Action),
}

fn sprite_action(rules: &Rules, rule: &Rule) -> Option<Action> {
    if rule.flag("skip") {
        return None;
    }
    let level = rules.level(Some(rule));
    Some(match rule.str("mode") {
        Some("white_swap") => Action::WhiteSwap(level),
        Some("card") => Action::Card(CardOptions::from_rule(rule, level)),
        _ => Action::Darken { level, glyphs: rule.flag("light_glyphs") },
    })
}

/// What to do with a texture; `None` if nothing applies.
fn plan(rules: &Rules, manual: &Path, name: &str, sprites: Option<&[SpriteRect]>) -> Option<Plan> {
    if let Some(sprites) = sprites.filter(|s| !s.is_empty()) {
        let mut actions = Vec::new();
        for s in sprites {
            let patch = manual.join(name).join(format!("{}.png", s.name));
            let action = if patch.exists() {
                Some(Action::Manual(patch))
            } else if let Some(rule) = rules.sprite(name, &s.name) {
                sprite_action(rules, rule)
            } else if rules.fallback_includes(&s.name) {
                Some(Action::Fallback(rules.level(None)))
            } else {
                None
            };
            if let Some(a) = action {
                actions.push((s.clone(), a));
            }
        }
        return (!actions.is_empty()).then_some(Plan::Atlas(actions));
    }

    let rule = rules.texture(name);
    let action = match rule.and_then(|r| r.str("mode")).unwrap_or("sprite") {
        "skip" => None,
        "card" => Some(Action::Card(CardOptions::from_rule(rule.unwrap(), rules.level(rule)))),
        "regions" => Some(Action::Regions(rules.level(rule))),
        _ => match rules.sprite("textures", name) {
            Some(srule) => sprite_action(rules, srule),
            None if rules.fallback_includes(name) => Some(Action::Fallback(rules.level(None))),
            None => None,
        },
    };
    action.map(Plan::Whole)
}

fn run(img: &mut Image, action: &Action) -> bool {
    match action {
        Action::Manual(path) => match decode_png(path) {
            Some(patch) => {
                let patch = if (patch.w, patch.h) != (img.w, img.h) {
                    log_warn!("{} is {}x{}, sprite is {}x{}; resizing", path.display(), patch.w, patch.h, img.w, img.h);
                    resize_nearest(&patch, img.w, img.h)
                } else {
                    patch
                };
                *img = patch;
                true
            }
            None => false,
        },
        Action::Darken { level, glyphs } => {
            process::darken(img, *level);
            if *glyphs {
                process::lighten_glyphs(img);
            }
            true
        }
        Action::WhiteSwap(level) => {
            process::white_swap(img, *level);
            true
        }
        Action::Card(opt) => process::darken_card(img, opt),
        Action::Regions(level) => process::darken_large_white_regions(img, *level) > 0,
        Action::Fallback(level) => {
            if process::looks_like_art(img) {
                return false;
            }
            if process::looks_illustrated(img) {
                let opt = CardOptions::from_rule(&Rule::default().with_flag("protect_outlined"), *level);
                let mut work = img.clone();
                if process::darken_card(&mut work, &opt) {
                    *img = work;
                    return true;
                }
                return false;
            }
            process::darken(img, *level);
            true
        }
    }
}

fn execute(img: &mut Image, plan: &Plan) -> usize {
    match plan {
        Plan::Whole(action) => run(img, action) as usize,
        Plan::Atlas(actions) => {
            let mut changed = 0;
            for (s, action) in actions {
                // Unity rect (bottom-left origin) -> top-down pixel box.
                let top = img.h as f32 - (s.y + s.h);
                let x0 = s.x.floor().max(0.0) as usize;
                let y0 = top.floor().max(0.0) as usize;
                let x1 = ((s.x + s.w).ceil() as usize).min(img.w);
                let y1 = ((top + s.h).ceil() as usize).min(img.h);
                if x1 <= x0 || y1 <= y0 {
                    continue;
                }
                let mut region = img.crop(x0, y0, x1, y1);
                if run(&mut region, action) {
                    img.paste(&region, x0, y0);
                    changed += 1;
                }
            }
            changed
        }
    }
}

/// Darkens `tex` according to the rules. Returns false if nothing applied (texture untouched).
/// Main thread only.
pub fn apply(tex: Ptr, kind_dir: &str, name: &str, sprites: Option<&[SpriteRect]>) -> bool {
    let rules = rules::get();
    let Some(plan) = plan(&rules, &config::dir().join("manual"), name, sprites) else { return false };
    let (w, h) = unity::texture_size(tex);
    if w <= 0 || h <= 0 {
        return false;
    }

    let cache = cache_path(&rules, kind_dir, name, w, h);
    if let Ok(bytes) = std::fs::read(&cache) {
        return unity::load_image(tex, &bytes, true);
    }

    let started = std::time::Instant::now();
    let Some(mut img) = read_back(tex) else {
        log_warn!("Could not read back {name}");
        return false;
    };
    let changed = execute(&mut img, &plan);
    if changed == 0 {
        return false;
    }
    let Some(png) = encode_png(&img) else { return false };
    let ok = unity::load_image(tex, &png, true);
    if ok {
        if let Some(parent) = cache.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&cache, &png);
        log_info!("Processed {kind_dir}/{name} ({w}x{h}, {changed} parts) in {} ms", started.elapsed().as_millis());
    }
    ok
}

fn cache_path(rules: &Rules, kind_dir: &str, name: &str, w: i32, h: i32) -> PathBuf {
    // Hand-made patches are part of the input too.
    let mut manual_sig = String::new();
    if let Ok(entries) = std::fs::read_dir(config::dir().join("manual").join(name)) {
        for e in entries.flatten() {
            if let Ok(meta) = e.metadata() {
                let mtime = meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs());
                manual_sig.push_str(&format!("{}:{}:{:?};", e.file_name().to_string_lossy(), meta.len(), mtime));
            }
        }
    }
    let key = format!("{name}|{w}x{h}|{:016x}|{}|{manual_sig}|p{PROCESS_VERSION}", rules.hash, game_version());
    let hash = key.bytes().fold(0xcbf29ce484222325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100000001b3));
    config::dir().join("cache").join(kind_dir).join(format!("{name}-{hash:016x}.png"))
}

fn game_version() -> &'static str {
    static VERSION: OnceLock<String> = OnceLock::new();
    VERSION.get_or_init(|| unity::application_version().unwrap_or_else(|| "unknown".into()))
}

/// Reads any texture (even GPU-only/compressed) back as top-down RGBA8.
fn read_back(tex: Ptr) -> Option<Image> {
    let copy = unity::readable_copy(tex)?;
    let (w, h) = unity::texture_size(copy);
    let pixels = unity::get_pixels32(copy);
    unity::destroy(copy);
    let (w, h) = (w as usize, h as usize);
    if pixels.len() != w * h * 4 {
        return None;
    }
    // Unity pixel arrays start at the bottom row.
    let mut data = vec![0u8; pixels.len()];
    for y in 0..h {
        let src = (h - 1 - y) * w * 4;
        data[y * w * 4..(y + 1) * w * 4].copy_from_slice(&pixels[src..src + w * 4]);
    }
    Some(Image { w, h, data })
}

fn encode_png(img: &Image) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, img.w as u32, img.h as u32);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_compression(png::Compression::Fast);
        let mut writer = enc.write_header().ok()?;
        writer.write_image_data(&img.data).ok()?;
    }
    Some(out)
}

fn decode_png(path: &Path) -> Option<Image> {
    let file = std::fs::File::open(path).ok()?;
    let mut dec = png::Decoder::new(std::io::BufReader::new(file));
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = dec.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).ok()?;
    let (w, h) = (info.width as usize, info.height as usize);
    let src = &buf[..info.buffer_size()];
    let data = match info.color_type {
        png::ColorType::Rgba => src.to_vec(),
        png::ColorType::Rgb => src.chunks(3).flat_map(|c| [c[0], c[1], c[2], 255]).collect(),
        png::ColorType::GrayscaleAlpha => src.chunks(2).flat_map(|c| [c[0], c[0], c[0], c[1]]).collect(),
        png::ColorType::Grayscale => src.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        _ => return None,
    };
    Some(Image { w, h, data })
}

fn resize_nearest(img: &Image, w: usize, h: usize) -> Image {
    let mut data = vec![0u8; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            let sx = x * img.w / w;
            let sy = y * img.h / h;
            let s = (sy * img.w + sx) * 4;
            data[(y * w + x) * 4..(y * w + x) * 4 + 4].copy_from_slice(&img.data[s..s + 4]);
        }
    }
    Image { w, h, data }
}

#[cfg(test)]
mod tests {
    //! Parity with the Python reference: runs the same rules on the dumped originals in
    //! `../research/dump` and compares against `../research/out` (built by build_dark_atlases.py).
    use super::*;

    fn research() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("research")
    }

    fn sprites_for(kind: &str, name: &str) -> Option<Vec<SpriteRect>> {
        let text = std::fs::read_to_string(research().join("loaded_assets.txt")).ok()?;
        let mut found = None;
        for line in text.lines() {
            let cols: Vec<&str> = line.split('\t').collect();
            if cols.len() >= 5 && cols[0] == kind && cols[1] == name && !cols[4].is_empty() {
                found = Some(
                    cols[4]
                        .split(';')
                        .filter_map(|e| {
                            let (n, r) = e.rsplit_once('@')?;
                            let v: Vec<f32> = r.split(',').filter_map(|x| x.parse().ok()).collect();
                            (v.len() == 4).then(|| SpriteRect { name: n.into(), x: v[0], y: v[1], w: v[2], h: v[3] })
                        })
                        .collect(),
                );
            }
        }
        found
    }

    fn compare(kind: &str, name: &str) {
        // research/ holds dumped game textures and is local-only (never committed); skip without it.
        let original = research().join("dump").join(kind).join(format!("{name}.png"));
        let reference = research().join("out").join(kind).join(format!("{name}.png"));
        if !original.exists() || !reference.exists() {
            eprintln!("{kind}/{name}: skipped (no local research data)");
            return;
        }
        let rules = Rules::parse(&std::fs::read_to_string(research().join("../tools/overrides.json")).unwrap()).unwrap();
        let sprites = sprites_for(kind, name);
        let mut img = decode_png(&original).unwrap();
        let expected = decode_png(&reference).unwrap();
        let plan = plan(&rules, &research().join("../tools/manual"), name, sprites.as_deref()).expect("plan");
        execute(&mut img, &plan);
        let (mut max, mut sum, mut big) = (0i32, 0u64, 0usize);
        for (a, b) in img.data.iter().zip(&expected.data) {
            let d = (*a as i32 - *b as i32).abs();
            max = max.max(d);
            sum += d as u64;
            if d > 8 {
                big += 1;
            }
        }
        let mean = sum as f64 / img.data.len() as f64;
        eprintln!("{kind}/{name}: max diff {max}, mean {mean:.4}, >8: {big}");
        assert!(mean < 0.5 && big < img.data.len() / 1000, "{kind}/{name} differs from the Python reference");
    }

    #[test]
    fn parity_atlases() {
        for name in ["PreIn", "Common", "SingleCommon", "TeamStadium", "Home"] {
            compare("atlas", name);
        }
        compare("textures", "SteamPreIn_tex");
    }

    #[test]
    fn parity_loose_textures() {
        for name in [
            "utx_btn_home_story_00", "utx_btn_home_story_02", "utx_btn_home_race_02", "tx_uTex_fl_footer_btn00_0_C",
            "utx_frm_white00", "utx_btn_storyevent_top_00_1021", "utx_btn_storyevent_top_01",
        ] {
            compare("textures", name);
        }
    }
}
