//! Remaps dark text colours to light ones so labels stay readable on darkened UI.
//!
//! Hooks `Graphic.set_color` (every colour assignment, from code or presets) and
//! `TextCommon.UpdateColor` (the game's own colour refresh). Only `Gallop.TextCommon` instances are
//! touched, and only when their colour is dark; mid-tone and light colours pass through.
//! The `set_color` hook also darkens plain white overlay Images (see `tick`).

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use crate::api::{self, log_error, log_info, Ptr};
use crate::config;
use crate::unity::{self, unity, Color};

static ORIG_SET_COLOR: AtomicUsize = AtomicUsize::new(0);
static ORIG_UPDATE_COLOR: AtomicUsize = AtomicUsize::new(0);
static ORIG_REBUILD_OUTLINE: AtomicUsize = AtomicUsize::new(0);
static ORIG_SET_GRADIENT1: AtomicUsize = AtomicUsize::new(0);
static ORIG_OUTLINE_APPLY: AtomicUsize = AtomicUsize::new(0);
static ORIG_SHADOW_MODIFY: AtomicUsize = AtomicUsize::new(0);
static ORIG_UI_OUTLINE_MODIFY: AtomicUsize = AtomicUsize::new(0);
static ORIG_OUTLINE_SET_COLOR: AtomicUsize = AtomicUsize::new(0);
static ORIG_SET_GRADIENT2: AtomicUsize = AtomicUsize::new(0);
static ORIG_SET_GRADIENT_VERTICAL: AtomicUsize = AtomicUsize::new(0);
static ENABLED: AtomicBool = AtomicBool::new(true);

pub fn set_enabled(enabled: bool) {
    ENABLED.store(enabled, Ordering::Relaxed);
}

fn is_text(obj: Ptr) -> bool {
    unity::is_instance_of(obj, unity().text_common_class)
}

/// Light version of a dark colour, same hue. `None` if the colour should stay as it is.
pub fn remap(c: Color) -> Option<Color> {
    if c.a <= 0.0 {
        return None;
    }
    let (h, s, l) = rgb_to_hsl(c.r, c.g, c.b);
    // Dark colours, plus muted mid-tones (greys / grey-purples that were readable on white but
    // vanish on grey). Strong mid-tone colours (red, blue, green highlights) stay as they are.
    let muted_mid = l < 0.6 && s < 0.3;
    if l >= 0.45 && !muted_mid {
        return None;
    }
    // 0.0 -> 0.95, 0.45 -> ~0.73: darker text becomes lighter text. Saturation is halved so dark
    // browns become warm off-white rather than peach, which reads better on grey buttons.
    let new_l = 0.95 - l * 0.5;
    let (r, g, b) = hsl_to_rgb(h, (s * 0.5).min(1.0), new_l);
    Some(Color { r, g, b, a: c.a })
}

/// Dark version of a light outline colour, for text whose fill is (now) light. `None` to keep it.
fn remap_outline(fill: Color, outline: Color) -> Option<Color> {
    let (_, _, fill_l) = rgb_to_hsl(fill.r, fill.g, fill.b);
    let (h, s, l) = rgb_to_hsl(outline.r, outline.g, outline.b);
    // Any outline that isn't already dark (including mid-tone coloured ones like orange) would
    // smear light text against grey UI.
    if fill_l < 0.6 || l <= 0.3 {
        return None;
    }
    // Outlines separate text from what's behind it, so make them properly dark (~0.12-0.2) with a
    // hint of their original hue.
    let (r, g, b) = hsl_to_rgb(h, s * 0.6, (0.12 + (1.0 - l) * 0.15).min(0.2));
    Some(Color { r, g, b, a: outline.a })
}

type SetColorFn = extern "C-unwind" fn(this: Ptr, color: Color, method: Ptr);
extern "C-unwind" fn set_color_hook(this: Ptr, color: Color, method: Ptr) {
    let orig: SetColorFn = unsafe { std::mem::transmute(ORIG_SET_COLOR.load(Ordering::Relaxed)) };
    let color = if ENABLED.load(Ordering::Relaxed) && is_text(this) {
        remap(color).unwrap_or(color)
    } else if let Some(dark) = overlay_color(this, color) {
        crate::tick::log_overlay(this);
        dark
    } else {
        color
    };
    orig(this, color, method);
}

type UpdateColorFn = extern "C-unwind" fn(this: Ptr, method: Ptr);
extern "C-unwind" fn update_color_hook(this: Ptr, method: Ptr) {
    let orig: UpdateColorFn = unsafe { std::mem::transmute(ORIG_UPDATE_COLOR.load(Ordering::Relaxed)) };
    orig(this, method);
    if ENABLED.load(Ordering::Relaxed) {
        fix_text(this);
    }
}

/// Plain white overlay Images (no sprite) get a dark colour instead, keeping their alpha.
fn overlay_color(graphic: Ptr, color: Color) -> Option<Color> {
    let dark = crate::tick::overlay_replacement(color)?;
    if !config::enabled() || !unity::is_instance_of(graphic, unity().image_class) {
        return None;
    }
    crate::tick::is_overlay(graphic).then_some(dark)
}

/// `TextCommon.RebuildOutline(Color)`: every outline colour goes through here.
type RebuildOutlineFn = extern "C-unwind" fn(this: Ptr, color: Color, method: Ptr);
extern "C-unwind" fn rebuild_outline_hook(this: Ptr, color: Color, method: Ptr) {
    let orig: RebuildOutlineFn = unsafe { std::mem::transmute(ORIG_REBUILD_OUTLINE.load(Ordering::Relaxed)) };
    let color = if ENABLED.load(Ordering::Relaxed) {
        remap_outline(unity::graphic_color(this), color).unwrap_or(color)
    } else {
        color
    };
    orig(this, color, method);
}

/// Fill colour of the text an OutlineCommon belongs to, if any.
fn outline_fill(outline: Ptr) -> Option<Color> {
    let text = unity::outline_text(outline);
    if text.is_null() || !unity::is_alive(text) {
        return None;
    }
    let fill = unity::graphic_color(text);
    // Texts drawn through a Gallop.GradientCommon (e.g. the roster's RankScore numbers) have a
    // transparent own colour; their visible fill is the (light) gradient.
    if fill.a < 0.05 || GRADIENT_TEXTS.lock().unwrap().as_ref().is_some_and(|s| s.contains(&(text as usize))) {
        return Some(Color { r: 1.0, g: 1.0, b: 1.0, a: 1.0 });
    }
    Some(fill)
}

/// TextCommons that have a GradientCommon providing their visible colour.
static GRADIENT_TEXTS: std::sync::Mutex<Option<std::collections::HashSet<usize>>> = std::sync::Mutex::new(None);
static ORIG_GRADIENT_MODIFY: AtomicUsize = AtomicUsize::new(0);

/// `Gallop.GradientCommon.ModifyMesh`: remember which text it colours.
type GradientModifyFn = extern "C-unwind" fn(this: Ptr, vh: Ptr, method: Ptr);
extern "C-unwind" fn gradient_modify_hook(this: Ptr, vh: Ptr, method: Ptr) {
    let orig: GradientModifyFn = unsafe { std::mem::transmute(ORIG_GRADIENT_MODIFY.load(Ordering::Relaxed)) };
    let text = unity::gradient_text(this);
    if !text.is_null() {
        GRADIENT_TEXTS.lock().unwrap().get_or_insert_with(Default::default).insert(text as usize);
    }
    orig(this, vh, method);
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Color32 {
    r: u8,
    g: u8,
    b: u8,
    a: u8,
}

// Gallop.OutlineCommon draws most text outlines with its own colour, so TextCommon.RebuildOutline
// alone doesn't catch them.
type OutlineApplyFn = extern "C-unwind" fn(this: Ptr, color: Color32, method: Ptr);
extern "C-unwind" fn outline_apply_hook(this: Ptr, color: Color32, method: Ptr) {
    let orig: OutlineApplyFn = unsafe { std::mem::transmute(ORIG_OUTLINE_APPLY.load(Ordering::Relaxed)) };
    let mut out = color;
    if ENABLED.load(Ordering::Relaxed) {
        let c = Color { r: color.r as f32 / 255.0, g: color.g as f32 / 255.0, b: color.b as f32 / 255.0, a: color.a as f32 / 255.0 };
        if let Some(m) = outline_fill(this).and_then(|fill| remap_outline(fill, c)) {
            out = Color32 { r: (m.r * 255.0) as u8, g: (m.g * 255.0) as u8, b: (m.b * 255.0) as u8, a: color.a };
        }
    }
    orig(this, out, method);
}

type OutlineSetColorFn = extern "C-unwind" fn(this: Ptr, color: Color, method: Ptr);
extern "C-unwind" fn outline_set_color_hook(this: Ptr, color: Color, method: Ptr) {
    let orig: OutlineSetColorFn = unsafe { std::mem::transmute(ORIG_OUTLINE_SET_COLOR.load(Ordering::Relaxed)) };
    let color = if ENABLED.load(Ordering::Relaxed) {
        outline_fill(this).and_then(|f| remap_outline(f, color)).unwrap_or(color)
    } else {
        color
    };
    orig(this, color, method);
}

/// Re-applies the outline remap to an OutlineCommon's current colour. Main thread only.
pub fn fix_outline(outline: Ptr) {
    let Some(fill) = outline_fill(outline) else { return };
    let current = unity::outline_color(outline);
    if let Some(dark) = remap_outline(fill, current) {
        unity::set_outline_color(outline, dark);
    }
}

// Unity's own UI Shadow/Outline effects (used by some texts instead of Gallop.OutlineCommon,
// with the colour usually serialized in the prefab). Fixed right before they build their mesh.
fn fix_mesh_effect(effect: Ptr) {
    if !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    let graphic = unity::mesh_effect_graphic(effect);
    if graphic.is_null() || !unity::is_instance_of(graphic, unity().text_class) {
        return;
    }
    let fill = unity::graphic_color(graphic);
    let current = unity::shadow_effect_color(effect);
    if let Some(dark) = remap_outline(fill, current) {
        unity::write_shadow_effect_color(effect, dark);
    }
}

type ModifyMeshFn = extern "C-unwind" fn(this: Ptr, vh: Ptr, method: Ptr);
extern "C-unwind" fn shadow_modify_hook(this: Ptr, vh: Ptr, method: Ptr) {
    let orig: ModifyMeshFn = unsafe { std::mem::transmute(ORIG_SHADOW_MODIFY.load(Ordering::Relaxed)) };
    fix_mesh_effect(this);
    orig(this, vh, method);
}
extern "C-unwind" fn ui_outline_modify_hook(this: Ptr, vh: Ptr, method: Ptr) {
    let orig: ModifyMeshFn = unsafe { std::mem::transmute(ORIG_UI_OUTLINE_MODIFY.load(Ordering::Relaxed)) };
    fix_mesh_effect(this);
    orig(this, vh, method);
}

fn remap_if_enabled(c: Color) -> Color {
    if ENABLED.load(Ordering::Relaxed) { remap(c).unwrap_or(c) } else { c }
}

// Vertical gradient colours (used by e.g. the big stat numbers) are separate from Graphic.color.
type SetGradientFn = extern "C-unwind" fn(this: Ptr, color: Color, method: Ptr);
extern "C-unwind" fn set_gradient1_hook(this: Ptr, color: Color, method: Ptr) {
    let orig: SetGradientFn = unsafe { std::mem::transmute(ORIG_SET_GRADIENT1.load(Ordering::Relaxed)) };
    orig(this, remap_if_enabled(color), method);
}
extern "C-unwind" fn set_gradient2_hook(this: Ptr, color: Color, method: Ptr) {
    let orig: SetGradientFn = unsafe { std::mem::transmute(ORIG_SET_GRADIENT2.load(Ordering::Relaxed)) };
    orig(this, remap_if_enabled(color), method);
}
type SetGradientVerticalFn = extern "C-unwind" fn(this: Ptr, top: Color, bottom: Color, method: Ptr);
extern "C-unwind" fn set_gradient_vertical_hook(this: Ptr, top: Color, bottom: Color, method: Ptr) {
    let orig: SetGradientVerticalFn = unsafe { std::mem::transmute(ORIG_SET_GRADIENT_VERTICAL.load(Ordering::Relaxed)) };
    orig(this, remap_if_enabled(top), remap_if_enabled(bottom), method);
}

/// Re-applies the remap to one text's current colour. Main thread only.
fn fix_text(text: Ptr) {
    let c = unity::graphic_color(text);
    if let Some(light) = remap(c) {
        // Goes through set_color_hook, which is a no-op for already-light colours.
        unity::set_graphic_color(text, light);
        // The outline was built against the old fill; rebuild it so rebuild_outline_hook can
        // darken a light outline.
        unity::text_force_update_outline(text);
    }
    // Existing gradients: re-set them through the (hooked) setters.
    let (g1, g2) = unity::text_gradient_colors(text);
    if remap(g1).is_some() || remap(g2).is_some() {
        unity::set_text_gradient_colors(text, g1, g2);
    }
}

pub fn install_hooks() {
    let u = unity();
    let hooks: [(&str, usize, usize, &AtomicUsize); 11] = [
        ("GradientCommon.ModifyMesh", u.gradient_common_modify_mesh, gradient_modify_hook as *const () as usize, &ORIG_GRADIENT_MODIFY),
        ("UI.Shadow.ModifyMesh", u.shadow_modify_mesh, shadow_modify_hook as *const () as usize, &ORIG_SHADOW_MODIFY),
        ("UI.Outline.ModifyMesh", u.outline_modify_mesh, ui_outline_modify_hook as *const () as usize, &ORIG_UI_OUTLINE_MODIFY),
        ("OutlineCommon.ApplyOutlineColor", u.outline_common_apply_outline_color, outline_apply_hook as *const () as usize, &ORIG_OUTLINE_APPLY),
        ("OutlineCommon.set_color", u.outline_common_set_color, outline_set_color_hook as *const () as usize, &ORIG_OUTLINE_SET_COLOR),
        ("Graphic.set_color", u.graphic_set_color_addr(), set_color_hook as *const () as usize, &ORIG_SET_COLOR),
        ("TextCommon.UpdateColor", u.text_common_update_color, update_color_hook as *const () as usize, &ORIG_UPDATE_COLOR),
        ("TextCommon.RebuildOutline", u.text_common_rebuild_outline, rebuild_outline_hook as *const () as usize, &ORIG_REBUILD_OUTLINE),
        ("TextCommon.set_GradientColor1", u.text_common_set_gradient1, set_gradient1_hook as *const () as usize, &ORIG_SET_GRADIENT1),
        ("TextCommon.set_GradientColor2", u.text_common_set_gradient2, set_gradient2_hook as *const () as usize, &ORIG_SET_GRADIENT2),
        ("TextCommon.SetGradientVertical", u.text_common_set_gradient_vertical, set_gradient_vertical_hook as *const () as usize, &ORIG_SET_GRADIENT_VERTICAL),
    ];
    for (name, target, hook, orig) in hooks {
        match api::hook(target, hook) {
            Some(t) => { orig.store(t, Ordering::Relaxed); log_info!("Hooked {name}"); }
            None => log_error!("Failed to hook {name}"),
        }
    }
}

/// Fixes texts that already exist (created before the hooks, or while disabled). Main thread only.
pub unsafe extern "C" fn fix_all_texts() {
    let _ = std::panic::catch_unwind(|| {
        if !config::get().remap_text {
            return;
        }
        let texts = unity::find_all_of_class(unity().text_common_class);
        let mut fixed = 0;
        for t in &texts {
            let before = unity::graphic_color(*t);
            fix_text(*t);
            if unity::graphic_color(*t) != before {
                fixed += 1;
            }
        }
        log_info!("Text remap: fixed {fixed}/{} existing texts", texts.len());
    });
}

fn rgb_to_hsl(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    if max == min {
        return (0.0, 0.0, l);
    }
    let d = max - min;
    let s = if l > 0.5 { d / (2.0 - max - min) } else { d / (max + min) };
    let h = if max == r {
        (g - b) / d + if g < b { 6.0 } else { 0.0 }
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    } / 6.0;
    (h, s, l)
}

fn hsl_to_rgb(h: f32, s: f32, l: f32) -> (f32, f32, f32) {
    if s == 0.0 {
        return (l, l, l);
    }
    let q = if l < 0.5 { l * (1.0 + s) } else { l + s - l * s };
    let p = 2.0 * l - q;
    let hue = |mut t: f32| {
        if t < 0.0 { t += 1.0; }
        if t > 1.0 { t -= 1.0; }
        if t < 1.0 / 6.0 { p + (q - p) * 6.0 * t }
        else if t < 0.5 { q }
        else if t < 2.0 / 3.0 { p + (q - p) * (2.0 / 3.0 - t) * 6.0 }
        else { p }
    };
    (hue(h + 1.0 / 3.0), hue(h), hue(h - 1.0 / 3.0))
}
