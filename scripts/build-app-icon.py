from __future__ import annotations

import argparse
from pathlib import Path

from PIL import Image


ICON_SIZES = (16, 20, 24, 32, 40, 48, 64, 128, 256)


def square_crop_with_padding(image: Image.Image, padding_ratio: float) -> Image.Image:
    rgba = image.convert("RGBA")
    alpha_bounds = rgba.getchannel("A").getbbox()
    if alpha_bounds is None:
        raise ValueError("The source image has no visible pixels.")

    left, top, right, bottom = alpha_bounds
    width = right - left
    height = bottom - top
    side = max(width, height)
    padding = max(1, round(side * padding_ratio))
    side += padding * 2

    center_x = (left + right) / 2
    center_y = (top + bottom) / 2
    crop_left = round(center_x - side / 2)
    crop_top = round(center_y - side / 2)
    crop_right = crop_left + side
    crop_bottom = crop_top + side

    canvas = Image.new("RGBA", (side, side), (0, 0, 0, 0))
    source_left = max(0, crop_left)
    source_top = max(0, crop_top)
    source_right = min(rgba.width, crop_right)
    source_bottom = min(rgba.height, crop_bottom)
    destination = (source_left - crop_left, source_top - crop_top)
    canvas.alpha_composite(
        rgba.crop((source_left, source_top, source_right, source_bottom)),
        destination,
    )
    return canvas


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Build a multi-resolution Windows ICO from an alpha PNG."
    )
    parser.add_argument("--input", required=True, type=Path)
    parser.add_argument("--ico", required=True, type=Path)
    parser.add_argument("--preview", required=True, type=Path)
    parser.add_argument("--padding", type=float, default=0.025)
    args = parser.parse_args()

    source = Image.open(args.input)
    icon_source = square_crop_with_padding(source, args.padding)

    args.ico.parent.mkdir(parents=True, exist_ok=True)
    args.preview.parent.mkdir(parents=True, exist_ok=True)
    icon_source.resize((512, 512), Image.Resampling.LANCZOS).save(args.preview)
    icon_source.save(
        args.ico,
        format="ICO",
        sizes=[(size, size) for size in ICON_SIZES],
        bitmap_format="png",
    )

    print(f"ICO: {args.ico}")
    print(f"Preview: {args.preview}")
    print("Sizes: " + ", ".join(str(size) for size in ICON_SIZES))


if __name__ == "__main__":
    main()
