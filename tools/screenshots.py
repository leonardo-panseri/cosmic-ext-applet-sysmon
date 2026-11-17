#!/usr/bin/env python3
"""Regenerate the screenshots the metainfo points at.

A stub until P6 (docs/implementation-plan.md §5). The plan is the same as in
cosmic-ext-applet-codexbar: a debug-only preview mode in the applet draws a mock
panel and the popup from made-up values, saves its own window's pixels as raw
RGBA and exits, and this script turns them into resources/screenshots/<name>.png.

Needs cargo, a running Wayland session and Python 3.11+ (stdlib only). Usage:

    python3 tools/screenshots.py            # all of them
    python3 tools/screenshots.py popup      # only resources/screenshots/popup.png
"""

import argparse
import sys

# name -> the preview's environment. Filled in once the preview exists.
SHOTS: dict[str, dict[str, str]] = {}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("names", nargs="*", metavar="name",
                        help="screenshots to take (default: all)")
    parser.parse_args()
    print("no screenshots yet: the preview mode arrives in P6", file=sys.stderr)
    return 1


if __name__ == "__main__":
    sys.exit(main())
