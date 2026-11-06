"""Replay images_auto --dump-committed through Kitty's native parser."""

import runpy
import sys

helpers = runpy.run_path("examples/tests/placements_native.py")
Screen, feed, require = (helpers[name] for name in ("Screen", "feed", "require"))
records = [record.rstrip("\n").split("\t") for record in sys.stdin]
stages = ["shown", "committed", "removed", "invalidated", "new image",
          "removed new image", "committed rest"]
require([name for name, _ in records] == [
    f"{protocol} {stage}" for protocol in ("kitty", "placeholders") for stage in stages
], "unexpected committed-image stages")

for index, protocol in enumerate(("kitty", "placeholders")):
    screen = Screen(None, 24, 40, 100, 8, 16)
    for stage, (_, encoded) in zip(stages, records[index * 7:index * 7 + 7]):
        payload = bytes.fromhex(encoded)
        require((b"a=t," in payload) == (stage in ("shown", "new image")),
                (protocol, stage, "unexpected resource upload"))
        require((b"d=I," in payload) == (stage == "removed new image"),
                (protocol, stage, "committed resource was deleted"))
        feed(screen, payload)
        screen.update_only_line_graphics_data()
        layers = screen.grman.update_layers(0, -1, 1, 2 / 40, 2 / 24, 40, 24, 8, 16)
        visible = set()
        for layer in layers:
            rect = layer["dest_rect"]
            left = round((rect["left"] + 1) * 20)
            top = round((1 - rect["top"]) * 12)
            right = round((rect["right"] + 1) * 20)
            bottom = round((1 - rect["bottom"]) * 12)
            visible.update((x, y) for y in range(top, bottom) for x in range(left, right))
        bottom = 5 if protocol == "kitty" or stage in ("shown", "committed") else 3
        expected = {(x, y) for y in range(1, bottom) for x in range(2, 14)}
        if stage == "new image":
            top = 6 if protocol == "kitty" else 4
            expected.update((x, y) for y in range(top, top + 4) for x in range(18, 30))
        require(visible == expected, (protocol, stage, visible ^ expected))
        require(screen.grman.image_count == (2 if stage == "new image" else 1),
                (protocol, stage, "incorrect resource lifetime"))
        if stage == "committed rest":
            require((screen.cursor.x, screen.cursor.y) == (0, 12),
                    (protocol, "incorrect final cursor", screen.cursor.x, screen.cursor.y))
    print(f"Verified seven committed-image frames: {protocol}.")
