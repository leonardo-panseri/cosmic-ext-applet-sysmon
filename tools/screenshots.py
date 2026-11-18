#!/usr/bin/env python3
"""Regenerate the screenshots the metainfo points at.

Builds the applet in debug mode and runs its preview (src/preview.rs) once per
screenshot below. The preview draws a mock top panel and the popup over a
backdrop in COSMIC's default dark theme, from fixed made-up stats (nothing is
read from the system, so no GPU is woken), saves its own window's pixels and
exits; this script writes them out as a PNG.

Needs cargo, a running Wayland session and Python 3.11+ (stdlib only). Usage:

    python3 tools/screenshots.py            # all of them
    python3 tools/screenshots.py popup      # only resources/screenshots/popup.png
"""

import argparse
import os
import struct
import subprocess
import sys
import tempfile
import zlib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
BINARY = ROOT / "target" / "debug" / "cosmic-ext-applet-sysmon"
OUT_DIR = ROOT / "resources" / "screenshots"
# The preview waits 1.5 s before its screenshot; this is the limit for the whole run.
TIMEOUT = 60
# libcosmic outlines the window, even with its decorations off; this many pixels are cut off
# each side.
BORDER = 1

# name -> the preview's environment. SYSMON_PREVIEW_HEIGHT stands for the screen's height, which
# the Auto popup layout measures: too short for one column, so these show two.
SHOTS = {
    # The popup as the panel shows it, in two columns: every section without scrolling.
    "popup": {"SYSMON_PREVIEW_HEIGHT": "920"},
    # The settings page, in the same two-column popup.
    "settings": {"SYSMON_PREVIEW_SETTINGS": "1", "SYSMON_PREVIEW_HEIGHT": "620"},
}


def capture(env: dict[str, str]) -> tuple[int, int, bytes]:
    """Runs the preview and returns the width, height and RGBA pixels it saved."""
    with tempfile.TemporaryDirectory() as tmp:
        raw = Path(tmp) / "shot.rgba"
        run_env = {**os.environ, "SYSMON_PREVIEW": "1", **env,
                   "SYSMON_PREVIEW_SCREENSHOT": str(raw)}
        subprocess.run([BINARY], cwd=ROOT, env=run_env, check=True, timeout=TIMEOUT)
        data = raw.read_bytes()
    if data[:4] != b"RGBA":
        raise ValueError("the preview didn't save a screenshot")
    width, height = struct.unpack("<II", data[4:12])
    pixels = data[12:]
    if len(pixels) != width * height * 4:
        raise ValueError(f"expected {width}x{height} pixels, got {len(pixels)} bytes")
    return width, height, pixels


def crop(width: int, height: int, rgba: bytes, margin: int) -> tuple[int, int, bytes]:
    """Cuts `margin` pixels off each side."""
    stride = width * 4
    rows = (rgba[y * stride + margin * 4:(y + 1) * stride - margin * 4]
            for y in range(margin, height - margin))
    return width - 2 * margin, height - 2 * margin, b"".join(rows)


def png(width: int, height: int, rgba: bytes) -> bytes:
    """An 8-bit RGBA PNG, every row unfiltered."""
    def chunk(kind: bytes, body: bytes) -> bytes:
        crc = zlib.crc32(kind + body)
        return struct.pack(">I", len(body)) + kind + body + struct.pack(">I", crc)

    stride = width * 4
    rows = b"".join(b"\0" + rgba[y * stride:(y + 1) * stride] for y in range(height))
    header = struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0)
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", header)
            + chunk(b"IDAT", zlib.compress(rows, 9)) + chunk(b"IEND", b""))


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("names", nargs="*", metavar="name",
                        help=f"screenshots to take: {', '.join(SHOTS)} (default: all)")
    names = parser.parse_args().names or list(SHOTS)
    unknown = [name for name in names if name not in SHOTS]
    if unknown:
        parser.error(f"unknown screenshot {', '.join(unknown)}; choose from {', '.join(SHOTS)}")

    if not os.environ.get("WAYLAND_DISPLAY"):
        print("needs a Wayland session to open the preview window", file=sys.stderr)
        return 2
    # The preview is compiled into debug builds only.
    subprocess.run(["cargo", "build"], cwd=ROOT, check=True)
    OUT_DIR.mkdir(parents=True, exist_ok=True)
    for name in names:
        width, height, pixels = crop(*capture(SHOTS[name]), BORDER)
        out = OUT_DIR / f"{name}.png"
        out.write_bytes(png(width, height, pixels))
        print(f"wrote {out.relative_to(ROOT)} ({width}x{height})")
    return 0


if __name__ == "__main__":
    sys.exit(main())
