#!/usr/bin/env python3
"""Generates the Aceris icon (an original acerola drawing) in every format the
project needs.

Requirements: python3, rsvg-convert (librsvg) and ImageMagick's `convert`.

Usage (from the repository root):
    python3 tools/generate_icons.py

Outputs:
    assets/icon/aceris.svg              full-colour master (rounded square)
    assets/icon/aceris-foreground.svg   transparent adaptive-icon foreground
    assets/icon/png/aceris-<size>.png   desktop / store sizes
    assets/icon/aceris.ico              Windows icon (multi-size)
    frontends/android/res/...           launcher icons for every density
"""

import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
ICON_DIR = ROOT / "assets" / "icon"
ANDROID_RES = ROOT / "frontends" / "android" / "res"

BACKGROUND_COLOR = "#2A1240"  # solid colour used by the Android adaptive icon

# Geometry of the fruit, in a 1024x1024 canvas. The fruit is centred on
# (FRUIT_CX, FRUIT_CY) and scaled around the canvas centre.
FRUIT_CX, FRUIT_CY = 520, 562

DEFS = """
  <defs>
    <linearGradient id="bg" x1="0" y1="0" x2="1" y2="1">
      <stop offset="0" stop-color="#4A1D5E"/>
      <stop offset="1" stop-color="#1A0B2E"/>
    </linearGradient>
    <radialGradient id="glow" cx="512" cy="560" r="430" gradientUnits="userSpaceOnUse">
      <stop offset="0" stop-color="#FF4D6D" stop-opacity="0.38"/>
      <stop offset="1" stop-color="#FF4D6D" stop-opacity="0"/>
    </radialGradient>
    <radialGradient id="fruit" cx="430" cy="500" r="470" gradientUnits="userSpaceOnUse">
      <stop offset="0" stop-color="#FF6B6B"/>
      <stop offset="0.45" stop-color="#E5173F"/>
      <stop offset="1" stop-color="#8E0A2B"/>
    </radialGradient>
    <linearGradient id="leaf" x1="0" y1="1" x2="1" y2="0">
      <stop offset="0" stop-color="#2E8B3E"/>
      <stop offset="1" stop-color="#86D95F"/>
    </linearGradient>
  </defs>
"""

FRUIT = """
    <ellipse cx="512" cy="905" rx="215" ry="26" fill="#000" fill-opacity="0.28"/>
    <g fill="url(#fruit)">
      <circle cx="445" cy="590" r="215"/>
      <circle cx="580" cy="590" r="215"/>
      <circle cx="512" cy="680" r="215"/>
    </g>
    <g fill="none" stroke="#7A0A26" stroke-opacity="0.35" stroke-width="10" stroke-linecap="round">
      <path d="M512 400 C 500 490, 500 570, 512 640"/>
      <path d="M512 640 C 440 660, 370 700, 312 752"/>
      <path d="M512 640 C 584 660, 654 700, 712 752"/>
    </g>
    <ellipse cx="385" cy="525" rx="68" ry="112" transform="rotate(28 385 525)"
             fill="#fff" fill-opacity="0.36"/>
    <circle cx="468" cy="440" r="17" fill="#fff" fill-opacity="0.5"/>
    <path d="M512 398 C 512 340, 530 292, 575 252" fill="none" stroke="#5E8B2F"
          stroke-width="22" stroke-linecap="round"/>
    <path d="M572 262 C 600 190, 700 170, 800 205 C 785 290, 680 330, 572 262 Z"
          fill="url(#leaf)"/>
    <path d="M582 258 C 650 240, 720 226, 782 210" fill="none" stroke="#1F5E2B"
          stroke-opacity="0.55" stroke-width="7" stroke-linecap="round"/>
    <g fill="#3E8E3A">
      <ellipse cx="512" cy="400" rx="46" ry="18"/>
      <ellipse cx="480" cy="410" rx="30" ry="12" transform="rotate(-20 480 410)"/>
      <ellipse cx="544" cy="410" rx="30" ry="12" transform="rotate(20 544 410)"/>
    </g>
"""


def build_svg(shape: str, scale: float) -> str:
    """shape: 'rounded', 'circle' or 'none' (transparent background)."""
    if shape == "rounded":
        background = '<rect width="1024" height="1024" rx="224" fill="url(#bg)"/>'
    elif shape == "circle":
        background = '<circle cx="512" cy="512" r="512" fill="url(#bg)"/>'
    else:
        background = ""

    glow = "" if shape == "none" else (
        '<circle cx="512" cy="560" r="430" fill="url(#glow)"/>'
    )
    transform = (
        f"translate(512 512) scale({scale}) translate({-FRUIT_CX} {-FRUIT_CY})"
    )
    return f"""<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1024 1024" width="1024" height="1024">
  <title>Aceris</title>{DEFS}
  {background}
  {glow}
  <g transform="{transform}">{FRUIT}  </g>
</svg>
"""


def run(*cmd: str) -> None:
    subprocess.run(cmd, check=True)


def rasterize(svg: Path, size: int, out: Path) -> None:
    out.parent.mkdir(parents=True, exist_ok=True)
    run("rsvg-convert", "-w", str(size), "-h", str(size), "-o", str(out), str(svg))


def main() -> int:
    for tool in ("rsvg-convert", "convert"):
        if shutil.which(tool) is None:
            print(f"error: `{tool}` not found in PATH", file=sys.stderr)
            return 1

    ICON_DIR.mkdir(parents=True, exist_ok=True)
    scratch = ICON_DIR / ".tmp"
    scratch.mkdir(exist_ok=True)

    master = ICON_DIR / "aceris.svg"
    foreground = ICON_DIR / "aceris-foreground.svg"
    round_svg = scratch / "aceris-round.svg"

    master.write_text(build_svg("rounded", 1.12))
    # The adaptive-icon safe zone is the central 66% of the canvas.
    foreground.write_text(build_svg("none", 0.82))
    round_svg.write_text(build_svg("circle", 1.0))

    # Desktop / store PNGs.
    for size in (16, 32, 48, 64, 128, 256, 512, 1024):
        rasterize(master, size, ICON_DIR / "png" / f"aceris-{size}.png")

    # Windows .ico (PNG-compressed, multi-size).
    ico_sources = [str(ICON_DIR / "png" / f"aceris-{s}.png") for s in (16, 32, 48, 64, 128, 256)]
    run("convert", *ico_sources, str(ICON_DIR / "aceris.ico"))

    # Android launcher icons (legacy + adaptive).
    densities = {"mdpi": 1, "hdpi": 1.5, "xhdpi": 2, "xxhdpi": 3, "xxxhdpi": 4}
    for name, factor in densities.items():
        folder = ANDROID_RES / f"mipmap-{name}"
        rasterize(master, round(48 * factor), folder / "ic_launcher.png")
        rasterize(round_svg, round(48 * factor), folder / "ic_launcher_round.png")
        rasterize(foreground, round(108 * factor), folder / "ic_launcher_foreground.png")

    adaptive = ANDROID_RES / "mipmap-anydpi-v26"
    adaptive.mkdir(parents=True, exist_ok=True)
    for name in ("ic_launcher", "ic_launcher_round"):
        (adaptive / f"{name}.xml").write_text(
            '<?xml version="1.0" encoding="utf-8"?>\n'
            '<adaptive-icon xmlns:android="http://schemas.android.com/apk/res/android">\n'
            '    <background android:drawable="@color/ic_launcher_background"/>\n'
            '    <foreground android:drawable="@mipmap/ic_launcher_foreground"/>\n'
            "</adaptive-icon>\n"
        )

    values = ANDROID_RES / "values"
    values.mkdir(parents=True, exist_ok=True)
    (values / "ic_launcher_background.xml").write_text(
        '<?xml version="1.0" encoding="utf-8"?>\n'
        "<resources>\n"
        f'    <color name="ic_launcher_background">{BACKGROUND_COLOR}</color>\n'
        "</resources>\n"
    )

    shutil.rmtree(scratch)
    print("icons generated")
    return 0


if __name__ == "__main__":
    sys.exit(main())
