"""Builds dark replacement atlases from the plugin's dumps.

Reads sprite rects from `loaded_assets.txt` (written by the plugin with "Log loaded textures" on)
and the original atlases from `dump/{atlas,textures}/`, darkens sprites, and writes the results to
`<out>/{atlas,textures}/<name>.png` for the plugin to load.

What happens to each sprite/texture is decided, in order, by:
  1. a hand-made patch in `tools/manual/` (pasted as-is),
  2. an entry in `tools/overrides.json` (exact name, `Atlas/name`, or `*` glob; first match wins),
  3. the generic fallback rules ("fallback" in overrides.json + artwork detection), for anything not
     reviewed yet, e.g. assets added by game updates.

usage: python build_dark_atlases.py <darkmode data dir> [<out dir>]
       (out dir defaults to the data dir, i.e. installs directly)
"""
import fnmatch
import json
import math
import re
import sys
from pathlib import Path

import numpy as np
from PIL import Image
from scipy import ndimage

TOOLS = Path(__file__).resolve().parent
OVERRIDES_PATH = TOOLS / "overrides.json"
MANUAL_DIR = TOOLS / "manual"

# Darkening levels: c' = base + c * scale per channel (keeps relative shading). Overridable and
# extendable via "levels" in overrides.json.
LEVELS = {
    "background": (np.array([12, 13, 17], dtype=np.float32), 0.08),  # white -> ~32
    "surface": (np.array([24, 26, 34], dtype=np.float32), 0.24),     # white -> ~85
    "raised": (np.array([32, 35, 46], dtype=np.float32), 0.33),      # white -> ~118
}
SURFACE = LEVELS["surface"]

# Saturation below SAT_LOW is treated as neutral (fully darkened); above SAT_HIGH as colour
# (left alone); in between blends. A flat region below SAT_LOW avoids speckling on faintly
# tinted, noisy whites.
SAT_LOW = 0.20
SAT_HIGH = 0.45

# Illustrated sprites (characters on buttons etc.) mix lots of colour with light neutral areas
# like skin and white clothing, which must not be darkened. Plain UI pieces are either almost
# entirely neutral or entirely coloured.
ART_COLORFUL_MIN = 0.45  # share of opaque pixels with saturation > SAT_HIGH
ART_NEUTRAL_MIN = 0.05   # share of opaque pixels that are light and neutral


def darken(rgb: np.ndarray, level) -> np.ndarray:
    """Darken low-saturation pixels, keeping relative shading; leave colourful pixels alone."""
    base, scale = level
    rgb = rgb.astype(np.float32)
    mx = rgb.max(axis=-1, keepdims=True)
    mn = rgb.min(axis=-1, keepdims=True)
    sat = np.where(mx > 0, (mx - mn) / np.maximum(mx, 1), 0)
    dark = base + rgb * scale
    w = np.clip((SAT_HIGH - sat) / (SAT_HIGH - SAT_LOW), 0, 1)
    return (dark * w + rgb * (1 - w)).clip(0, 255).astype(np.uint8)


# Standalone illustrated textures (home menu cards, animated footer). These are darkened by region:
# large connected near-white areas (label panels, card frames, footer tiles) go dark, while the
# small white bits inside artwork (socks, highlights, sparkles) are left alone.
REGION_TEXTURES = re.compile(r"^tx_uTex_fl_footer_btn")
# Home menu cards: artwork on top (square) or left (wide), white label panel below / right.
CARD_TEXTURES = re.compile(r"^utx_btn_home_(story|umamusume|race)_")
CARD_PANEL_NEUTRAL = 0.85  # rows/cols at least this neutral-white belong to the label panel
CARD_BLEND = 0.10          # fade band (fraction of height) above the panel on square cards
CARD_BLEND_WIDE = 0.18     # fade band (fraction of width) left of the panel on wide cards
CARD_RING = 14             # px from the card edge treated as frame
CARD_FEATHER = 1.5         # px (gaussian sigma) of softening between artwork and dark base
BASE_SMOOTH = 0.8          # px (gaussian sigma) of smoothing on the dark base (hides block noise)
REGION_MIN_PIXELS = 3000
REGION_MIN_SHARE = 0.02  # of the texture's opaque pixels


def darken_large_white_regions(im: np.ndarray) -> int:
    """Darken big connected near-white neutral areas in place. Returns how many were darkened."""
    rgb = im[..., :3].astype(np.float32)
    mx = rgb.max(axis=-1)
    mn = rgb.min(axis=-1)
    sat = np.where(mx > 0, (mx - mn) / np.maximum(mx, 1), 0)
    opaque = im[..., 3] > 0
    candidate = opaque & (sat < 0.15) & (mx > 180)

    labels, count = ndimage.label(candidate, structure=np.ones((3, 3)))
    if count == 0:
        return 0
    sizes = ndimage.sum(candidate, labels, index=np.arange(1, count + 1))
    min_size = max(REGION_MIN_PIXELS, REGION_MIN_SHARE * opaque.sum())
    keep = np.flatnonzero(sizes >= min_size) + 1
    if keep.size == 0:
        return 0
    region = np.isin(labels, keep)
    # Grow by a couple of pixels to take the anti-aliased edges with it, but only neutral pixels.
    region = ndimage.binary_dilation(region, iterations=2) & opaque & (sat < SAT_HIGH)
    im[..., :3][region] = darken(im[..., :3][region][None], SURFACE)[0]
    return int(keep.size)


def replace_white(rgb: np.ndarray, amount: np.ndarray, target: np.ndarray) -> np.ndarray:
    """Swap the white mixed into each pixel for `target`, scaled by `amount` (0..1 per pixel).

    The share of white in a pixel is estimated from its smallest channel: pure white is all white,
    saturated colours contain little. Fades from artwork into white therefore become fades into
    `target`, and shading is preserved.
    """
    rgb = rgb.astype(np.float32)
    white_share = rgb.min(axis=-1, keepdims=True) / 255.0
    a = white_share * amount[..., None]
    return (rgb - a * (255.0 - target)).clip(0, 255).astype(np.uint8)


def darken_card(im: np.ndarray, rule=None) -> bool:
    """Darken a home menu card's label panel and frame, fading smoothly into the artwork.

    `rule` (from overrides.json) may set: "panel_start" (px from top/left where the label panel
    begins, skipping detection), "direction" ("bottom" or "right": where the panel is; default
    guessed from the shape), "blend" (fade band as a fraction of size), "ring" (frame width px),
    "protect_art" (only darken white areas connected to the panel, so outlined artwork standing
    over it keeps its whites), "protect_outlined" (leave every area enclosed by dark outlines
    untouched, e.g. chibi characters standing over the panel), "level".
    """
    rule = rule or {}
    rgb = im[..., :3].astype(np.float32)
    mx = rgb.max(axis=-1)
    mn = rgb.min(axis=-1)
    sat = np.where(mx > 0, (mx - mn) / np.maximum(mx, 1), 0)
    opaque = im[..., 3] > 128
    neutral = opaque & (sat < 0.15) & (mx > 170)
    if not opaque.any():
        return False

    h, w = opaque.shape
    direction = rule.get("direction")
    wide = direction == "right" if direction else w > h * 1.6
    axis = 0 if wide else 1  # sum over rows for columns (wide) / over columns for rows (square)
    count = opaque.sum(axis=axis)
    share = np.where(count > 0, neutral.sum(axis=axis) / np.maximum(count, 1), 0)
    filled = count > 0.5 * count.max()
    # Walk inwards from the bottom (square) / right (wide) edge while lines are panel-like.
    idx = np.flatnonzero(filled)
    if idx.size == 0:
        return False
    if "panel_start" in rule:
        start = int(rule["panel_start"])
    else:
        edge = idx[-1]
        # Skip the frame outline/shadow lines at the very edge.
        while edge > 0 and share[edge] < CARD_PANEL_NEUTRAL and idx[-1] - edge < 20:
            edge -= 1
        start = edge
        while start > 0 and (share[start - 1] >= CARD_PANEL_NEUTRAL or not filled[start - 1]):
            start -= 1
        if edge - start < 8:
            return False

    # Ramp 0 -> 1 across a band ending where the panel starts, 1 inside the panel.
    size = w if wide else h
    band = max(8, int(rule.get("blend", CARD_BLEND_WIDE if wide else CARD_BLEND) * size))
    pos = np.arange(size, dtype=np.float32)
    ramp = np.clip((pos - (start - band)) / band, 0, 1)
    amount = np.broadcast_to(ramp[None, :] if wide else ramp[:, None], (h, w)).copy()

    if rule.get("protect_art"):
        # Keep only near-white regions that reach into the panel itself; artwork with outlines
        # (characters' socks, highlights) forms separate regions and is left alone.
        candidate = opaque & (sat < 0.15) & (mx > 150)
        labels, _ = ndimage.label(candidate, structure=np.ones((3, 3)))
        panel = np.zeros_like(opaque)
        if wide:
            panel[:, start:] = True
        else:
            panel[start:, :] = True
        ids = np.unique(labels[panel & candidate])
        ids = ids[ids != 0]
        connected = ndimage.binary_dilation(np.isin(labels, ids), iterations=2) & (sat < SAT_HIGH)
        amount = np.where(connected, amount, 0.0)

    if rule.get("protect_outlined"):
        # Dark outline pixels, gaps closed, enclosed areas filled: whole outlined shapes (characters
        # incl. their white shirts/socks) become protected silhouettes.
        outline = opaque & (mx < 120)
        closed = ndimage.binary_closing(outline, structure=np.ones((3, 3)), iterations=2)
        silhouettes = ndimage.binary_fill_holes(closed)
        silhouettes = ndimage.binary_dilation(silhouettes, iterations=1)
        amount = np.where(silhouettes, 0.0, amount)

    # Frame ring around the card edge (white bevel), only where it's neutral.
    ring = (ndimage.distance_transform_edt(opaque) <= rule.get("ring", CARD_RING)) & (sat < 0.2)
    amount = np.where(ring, 1.0, amount) * opaque

    base, scale = resolve_level(rule.get("level", "surface"))
    target = base + 255.0 * scale
    im[..., :3] = composite_over_dark_base(im, amount, target, rule.get("feather", CARD_FEATHER))
    return True


def composite_over_dark_base(im: np.ndarray, amount: np.ndarray, target: np.ndarray, feather: float):
    """Recreate a card as clean dark base + original artwork on top.

    `amount` is 1 where the dark base should show (panel, frame) and 0 where the original artwork
    stays. The base is the whole image with its white swapped for `target`, lightly smoothed to
    hide GPU compression blocks (it's a flat panel, so nothing is lost). The keep mask is feathered
    so the edges between artwork and base are soft instead of stair-stepped.
    """
    rgb = im[..., :3].astype(np.float32)
    alpha = im[..., 3].astype(np.float32) / 255.0

    dark = replace_white(rgb, np.ones(alpha.shape, dtype=np.float32), target).astype(np.float32)
    # Alpha-weighted blur so transparent pixels outside the card don't bleed into its edge.
    weight = ndimage.gaussian_filter(alpha, BASE_SMOOTH)
    dark = np.stack([ndimage.gaussian_filter(dark[..., c] * alpha, BASE_SMOOTH) for c in range(3)], axis=-1)
    dark = dark / np.maximum(weight, 1e-6)[..., None]

    keep = 1.0 - np.clip(amount, 0, 1)
    if feather > 0:
        keep = ndimage.gaussian_filter(keep, feather)
    keep = keep[..., None]
    return (dark * (1 - keep) + rgb * keep).clip(0, 255).astype(np.uint8)


def lighten_glyphs(rgb: np.ndarray, alpha: np.ndarray) -> np.ndarray:
    """Turn dark, coloured pixels (e.g. the brown "i" on info buttons) light, keeping their hue.

    Same lightness mapping as the plugin's text remap: L -> 0.95 - 0.5 * L. Neutral pixels (the
    darkened button body) and lighter ones are left alone, as is a 4 px band along the sprite's
    edge so dark outlines/rims aren't caught.
    """
    x = rgb.astype(np.float32) / 255.0
    mx = x.max(axis=-1)
    mn = x.min(axis=-1)
    light = (mx + mn) / 2
    sat = np.where(mx > 0, (mx - mn) / np.maximum(mx, 1e-6), 0)
    interior = ndimage.binary_erosion(alpha > 128, iterations=4)
    glyph = (light < 0.4) & (sat > 0.25) & interior
    if not glyph.any():
        return rgb
    target = 0.95 - 0.5 * light
    # Scale each pixel's colour around its own lightness so hue and saturation carry over.
    scale = np.where(light > 0, target / np.maximum(light, 1e-6), 1)
    lit = np.clip(x * scale[..., None], 0, 1)
    # Pull towards grey a little where scaling clipped a channel (keeps it from going neon).
    out = np.where(glyph[..., None], lit * 0.85 + target[..., None] * 0.15, x)
    return (out * 255).clip(0, 255).astype(np.uint8)


def looks_like_art(region: np.ndarray) -> bool:
    opaque = region[..., 3] > 128
    if not opaque.any():
        return False
    rgb = region[..., :3][opaque].astype(np.float32)
    mx = rgb.max(axis=-1)
    mn = rgb.min(axis=-1)
    sat = np.where(mx > 0, (mx - mn) / np.maximum(mx, 1), 0)
    colorful = (sat > SAT_HIGH).mean()
    neutral_light = ((sat < SAT_LOW) & (mx > 150)).mean()
    return colorful >= ART_COLORFUL_MIN and neutral_light >= ART_NEUTRAL_MIN


# Fallback rules for unreviewed sprites, from overrides.json "fallback": base UI piece name
# prefixes (buttons, frames, windows, list rows, tabs, sliders, gauges, text plates), minus names
# containing artwork-ish words. Shared with the plugin so both behave the same.
INCLUDE_PREFIXES: list = []
EXCLUDE_CONTAINS: list = []


def fallback_includes(name: str) -> bool:
    return any(name.startswith(p) for p in INCLUDE_PREFIXES) and not any(e in name for e in EXCLUDE_CONTAINS)


class Overrides:
    """Per-asset settings from overrides.json. Keys: exact name, "Atlas/name", or fnmatch globs."""

    def __init__(self, path: Path):
        data = json.loads(path.read_text(encoding="utf-8")) if path.exists() else {}
        fallback = data.get("fallback", {})
        INCLUDE_PREFIXES[:] = fallback.get("include_prefixes", [])
        EXCLUDE_CONTAINS[:] = fallback.get("exclude_contains", [])
        for name, v in data.get("levels", {}).items():
            LEVELS[name] = (np.array(v[:3], dtype=np.float32), float(v[3]))
        self.sprites = list(data.get("sprites", {}).items())
        self.textures = list(data.get("textures", {}).items())

    @staticmethod
    def _find(entries, atlas, name):
        for exact in (f"{atlas}/{name}", name):
            for key, value in entries:
                if key == exact:
                    return value
        for key, value in entries:
            if "*" in key or "?" in key:
                if fnmatch.fnmatchcase(f"{atlas}/{name}" if "/" in key else name, key):
                    return value
        return None

    def sprite(self, atlas, name):
        return self._find(self.sprites, atlas, name)

    def texture(self, name):
        return self._find(self.textures, "textures", name)


def resolve_level(level):
    if isinstance(level, list):
        return (np.array(level[:3], dtype=np.float32), float(level[3]))
    return LEVELS[level]


def sprite_box(height, x, y, w, h):
    """Pixel box (x0, y0, x1, y1) of a Unity rect (bottom-left origin) in a top-down image."""
    top = height - (y + h)
    return math.floor(x), math.floor(top), math.ceil(x + w), math.ceil(top + h)


def paste_manual(im, path, box):
    patch = Image.open(path).convert("RGBA")
    x0, y0, x1, y1 = box
    if patch.size != (x1 - x0, y1 - y0):
        print(f"  warning: {path.name} is {patch.size}, sprite box is {(x1 - x0, y1 - y0)}; resizing")
        patch = patch.resize((x1 - x0, y1 - y0), Image.LANCZOS)
    im[y0:y1, x0:x1] = np.array(patch)


# Unreviewed illustrated cards (characters with dark outlines over a white panel, e.g. new event
# buttons): big enough, some colour, and a noticeable share of dark outline pixels. Plain buttons,
# even colourful ones, have no dark outlines.
ILLUSTRATED_MIN_AREA = 200 * 120
ILLUSTRATED_COLORFUL_MIN = 0.05
ILLUSTRATED_DARK_MIN = 0.06
ILLUSTRATED_NEUTRAL_MIN = 0.20  # the white label panel


def looks_illustrated(region: np.ndarray) -> bool:
    h, w = region.shape[:2]
    opaque = region[..., 3] > 128
    if w * h < ILLUSTRATED_MIN_AREA or not opaque.any():
        return False
    rgb = region[..., :3][opaque].astype(np.float32)
    mx = rgb.max(axis=-1)
    mn = rgb.min(axis=-1)
    sat = np.where(mx > 0, (mx - mn) / np.maximum(mx, 1), 0)
    return (
        (sat > SAT_HIGH).mean() >= ILLUSTRATED_COLORFUL_MIN
        and (mx < 120).mean() >= ILLUSTRATED_DARK_MIN
        and ((sat < SAT_LOW) & (mx > 150)).mean() >= ILLUSTRATED_NEUTRAL_MIN
    )


def apply_fallback(region: np.ndarray) -> str:
    """Fallback for unreviewed sprites. Returns what was done: "art", "card", "nocard", "darkened"."""
    if looks_like_art(region):
        return "art"
    if looks_illustrated(region):
        work = region.copy()
        if darken_card(work, {"protect_outlined": True}):
            region[...] = work
            return "card"
        return "nocard"
    region[..., :3] = darken(region[..., :3], SURFACE)
    return "darkened"


def parse_assets(path: Path):
    """{(kind, name): [(sprite name, x, y, w, h), ...]} with Unity (bottom-left origin) rects.

    kind is "atlas" (an AtlasReference) or "textures" (a loose atlas texture like SteamPreIn_tex).
    """
    atlases = {}
    for line in path.read_text(encoding="utf-8").splitlines():
        cols = line.split("\t")
        if len(cols) < 5 or cols[0] not in ("atlas", "textures") or not cols[4]:
            continue
        sprites = []
        for entry in cols[4].split(";"):
            name, _, rect = entry.rpartition("@")
            try:
                x, y, w, h = map(float, rect.split(","))
            except ValueError:
                continue
            sprites.append((name, x, y, w, h))
        atlases[(cols[0], cols[1])] = sprites
    return atlases


def main():
    data = Path(sys.argv[1])
    out = Path(sys.argv[2]) if len(sys.argv) > 2 else data
    overrides = Overrides(OVERRIDES_PATH)
    atlases = parse_assets(data / "loaded_assets.txt")

    for (kind, atlas), sprites in sorted(atlases.items()):
        src = data / "dump" / kind / f"{atlas}.png"
        if not src.exists():
            print(f"skip {atlas}: no dump")
            continue

        im = np.array(Image.open(src).convert("RGBA"))
        height = im.shape[0]
        changed, skipped, manual = 0, [], 0
        for name, x, y, w, h in sprites:
            box = sprite_box(height, x, y, w, h)
            x0, y0, x1, y1 = box
            patch = MANUAL_DIR / atlas / f"{name}.png"
            if patch.exists():
                paste_manual(im, patch, box)
                changed += 1
                manual += 1
                continue

            rule = overrides.sprite(atlas, name)
            if rule is not None:
                if rule.get("skip"):
                    continue
                if rule.get("mode") == "white_swap":
                    # Swap the white in every pixel for the level's colour: pastel gradients
                    # (e.g. rainbow skill plates) become deep versions of themselves instead of
                    # half-darkened mud.
                    base, scale = resolve_level(rule.get("level", "surface"))
                    region = im[y0:y1, x0:x1]
                    amount = np.ones(region.shape[:2], dtype=np.float32)
                    region[..., :3] = replace_white(region[..., :3], amount, base + 255.0 * scale)
                    changed += 1
                    continue
                if rule.get("mode") == "card":
                    region = im[y0:y1, x0:x1].copy()
                    if darken_card(region, rule):
                        im[y0:y1, x0:x1] = region
                        changed += 1
                    else:
                        print(f"  {atlas}/{name}: card panel not found (set panel_start)")
                    continue
                level = resolve_level(rule.get("level", "surface"))
            else:
                if not fallback_includes(name):
                    continue
                result = apply_fallback(im[y0:y1, x0:x1])
                if result in ("art", "nocard"):
                    skipped.append(name)
                else:
                    changed += 1
                continue
            region = im[y0:y1, x0:x1]
            region[..., :3] = darken(region[..., :3], level)
            if rule is not None and rule.get("light_glyphs"):
                region[..., :3] = lighten_glyphs(region[..., :3], region[..., 3])
            changed += 1
        if skipped:
            print(f"  {atlas}: kept as artwork (add an override to change): {', '.join(skipped)}")
        if not changed:
            continue

        dst = out / kind / f"{atlas}.png"
        dst.parent.mkdir(parents=True, exist_ok=True)
        Image.fromarray(im).save(dst, optimize=False, compress_level=1)
        print(f"{atlas}: changed {changed}/{len(sprites)} sprites ({manual} manual) -> {dst}")

    for src in sorted((data / "dump" / "textures").glob("*.png")):
        if any(kind == "textures" and name == src.stem for kind, name in atlases):
            continue  # loose atlas texture with sprite rects; handled above
        dst = out / "textures" / src.name
        manual = MANUAL_DIR / "textures" / src.name
        if manual.exists():
            dst.parent.mkdir(parents=True, exist_ok=True)
            Image.open(manual).save(dst)
            print(f"{src.stem}: manual replacement -> {dst}")
            continue

        rule = overrides.texture(src.stem) or {}
        mode = rule.get("mode")
        if mode is None:
            if CARD_TEXTURES.search(src.stem):
                mode = "card"
            elif REGION_TEXTURES.search(src.stem):
                mode = "regions"
            else:
                mode = "sprite"
        if mode == "skip":
            continue
        im = np.array(Image.open(src).convert("RGBA"))
        if mode == "sprite":
            # Single-image texture (e.g. utx_frm_whiteboard_00_sl): treat the whole texture as one
            # sprite named after it, through the same rules as atlas sprites.
            srule = overrides.sprite("textures", src.stem)
            if srule is not None:
                if srule.get("skip"):
                    continue
                im[..., :3] = darken(im[..., :3], resolve_level(srule.get("level", "surface")))
                regions = "whole texture"
            else:
                if not fallback_includes(src.stem) or apply_fallback(im) in ("art", "nocard"):
                    continue
                regions = "whole texture (fallback)"
        elif mode == "card":
            if not darken_card(im, rule):
                print(f"{src.stem}: no label panel found, skipped (set panel_start in overrides)")
                continue
            regions = "card panel"
        elif mode == "regions":
            regions = darken_large_white_regions(im)
            if not regions:
                continue
        else:
            print(f"{src.stem}: unknown mode {mode!r}")
            continue
        dst.parent.mkdir(parents=True, exist_ok=True)
        Image.fromarray(im).save(dst, optimize=False, compress_level=1)
        print(f"{src.stem}: darkened {regions} -> {dst}")


if __name__ == "__main__":
    main()
