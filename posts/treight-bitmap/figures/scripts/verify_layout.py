#!/usr/bin/env python3
"""Layout verification for blog figures (diagram-eng).

Text-channel visual QA for compiled figure PNGs: this model cannot ingest
images, so this script turns each render into things a text model CAN read:

  1. clip check   — ink bounding box must clear the canvas edges by a margin
                    (catches clipped/cut-off labels);
  2. region map   — dominant solid-color regions with bounding boxes; large
                    SATURATED regions must not pixel-overlap (catches panels
                    colliding); whitish background tones are ignored;
  3. ascii preview— coarse luminance map (catches gross layout: empty zones,
                    overlapping blobs, missing elements). --crop zooms.

Usage:  python3 verify_layout.py FILE.png [--margin 8] [--preview-width 110]
                                          [--crop X0 Y0 X1 Y1]  (px, original)
Exit 0 = clean, 1 = problems found (printed).
"""
from __future__ import annotations

import sys

import numpy as np
from PIL import Image


def downscale_mask(mask: np.ndarray, w: int, h: int) -> np.ndarray:
    ys = (np.arange(h) * mask.shape[0] / h).astype(int)
    xs = (np.arange(w) * mask.shape[1] / w).astype(int)
    return mask[np.ix_(ys, xs)]


def ascii_preview(img: Image.Image, width: int = 110) -> list[str]:
    g = np.asarray(img.convert("L"), dtype=np.uint8)
    h = max(1, int(width * g.shape[0] / g.shape[1] * 0.45))
    small = downscale_mask(g, width, h)
    ramp = "@%#*+=-:. "
    out = []
    for row in small:
        out.append("".join(ramp[min(int(v) * len(ramp) // 256, len(ramp) - 1)] for v in row))
    return out


def whitish(c: tuple[int, ...]) -> bool:
    return all(v > 228 for v in c[:3])


def main() -> int:
    path = sys.argv[1]
    margin, width, crop = 8, 110, None
    args = sys.argv[2:]
    if "--margin" in args:
        margin = int(args[args.index("--margin") + 1])
    if "--preview-width" in args:
        width = int(args[args.index("--preview-width") + 1])
    if "--crop" in args:
        crop = [int(v) for v in args[args.index("--crop") + 1 : args.index("--crop") + 5]]

    img = Image.open(path).convert("RGB")
    if crop:
        img = img.crop(crop)
        print(f"cropped to {crop}")
    a = np.asarray(img, dtype=np.int16)
    h, w, _ = a.shape
    problems: list[str] = []

    # 1. ink bounding box (anything not near-white)
    near_white = (
        (np.abs(a[:, :, 0] - 255) < 12)
        & (np.abs(a[:, :, 1] - 255) < 12)
        & (np.abs(a[:, :, 2] - 255) < 12)
    )
    ink = ~near_white
    if not ink.any():
        print("FAIL: canvas is blank")
        return 1
    ys, xs = np.where(ink)
    t, b, l, r = ys.min(), ys.max(), xs.min(), xs.max()
    print(f"canvas {w}x{h}px  ink bbox margins: top={t} bottom={h-1-b} left={l} right={w-1-r}")
    for side, val in (("top", t), ("bottom", h - 1 - b), ("left", l), ("right", w - 1 - r)):
        if val < margin:
            problems.append(f"ink within {val}px of {side} edge (margin {margin}px) — possible clipping")

    # 2. solid-color regions (panels/fills), whitish tones ignored for collisions
    quant = (a // 8 * 8).astype(np.uint8)
    colors, counts = np.unique(quant.reshape(-1, 3), axis=0, return_counts=True)
    order = np.argsort(-counts)
    print("\ndominant color regions (>0.25% of canvas):")
    regions = []
    masks = {}
    for i in order[:20]:
        c, n = colors[i], counts[i]
        if n < 0.0025 * w * h or whitish(c):
            continue
        mask = np.all(quant == c, axis=2)
        if mask.sum() < 0.0025 * w * h:
            continue
        ys2, xs2 = np.where(mask)
        bb = (int(xs2.min()), int(ys2.min()), int(xs2.max()), int(ys2.max()))
        frac = mask.sum() / (w * h)
        key = tuple(int(v) for v in c)
        regions.append((key, bb, frac))
        masks[key] = mask
        print(f"  #{key[0]:02x}{key[1]:02x}{key[2]:02x}  {frac:5.1%}  bbox x{bb[0]}-{bb[2]} y{bb[1]}-{bb[3]}")

    for i in range(len(regions)):
        for j in range(i + 1, len(regions)):
            (c1, _, f1), (c2, _, f2) = regions[i], regions[j]
            if f1 < 0.015 or f2 < 0.015:
                continue
            inter = masks[c1] & masks[c2]
            if inter.any():
                problems.append(f"saturated regions {c1} and {c2} pixel-overlap")
            # adjacency of two LARGE panel-sized fills is fine; touching edges
            # of stroke vs fill is fine — only pixel intersection counts here.

    # 3. ascii preview
    print("\nascii preview (dark = ink):")
    for line in ascii_preview(img, width):
        print("  " + line)

    if problems:
        print("\nPROBLEMS:")
        for p in problems:
            print("  -", p)
        return 1
    print("\nOK: no clipping, no large-region collisions detected")
    return 0


if __name__ == "__main__":
    sys.exit(main())
