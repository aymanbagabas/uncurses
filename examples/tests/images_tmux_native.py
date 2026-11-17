"""Replay raw tmux output from images_auto in Kitty's native parser.

Pass five raw captures in order: shown, moved, removed, restored, exited.
Capture the outer tmux client with passthrough enabled and an 80x24 terminal.
Use 8x16 cell pixels, true color, and colored underlines.
"""

from pathlib import Path
import re
import runpy
import sys

helpers = runpy.run_path("examples/tests/placements_native.py")
Screen, feed, require = (helpers[name] for name in ("Screen", "feed", "require"))
require(len(sys.argv) == 6, "expected shown, moved, removed, restored, and exited captures")

for path, stage, origin, uploads, deletions in zip(
    sys.argv[1:],
    ("shown", "moved", "removed", "restored", "exited"),
    ((2, 3), (18, 3), None, (2, 3), None),
    (1, 1, 1, 2, 2),
    (0, 0, 1, 1, 2),
):
    payload = Path(path).read_bytes()
    require(b"\x1bPtmux;" not in payload, (stage, "tmux did not unwrap passthrough"))
    require(b"\x1b_Ga=q," in payload, (stage, "graphics query was not forwarded"))
    require(b"\x1b[>q" in payload, (stage, "identity query was not forwarded"))
    require(payload.count(b"\x1b_Ga=t,") == uploads, (stage, "unexpected upload count"))
    require(payload.count(b"\x1b_Ga=d,d=I,") == deletions, (stage, "unexpected deletion count"))
    for match in re.finditer(rb"\x1b_G[^;]*;([^\x1b]*)\x1b\\", payload):
        require(len(match[1]) <= 4096, (stage, "upload chunk exceeded the protocol limit"))

    screen = Screen(None, 24, 80, 100, 8, 16)
    feed(screen, payload)
    require(screen.grman.image_count == int(origin is not None), (stage, "resource lifetime"))
    screen.update_only_line_graphics_data()
    layers = screen.grman.update_layers(0, -1, 1, 2 / 80, 2 / 24, 80, 24, 8, 16)
    actual = {}
    for layer in layers:
        dest, source = layer["dest_rect"], layer["src_rect"]
        left = round((dest["left"] + 1) * 40)
        right = round((dest["right"] + 1) * 40)
        top = round((1 - dest["top"]) * 12)
        bottom = round((1 - dest["bottom"]) * 12)
        for y in range(top, bottom):
            for x in range(left, right):
                require((x, y) not in actual, (stage, "overlapping fragments"))
                sx = round((source["left"] + (x - left) *
                            (source["right"] - source["left"]) / (right - left)) * 12)
                sy = round((source["top"] + (y - top) *
                            (source["bottom"] - source["top"]) / (bottom - top)) * 4)
                actual[(x, y)] = (sx, sy)
    expected = {} if origin is None else {
        (origin[0] + x, origin[1] + y): (x, y) for y in range(4) for x in range(12)
    }
    require(actual == expected, (stage, "incorrect image cells", actual, expected))

print("Verified tmux forwarding, placeholder coordinates, movement, and resource cleanup.")
