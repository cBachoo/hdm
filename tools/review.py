"""Renders before/after review sheets for every sprite in each atlas.

usage: python review.py <data dir> <built dir> <sheet dir> [atlas ...]

For each atlas (or only the ones given), writes `<sheet dir>/<atlas>.png`: a grid with one cell per
sprite showing the original and the built result side by side on a mid-grey background, labelled
with the sprite name and what the build did (override level, skip, manual, fallback, untouched).
Use it to decide which entries to add to overrides.json or tools/manual/.
"""
import sys
from pathlib import Path

import numpy as np
from PIL import Image, ImageDraw, ImageFont

sys.path.insert(0, str(Path(__file__).resolve().parent))
import build_dark_atlases as b  # noqa: E402

THUMB = 150
LABEL_H = 30
BG = (60, 62, 70, 255)


def describe(overrides, atlas, name, region):
    if (b.MANUAL_DIR / atlas / f"{name}.png").exists():
        return "manual"
    rule = overrides.sprite(atlas, name)
    if rule is not None:
        if rule.get("skip"):
            return "skip (override)"
        level = rule.get("level", "surface")
        return f"{level if isinstance(level, str) else 'custom'} (override)"
    if not b.fallback_includes(name):
        return "untouched"
    if b.looks_like_art(region):
        return "kept: artwork?"
    return "surface (auto)"


def thumb(region):
    im = Image.fromarray(region)
    im.thumbnail((THUMB, THUMB))
    cell = Image.new("RGBA", (THUMB, THUMB), BG)
    cell.alpha_composite(im, ((THUMB - im.width) // 2, (THUMB - im.height) // 2))
    return cell


def main():
    data, built, sheets = Path(sys.argv[1]), Path(sys.argv[2]), Path(sys.argv[3])
    only = set(sys.argv[4:])
    overrides = b.Overrides(b.OVERRIDES_PATH)
    font = ImageFont.load_default()
    sheets.mkdir(parents=True, exist_ok=True)

    for (kind, atlas), sprites in sorted(b.parse_assets(data / "loaded_assets.txt").items()):
        if only and atlas not in only:
            continue
        src = data / "dump" / kind / f"{atlas}.png"
        if not src.exists():
            continue
        dst = built / kind / f"{atlas}.png"
        before = np.array(Image.open(src).convert("RGBA"))
        after = np.array(Image.open(dst).convert("RGBA")) if dst.exists() else before

        cols = 5
        cell_w, cell_h = THUMB * 2 + 10, THUMB + LABEL_H
        rows = (len(sprites) + cols - 1) // cols
        sheet = Image.new("RGBA", (cols * (cell_w + 10), rows * (cell_h + 10)), (25, 26, 30, 255))
        draw = ImageDraw.Draw(sheet)
        for i, (name, x, y, w, h) in enumerate(sorted(sprites)):
            x0, y0, x1, y1 = b.sprite_box(before.shape[0], x, y, w, h)
            cx, cy = (i % cols) * (cell_w + 10), (i // cols) * (cell_h + 10)
            sheet.alpha_composite(thumb(before[y0:y1, x0:x1]), (cx, cy))
            sheet.alpha_composite(thumb(after[y0:y1, x0:x1]), (cx + THUMB + 10, cy))
            draw.text((cx, cy + THUMB + 2), name, fill=(230, 230, 230), font=font)
            draw.text((cx, cy + THUMB + 15), describe(overrides, atlas, name, before[y0:y1, x0:x1]),
                      fill=(150, 200, 255), font=font)
        out = sheets / f"{atlas}.png"
        sheet.convert("RGB").save(out)
        print(f"{atlas}: {len(sprites)} sprites -> {out}")


if __name__ == "__main__":
    main()
