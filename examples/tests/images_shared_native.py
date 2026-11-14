"""Replay images_auto --dump-shared through Kitty's native parser."""

import re
import runpy
import sys

helpers = runpy.run_path("examples/tests/placements_native.py")
Screen, feed, require = (helpers[name] for name in ("Screen", "feed", "require"))
records = [record.rstrip("\n").split("\t") for record in sys.stdin]
stages = [
    "shared", "unchanged", "overlap", "reordered", "dialog",
    "removed first", "replaced", "removed all",
]
require([name for name, _ in records] == [
    f"{protocol} {stage}" for protocol in ("kitty", "placeholders") for stage in stages
], "unexpected shared-image stages")


def cells(rect):
    x, y, width, height = rect
    return {(column, row) for row in range(y, y + height) for column in range(x, x + width)}


for index, protocol in enumerate(("kitty", "placeholders")):
    screen = Screen(None, 12, 40, 100, 8, 16)
    identities = []
    for stage, (_, encoded) in enumerate(records[index * 8:index * 8 + 8]):
        payload = bytes.fromhex(encoded)
        name = stages[stage]
        uploads = re.findall(rb"\x1b_Ga=t,[^;]*?i=(\d+)", payload)
        require(len(uploads) == int(stage in (0, 2, 6)), (protocol, name, "upload count"))
        require(payload.count(b"d=I,") == int(stage in (5, 6, 7)),
                (protocol, name, "resource deletion count"))
        require(b"\0p" not in payload, (protocol, name, "private marker escaped"))
        if stage == 1:
            require(not payload, (protocol, name, "unchanged frame emitted bytes"))
        feed(screen, payload)
        for client_id in uploads:
            image = screen.grman.image_for_client_id(int(client_id))
            require(image is not None, (protocol, name, "new resource was deleted"))
            identities.append(image["internal_id"])
            if stage in (2, 6):
                color = b"\0\0\xff" if stage == 2 else b"\0\xff\0"
                require(image["data"] == color * (image["width"] * image["height"]),
                        (protocol, name, "replacement pixels differ"))
        expected = {}
        if stage < 7:
            for placement in ([1, 2] if stage < 3 else [2, 1]):
                if placement == 1 and stage >= 5:
                    continue
                area = (2, 2, 8, 4) if placement == 1 else (
                    (14, 2, 8, 4) if stage < 2 else (6, 3, 8, 4)
                )
                identity = identities[
                    0 if placement == 1 or stage < 2 else (1 if stage < 6 else 2)
                ]
                expected.update(dict.fromkeys(cells(area), identity))
            if stage == 4:
                for cell in cells((4, 3, 6, 2)):
                    expected.pop(cell, None)
        screen.update_only_line_graphics_data()
        layers = screen.grman.update_layers(0, -1, 1, 2 / 40, 2 / 12, 40, 12, 8, 16)
        actual = {}
        for layer in layers:
            rect = layer["dest_rect"]
            left = round((rect["left"] + 1) * 20)
            top = round((1 - rect["top"]) * 6)
            right = round((rect["right"] + 1) * 20)
            bottom = round((1 - rect["bottom"]) * 6)
            occupied = cells((left, top, right - left, bottom - top))
            require(not occupied.intersection(actual), (protocol, name, "overlapping image fragments"))
            actual.update(dict.fromkeys(occupied, layer["image_id"]))
        require(actual == expected, (protocol, name, "incorrect cell ownership", actual, expected))
        require(screen.grman.image_count == [1, 1, 2, 2, 2, 1, 1, 0][stage],
                (protocol, name, "resource lifetime"))
    print(f"Verified eight shared-image frames: {protocol}, including overlap, order, and replacement pixels.")
