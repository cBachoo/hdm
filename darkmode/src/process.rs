//! Image processing for dark mode, run at load time on textures read back from the GPU.
//!
//! A port of `tools/build_dark_atlases.py` (keep the two in sync; the Python version is the
//! reference used for previewing and tuning). Images are RGBA8, top-down rows. Morphology and
//! filters match scipy.ndimage defaults: cross-shaped structuring element for dilation/erosion,
//! 8-connected labelling where noted, gaussian with truncate=4 and mode="reflect".

use std::collections::VecDeque;

use crate::rules::{Level, Rule};

pub const SAT_LOW: f32 = 0.20;
pub const SAT_HIGH: f32 = 0.45;
const ART_COLORFUL_MIN: f32 = 0.45;
const ART_NEUTRAL_MIN: f32 = 0.05;
const CARD_PANEL_NEUTRAL: f32 = 0.85;
const CARD_BLEND: f32 = 0.10;
const CARD_BLEND_WIDE: f32 = 0.18;
const CARD_RING: f32 = 14.0;
const CARD_FEATHER: f32 = 1.5;
const BASE_SMOOTH: f32 = 0.8;
const REGION_MIN_PIXELS: usize = 3000;
const REGION_MIN_SHARE: f32 = 0.02;

#[derive(Clone)]
pub struct Image {
    pub w: usize,
    pub h: usize,
    pub data: Vec<u8>, // RGBA8, top-down
}

impl Image {
    pub fn crop(&self, x0: usize, y0: usize, x1: usize, y1: usize) -> Image {
        let (w, h) = (x1 - x0, y1 - y0);
        let mut data = Vec::with_capacity(w * h * 4);
        for y in y0..y1 {
            let row = (y * self.w + x0) * 4;
            data.extend_from_slice(&self.data[row..row + w * 4]);
        }
        Image { w, h, data }
    }

    pub fn paste(&mut self, img: &Image, x0: usize, y0: usize) {
        for y in 0..img.h {
            let dst = ((y0 + y) * self.w + x0) * 4;
            let src = y * img.w * 4;
            self.data[dst..dst + img.w * 4].copy_from_slice(&img.data[src..src + img.w * 4]);
        }
    }

    fn px(&self, i: usize) -> [f32; 3] {
        let p = &self.data[i * 4..i * 4 + 3];
        [p[0] as f32, p[1] as f32, p[2] as f32]
    }

    fn alpha(&self, i: usize) -> u8 {
        self.data[i * 4 + 3]
    }

    fn set_rgb(&mut self, i: usize, c: [f32; 3]) {
        for k in 0..3 {
            self.data[i * 4 + k] = c[k].clamp(0.0, 255.0) as u8;
        }
    }

    fn len(&self) -> usize {
        self.w * self.h
    }
}

fn max3(c: [f32; 3]) -> f32 {
    c[0].max(c[1]).max(c[2])
}
fn min3(c: [f32; 3]) -> f32 {
    c[0].min(c[1]).min(c[2])
}
/// Saturation as in the Python tool: (max - min) / max(max, 1), on 0..255 values.
fn sat(c: [f32; 3]) -> f32 {
    let mx = max3(c);
    if mx > 0.0 { (mx - min3(c)) / mx.max(1.0) } else { 0.0 }
}

// ---------------------------------------------------------------------------------------------
// Pixel operations

/// Darken low-saturation pixels, keeping relative shading; leave colourful pixels alone.
pub fn darken(img: &mut Image, level: Level) {
    for i in 0..img.len() {
        let c = img.px(i);
        let w = ((SAT_HIGH - sat(c)) / (SAT_HIGH - SAT_LOW)).clamp(0.0, 1.0);
        let mut out = [0.0; 3];
        for k in 0..3 {
            let dark = level.base[k] + c[k] * level.scale;
            out[k] = dark * w + c[k] * (1.0 - w);
        }
        img.set_rgb(i, out);
    }
}

/// Swap the white mixed into each pixel for `target`, scaled by `amount` (0..1 per pixel).
fn replace_white_px(c: [f32; 3], amount: f32, target: [f32; 3]) -> [f32; 3] {
    let a = min3(c) / 255.0 * amount;
    [c[0] - a * (255.0 - target[0]), c[1] - a * (255.0 - target[1]), c[2] - a * (255.0 - target[2])]
}

pub fn white_swap(img: &mut Image, level: Level) {
    let target = level.white();
    for i in 0..img.len() {
        let c = replace_white_px(img.px(i), 1.0, target);
        img.set_rgb(i, c);
    }
}

/// Turn dark, coloured pixels (e.g. the brown "i" on info buttons) light, keeping their hue.
pub fn lighten_glyphs(img: &mut Image) {
    let opaque: Vec<bool> = (0..img.len()).map(|i| img.alpha(i) > 128).collect();
    let interior = erode(&opaque, img.w, img.h, 4);
    for i in 0..img.len() {
        if !interior[i] {
            continue;
        }
        let c = img.px(i);
        let x = [c[0] / 255.0, c[1] / 255.0, c[2] / 255.0];
        let mx = max3(x);
        let mn = min3(x);
        let light = (mx + mn) / 2.0;
        let s = if mx > 0.0 { (mx - mn) / mx.max(1e-6) } else { 0.0 };
        if !(light < 0.4 && s > 0.25) {
            continue;
        }
        let target = 0.95 - 0.5 * light;
        let scale = if light > 0.0 { target / light.max(1e-6) } else { 1.0 };
        let mut out = [0.0; 3];
        for k in 0..3 {
            let lit = (x[k] * scale).clamp(0.0, 1.0);
            out[k] = (lit * 0.85 + target * 0.15) * 255.0;
        }
        img.set_rgb(i, out);
    }
}

/// Unreviewed illustrated cards: characters (dark outlines) over a white panel. Plain buttons, even
/// colourful ones, have no dark outlines. Mirrors `looks_illustrated` in the Python tool.
pub fn looks_illustrated(img: &Image) -> bool {
    if img.w * img.h < 200 * 120 {
        return false;
    }
    let (mut opaque, mut colorful, mut dark, mut neutral) = (0usize, 0usize, 0usize, 0usize);
    for i in 0..img.len() {
        if img.alpha(i) <= 128 {
            continue;
        }
        opaque += 1;
        let c = img.px(i);
        let s = sat(c);
        if s > SAT_HIGH {
            colorful += 1;
        }
        if max3(c) < 120.0 {
            dark += 1;
        }
        if s < SAT_LOW && max3(c) > 150.0 {
            neutral += 1;
        }
    }
    let share = |n: usize| n as f32 / opaque.max(1) as f32;
    opaque > 0 && share(colorful) >= 0.05 && share(dark) >= 0.06 && share(neutral) >= 0.20
}

/// Illustrated sprites mix lots of colour with light neutral areas; plain UI pieces don't.
pub fn looks_like_art(img: &Image) -> bool {
    let (mut opaque, mut colorful, mut neutral_light) = (0usize, 0usize, 0usize);
    for i in 0..img.len() {
        if img.alpha(i) <= 128 {
            continue;
        }
        opaque += 1;
        let c = img.px(i);
        let s = sat(c);
        if s > SAT_HIGH {
            colorful += 1;
        }
        if s < SAT_LOW && max3(c) > 150.0 {
            neutral_light += 1;
        }
    }
    if opaque == 0 {
        return false;
    }
    colorful as f32 / opaque as f32 >= ART_COLORFUL_MIN && neutral_light as f32 / opaque as f32 >= ART_NEUTRAL_MIN
}

// ---------------------------------------------------------------------------------------------
// Cards: dark base + original artwork on top

pub struct CardOptions {
    pub panel_start: Option<i64>,
    pub direction: Option<String>,
    pub blend: Option<f32>,
    pub ring: f32,
    pub protect_art: bool,
    pub protect_outlined: bool,
    pub feather: f32,
    pub level: Level,
}

impl CardOptions {
    pub fn from_rule(rule: &Rule, level: Level) -> Self {
        CardOptions {
            panel_start: rule.int("panel_start"),
            direction: rule.str("direction").map(str::to_owned),
            blend: rule.float("blend"),
            ring: rule.float("ring").unwrap_or(CARD_RING),
            protect_art: rule.flag("protect_art"),
            protect_outlined: rule.flag("protect_outlined"),
            feather: rule.float("feather").unwrap_or(CARD_FEATHER),
            level,
        }
    }
}

/// Darken a card's label panel and frame, fading smoothly into the artwork. False if no panel.
pub fn darken_card(img: &mut Image, opt: &CardOptions) -> bool {
    let (w, h) = (img.w, img.h);
    let n = img.len();
    let mut mx = vec![0f32; n];
    let mut sats = vec![0f32; n];
    let mut opaque = vec![false; n];
    for i in 0..n {
        let c = img.px(i);
        mx[i] = max3(c);
        sats[i] = sat(c);
        opaque[i] = img.alpha(i) > 128;
    }
    if !opaque.iter().any(|&o| o) {
        return false;
    }
    let neutral: Vec<bool> = (0..n).map(|i| opaque[i] && sats[i] < 0.15 && mx[i] > 170.0).collect();

    let wide = match opt.direction.as_deref() {
        Some(d) => d == "right",
        None => w as f32 > h as f32 * 1.6,
    };
    let size = if wide { w } else { h };
    // Per column (wide) or per row (square): opaque count and neutral share.
    let mut count = vec![0usize; size];
    let mut ncount = vec![0usize; size];
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            let line = if wide { x } else { y };
            if opaque[i] {
                count[line] += 1;
            }
            if neutral[i] {
                ncount[line] += 1;
            }
        }
    }
    let share: Vec<f32> = (0..size).map(|l| if count[l] > 0 { ncount[l] as f32 / count[l] as f32 } else { 0.0 }).collect();
    let cmax = *count.iter().max().unwrap_or(&0) as f32;
    let filled: Vec<bool> = count.iter().map(|&c| c as f32 > 0.5 * cmax).collect();
    let Some(last) = filled.iter().rposition(|&f| f) else { return false };

    let start: i64 = if let Some(s) = opt.panel_start {
        s
    } else {
        let mut edge = last;
        while edge > 0 && share[edge] < CARD_PANEL_NEUTRAL && last - edge < 20 {
            edge -= 1;
        }
        let mut start = edge;
        while start > 0 && (share[start - 1] >= CARD_PANEL_NEUTRAL || !filled[start - 1]) {
            start -= 1;
        }
        if edge - start < 8 {
            return false;
        }
        start as i64
    };

    let blend = opt.blend.unwrap_or(if wide { CARD_BLEND_WIDE } else { CARD_BLEND });
    let band = ((blend * size as f32) as i64).max(8) as f32;
    let ramp: Vec<f32> = (0..size).map(|p| ((p as f32 - (start as f32 - band)) / band).clamp(0.0, 1.0)).collect();
    let mut amount: Vec<f32> = (0..n).map(|i| ramp[if wide { i % w } else { i / w }]).collect();

    if opt.protect_art {
        let candidate: Vec<bool> = (0..n).map(|i| opaque[i] && sats[i] < 0.15 && mx[i] > 150.0).collect();
        let (labels, _) = label8(&candidate, w, h);
        let mut in_panel = std::collections::HashSet::new();
        for i in 0..n {
            let line = if wide { i % w } else { i / w } as i64;
            if line >= start && labels[i] != 0 {
                in_panel.insert(labels[i]);
            }
        }
        let connected: Vec<bool> = labels.iter().map(|l| *l != 0 && in_panel.contains(l)).collect();
        let connected = dilate(&connected, w, h, 2, false);
        for i in 0..n {
            if !(connected[i] && sats[i] < SAT_HIGH) {
                amount[i] = 0.0;
            }
        }
    }

    if opt.protect_outlined {
        let outline: Vec<bool> = (0..n).map(|i| opaque[i] && mx[i] < 120.0).collect();
        let closed = erode_sq(&dilate(&outline, w, h, 2, true), w, h, 2);
        let filled_shapes = fill_holes(&closed, w, h);
        let silhouettes = dilate(&filled_shapes, w, h, 1, false);
        for i in 0..n {
            if silhouettes[i] {
                amount[i] = 0.0;
            }
        }
    }

    // Frame ring around the card edge (white bevel), only where it's neutral.
    let dist = edt(&opaque, w, h);
    for i in 0..n {
        if dist[i] <= opt.ring && sats[i] < 0.2 {
            amount[i] = 1.0;
        }
        if !opaque[i] {
            amount[i] = 0.0;
        }
    }

    composite_over_dark_base(img, &amount, opt.level.white(), opt.feather);
    true
}

/// Recreate an image as clean dark base (white swapped for `target`, lightly smoothed) with the
/// original on top where `amount` is 0, using a feathered mask.
fn composite_over_dark_base(img: &mut Image, amount: &[f32], target: [f32; 3], feather: f32) {
    let (w, h) = (img.w, img.h);
    let n = img.len();
    let alpha: Vec<f32> = (0..n).map(|i| img.alpha(i) as f32 / 255.0).collect();
    let weight = gaussian(&alpha, w, h, BASE_SMOOTH);
    let mut dark_ch = [vec![0f32; n], vec![0f32; n], vec![0f32; n]];
    for i in 0..n {
        let d = replace_white_px(img.px(i), 1.0, target);
        for k in 0..3 {
            // replace_white in the Python version rounds to uint8 before smoothing.
            dark_ch[k][i] = d[k].clamp(0.0, 255.0).trunc() * alpha[i];
        }
    }
    for k in 0..3 {
        dark_ch[k] = gaussian(&dark_ch[k], w, h, BASE_SMOOTH);
    }
    let mut keep: Vec<f32> = amount.iter().map(|a| 1.0 - a.clamp(0.0, 1.0)).collect();
    if feather > 0.0 {
        keep = gaussian(&keep, w, h, feather);
    }
    for i in 0..n {
        let c = img.px(i);
        let wgt = weight[i].max(1e-6);
        let mut out = [0.0; 3];
        for k in 0..3 {
            let d = dark_ch[k][i] / wgt;
            out[k] = d * (1.0 - keep[i]) + c[k] * keep[i];
        }
        img.set_rgb(i, out);
    }
}

/// Darken big connected near-white neutral areas (e.g. footer tiles). Returns how many.
pub fn darken_large_white_regions(img: &mut Image, level: Level) -> usize {
    let (w, h) = (img.w, img.h);
    let n = img.len();
    let opaque: Vec<bool> = (0..n).map(|i| img.alpha(i) > 0).collect();
    let candidate: Vec<bool> = (0..n)
        .map(|i| {
            let c = img.px(i);
            opaque[i] && sat(c) < 0.15 && max3(c) > 180.0
        })
        .collect();
    let (labels, count) = label8(&candidate, w, h);
    if count == 0 {
        return 0;
    }
    let mut sizes = vec![0usize; count + 1];
    for &l in &labels {
        sizes[l] += 1;
    }
    let opaque_count = opaque.iter().filter(|&&o| o).count();
    let min_size = (REGION_MIN_PIXELS as f32).max(REGION_MIN_SHARE * opaque_count as f32);
    let keep: Vec<bool> = (0..=count).map(|l| l != 0 && sizes[l] as f32 >= min_size).collect();
    let kept = keep.iter().filter(|&&k| k).count();
    if kept == 0 {
        return 0;
    }
    let region: Vec<bool> = labels.iter().map(|&l| keep[l]).collect();
    let region = dilate(&region, w, h, 2, false);
    let mut patch = img.clone();
    darken(&mut patch, level);
    for i in 0..n {
        if region[i] && opaque[i] && sat(img.px(i)) < SAT_HIGH {
            let c = patch.px(i);
            img.set_rgb(i, c);
        }
    }
    kept
}

// ---------------------------------------------------------------------------------------------
// Morphology / filters (scipy.ndimage equivalents)

fn neighbours(square: bool) -> &'static [(i64, i64)] {
    const CROSS: [(i64, i64); 4] = [(-1, 0), (1, 0), (0, -1), (0, 1)];
    const SQUARE: [(i64, i64); 8] = [(-1, 0), (1, 0), (0, -1), (0, 1), (-1, -1), (-1, 1), (1, -1), (1, 1)];
    if square { &SQUARE } else { &CROSS }
}

/// binary_dilation (cross, or 3x3 square if `square`), `iters` times.
fn dilate(mask: &[bool], w: usize, h: usize, iters: usize, square: bool) -> Vec<bool> {
    let mut cur = mask.to_vec();
    for _ in 0..iters {
        let mut next = cur.clone();
        for y in 0..h {
            for x in 0..w {
                if cur[y * w + x] {
                    continue;
                }
                for &(dx, dy) in neighbours(square) {
                    let (nx, ny) = (x as i64 + dx, y as i64 + dy);
                    if nx >= 0 && ny >= 0 && (nx as usize) < w && (ny as usize) < h && cur[ny as usize * w + nx as usize] {
                        next[y * w + x] = true;
                        break;
                    }
                }
            }
        }
        cur = next;
    }
    cur
}

/// binary_erosion with the given neighbourhood; outside the image counts as false.
fn erode_with(mask: &[bool], w: usize, h: usize, iters: usize, square: bool) -> Vec<bool> {
    let mut cur = mask.to_vec();
    for _ in 0..iters {
        let mut next = cur.clone();
        for y in 0..h {
            for x in 0..w {
                if !cur[y * w + x] {
                    continue;
                }
                for &(dx, dy) in neighbours(square) {
                    let (nx, ny) = (x as i64 + dx, y as i64 + dy);
                    let inside = nx >= 0 && ny >= 0 && (nx as usize) < w && (ny as usize) < h;
                    if !inside || !cur[ny as usize * w + nx as usize] {
                        next[y * w + x] = false;
                        break;
                    }
                }
            }
        }
        cur = next;
    }
    cur
}

fn erode(mask: &[bool], w: usize, h: usize, iters: usize) -> Vec<bool> {
    erode_with(mask, w, h, iters, false)
}

fn erode_sq(mask: &[bool], w: usize, h: usize, iters: usize) -> Vec<bool> {
    erode_with(mask, w, h, iters, true)
}

/// binary_fill_holes: false areas not 4-connected to the border become true.
fn fill_holes(mask: &[bool], w: usize, h: usize) -> Vec<bool> {
    let mut outside = vec![false; w * h];
    let mut queue = VecDeque::new();
    for y in 0..h {
        for x in 0..w {
            if (x == 0 || y == 0 || x == w - 1 || y == h - 1) && !mask[y * w + x] {
                outside[y * w + x] = true;
                queue.push_back((x, y));
            }
        }
    }
    while let Some((x, y)) = queue.pop_front() {
        for &(dx, dy) in neighbours(false) {
            let (nx, ny) = (x as i64 + dx, y as i64 + dy);
            if nx < 0 || ny < 0 || nx as usize >= w || ny as usize >= h {
                continue;
            }
            let j = ny as usize * w + nx as usize;
            if !mask[j] && !outside[j] {
                outside[j] = true;
                queue.push_back((nx as usize, ny as usize));
            }
        }
    }
    outside.iter().map(|o| !o).collect()
}

/// 8-connected component labels (0 = background) and the number of components.
fn label8(mask: &[bool], w: usize, h: usize) -> (Vec<usize>, usize) {
    let mut labels = vec![0usize; w * h];
    let mut next = 0;
    let mut queue = VecDeque::new();
    for start in 0..w * h {
        if !mask[start] || labels[start] != 0 {
            continue;
        }
        next += 1;
        labels[start] = next;
        queue.push_back(start);
        while let Some(i) = queue.pop_front() {
            let (x, y) = ((i % w) as i64, (i / w) as i64);
            for &(dx, dy) in neighbours(true) {
                let (nx, ny) = (x + dx, y + dy);
                if nx < 0 || ny < 0 || nx as usize >= w || ny as usize >= h {
                    continue;
                }
                let j = ny as usize * w + nx as usize;
                if mask[j] && labels[j] == 0 {
                    labels[j] = next;
                    queue.push_back(j);
                }
            }
        }
    }
    (labels, next)
}

/// Exact euclidean distance from each true pixel to the nearest false pixel (0 for false pixels),
/// like scipy's distance_transform_edt. Felzenszwalb & Huttenlocher, two 1-D passes.
fn edt(mask: &[bool], w: usize, h: usize) -> Vec<f32> {
    const INF: f32 = 1e20;
    let mut grid: Vec<f32> = mask.iter().map(|&m| if m { INF } else { 0.0 }).collect();
    let mut f = vec![0f32; w.max(h)];
    let mut d = vec![0f32; w.max(h)];
    let mut v = vec![0usize; w.max(h)];
    let mut z = vec![0f32; w.max(h) + 1];
    let mut pass = |grid: &mut Vec<f32>, len: usize, count: usize, idx: &dyn Fn(usize, usize) -> usize| {
        for line in 0..count {
            for q in 0..len {
                f[q] = grid[idx(line, q)];
            }
            dt1d(&f[..len], &mut d[..len], &mut v, &mut z);
            for q in 0..len {
                grid[idx(line, q)] = d[q];
            }
        }
    };
    pass(&mut grid, h, w, &|x, y| y * w + x);
    pass(&mut grid, w, h, &|y, x| y * w + x);
    grid.iter().map(|g| g.sqrt()).collect()
}

fn dt1d(f: &[f32], d: &mut [f32], v: &mut [usize], z: &mut [f32]) {
    let n = f.len();
    let mut k = 0usize;
    v[0] = 0;
    z[0] = f32::NEG_INFINITY;
    z[1] = f32::INFINITY;
    for q in 1..n {
        loop {
            let p = v[k];
            let s = ((f[q] + (q * q) as f32) - (f[p] + (p * p) as f32)) / (2.0 * q as f32 - 2.0 * p as f32);
            if s <= z[k] && k > 0 {
                k -= 1;
                continue;
            }
            if s <= z[k] {
                // k == 0: replace the only parabola
                v[0] = q;
                z[0] = f32::NEG_INFINITY;
                z[1] = f32::INFINITY;
                break;
            }
            k += 1;
            v[k] = q;
            z[k] = s;
            z[k + 1] = f32::INFINITY;
            break;
        }
    }
    k = 0;
    for q in 0..n {
        while z[k + 1] < q as f32 {
            k += 1;
        }
        let p = v[k];
        let dq = q as f32 - p as f32;
        d[q] = dq * dq + f[p];
    }
}

/// gaussian_filter(sigma, truncate=4.0, mode="reflect"), separable.
fn gaussian(src: &[f32], w: usize, h: usize, sigma: f32) -> Vec<f32> {
    let radius = (4.0 * sigma + 0.5) as i64;
    let mut kernel: Vec<f32> = (-radius..=radius).map(|x| (-(x * x) as f32 / (2.0 * sigma * sigma)).exp()).collect();
    let sum: f32 = kernel.iter().sum();
    kernel.iter_mut().for_each(|k| *k /= sum);
    // scipy "reflect": (d c b a | a b c d | d c b a)
    let reflect = |i: i64, n: i64| -> usize {
        let period = 2 * n;
        let mut m = i.rem_euclid(period);
        if m >= n {
            m = period - 1 - m;
        }
        m as usize
    };
    let mut tmp = vec![0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            let mut acc = 0.0;
            for (k, kv) in kernel.iter().enumerate() {
                let sx = reflect(x as i64 + k as i64 - radius, w as i64);
                acc += src[y * w + sx] * kv;
            }
            tmp[y * w + x] = acc;
        }
    }
    let mut out = vec![0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            let mut acc = 0.0;
            for (k, kv) in kernel.iter().enumerate() {
                let sy = reflect(y as i64 + k as i64 - radius, h as i64);
                acc += tmp[sy * w + x] * kv;
            }
            out[y * w + x] = acc;
        }
    }
    out
}
