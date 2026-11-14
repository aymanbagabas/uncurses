"""Replay images_auto --dump-placeholders and verify each cell's source coordinates."""

import re
import runpy
import sys

helpers = runpy.run_path("examples/tests/placements_native.py")
Screen, feed, require = (helpers[name] for name in ("Screen", "feed", "require"))
records = [record.rstrip("\n").split("\t") for record in sys.stdin]
require([name for name, _ in records] == [
    "shown", "unchanged", "origin covered", "middle covered",
    "second image", "second removed", "restored", "source offset",
    "offset restored", "vertical", "bottom right", "resized", "symbols",
    "placeholders restored", "removed",
], "unexpected placeholder stages")


def image_cells(area, image_id):
    left, top, width, height = area
    return {(left + x, top + y): (image_id, x, y)
            for y in range(height) for x in range(width)}


height = 12
screen = Screen(None, height, 40, 100, 8, 16)
dimensions = {}
background = None
foreground = None
for name, encoded in records:
    payload = bytes.fromhex(encoded)
    if name == "resized":
        height = 10
        screen.resize(height, 40)
    feed(screen, payload)
    uploads = re.findall(rb"\x1b_Ga=t,[^;]*?\bi=(\d+)", payload)
    require(len(uploads) == (name in ("shown", "second image", "resized", "placeholders restored")),
            (name, "unexpected upload"))
    if uploads:
        image_id = screen.grman.image_for_client_id(int(uploads[0]))["internal_id"]
        if name == "second image":
            foreground = image_id
            dimensions[image_id] = (4, 2)
        else:
            background = image_id
            dimensions[image_id] = (12, 4)

    expected = {}
    if name not in ("symbols", "removed"):
        origin = ((2, 5) if name == "vertical" else
                  (28, 8) if name == "bottom right" else
                  (26, 5) if name in ("resized", "placeholders restored") else (2, 3))
        expected = image_cells((*origin, 12, 4), background)
        if name in ("origin covered", "middle covered", "second image", "second removed"):
            for x in (2, 3):
                expected.pop((x, 4), None)
        if name in ("middle covered", "second image", "second removed"):
            for x in (6, 7, 8):
                expected.pop((x, 5), None)
        if name in ("second image", "second removed"):
            for cell in image_cells((8, 4, 4, 2), foreground):
                expected.pop(cell, None)
            if name == "second image":
                expected.update(image_cells((8, 4, 4, 2), foreground))
        if name == "source offset":
            expected[(2, 4)] = (background, 5, 1)

    screen.update_only_line_graphics_data()
    layers = screen.grman.update_layers(0, -1, 1, 2 / 40, 2 / height, 40, height, 8, 16)
    actual = {}
    for layer in layers:
        dest, source = layer["dest_rect"], layer["src_rect"]
        left = round((dest["left"] + 1) * 20)
        right = round((dest["right"] + 1) * 20)
        top = round((1 - dest["top"]) * height / 2)
        bottom = round((1 - dest["bottom"]) * height / 2)
        image_id = layer["image_id"]
        columns, rows = dimensions[image_id]
        for y in range(top, bottom):
            for x in range(left, right):
                require((x, y) not in actual, (name, "overlapping image fragments", x, y))
                sx = round((source["left"] + (x - left) *
                            (source["right"] - source["left"]) / (right - left)) * columns)
                sy = round((source["top"] + (y - top) *
                            (source["bottom"] - source["top"]) / (bottom - top)) * rows)
                actual[(x, y)] = (image_id, sx, sy)
    require(actual == expected, (name, "incorrect image coordinates", actual, expected))
    count = 0 if name in ("symbols", "removed") else 2 if name == "second image" else 1
    require(screen.grman.image_count == count, (name, "incorrect resource count"))
    if name == "unchanged":
        require(not payload, "unchanged frame emitted bytes")
    require((b"d=I," in payload) == (name in ("second removed", "symbols", "removed")),
            (name, "incorrect resource deletion"))

print("Verified 15 compressed placeholder frames, including overlays, source offsets, resize, and cleanup.")
