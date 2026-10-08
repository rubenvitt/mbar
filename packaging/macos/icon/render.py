#!/usr/bin/env python3
"""Render AppIcon.svg into packaging/macos/AppIcon.icns (and a preview PNG).

Run after editing the SVG and commit both files; the app build only copies
the .icns. Needs `pip install cairosvg pillow` (cairosvg needs libcairo).
"""
import io
import pathlib
import sys

import cairosvg
from PIL import Image, ImageFilter

HERE = pathlib.Path(__file__).resolve().parent
SVG = HERE / "AppIcon.svg"
ICNS = HERE.parent / "AppIcon.icns"
PREVIEW = HERE / "AppIcon.png"
SIZES = [16, 32, 64, 128, 256, 512, 1024]


def render(size: int) -> Image.Image:
    # Draw at 1024 so the shadow geometry matches the macOS icon template,
    # then scale down; small sizes come out crisper than rasterising the SVG
    # straight at 16 px.
    png = cairosvg.svg2png(url=str(SVG), output_width=1024, output_height=1024)
    art = Image.open(io.BytesIO(png)).convert("RGBA")
    alpha = art.getchannel("A")
    shadow = Image.new("RGBA", art.size, (0, 0, 0, 0))
    shadow.putalpha(alpha.point(lambda a: a * 0.35))
    shadow = shadow.transform(art.size, Image.AFFINE, (1, 0, 0, 0, 1, -12))
    shadow = shadow.filter(ImageFilter.GaussianBlur(14))
    out = Image.alpha_composite(shadow, art)
    return out if size == 1024 else out.resize((size, size), Image.LANCZOS)


def main() -> int:
    images = [render(s) for s in SIZES]
    big = images[-1]
    big.save(ICNS, append_images=images[:-1])
    images[SIZES.index(512)].save(PREVIEW)
    print(f"wrote {ICNS.relative_to(HERE.parents[2])} and {PREVIEW.name}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
