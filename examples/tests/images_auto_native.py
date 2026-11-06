"""Replay images_auto --dump with Kitty and check all protocol streams."""

import runpy
import sys

helpers = runpy.run_path("examples/tests/placements_native.py")
Screen, feed, require = (helpers[name] for name in ("Screen", "feed", "require"))
records = [record.rstrip("\n").split("\t") for record in sys.stdin]
protocols = ["KittyPlaceholders", "Kitty", "Iterm2", "Sixel", "Symbols"]
stages = ["shown", "unchanged", "moved", "removed"]
require([name for name, _ in records] == [
    f"{protocol} {stage}" for protocol in protocols for stage in stages
], "unexpected dump stages")

for index, protocol in enumerate(protocols):
    screen = Screen(None, 12, 40, 100, 8, 16)
    for stage, (_, encoded) in zip(stages, records[index * 4:index * 4 + 4]):
        payload = bytes.fromhex(encoded)
        require(b"\0p" not in payload, (protocol, stage, "private metadata escaped"))
        if stage == "unchanged":
            require(not payload, (protocol, "unchanged frame emitted bytes"))
        if protocol in ("KittyPlaceholders", "Kitty"):
            require((b"a=t," in payload) == (stage == "shown"), (protocol, stage, "upload reuse"))
            require((b"d=I," in payload) == (stage == "removed"), (protocol, stage, "resource deletion"))
            feed(screen, payload)
            origin = 18 if stage == "moved" else 2
            if protocol == "Kitty" and stage != "removed":
                feed(screen, (
                    f"\x1b7\x1b[5;{origin + 2}H"
                    "\x1b[38;2;255;255;255;48;2;0;0;0mLabel\x1b8"
                ).encode())
                label = "".join(screen.line(4)[x] for x in range(origin + 1, origin + 6))
                require(label == "Label", (stage, "overlapping text disappeared", label))
                style = screen.line(4).cursor_from(origin + 1)
                require(style.fg == 0xffffff02 and style.bg == 0x00000002,
                        (stage, "overlapping text style changed"))
            screen.update_only_line_graphics_data()
            layers = screen.grman.update_layers(0, -1, 1, 2 / 40, 2 / 12, 40, 12, 8, 16)
            visible = set()
            for layer in layers:
                if protocol == "Kitty":
                    require(layer["z_index"] == -1,
                            (stage, "image must stay below text and above backgrounds", layer["z_index"]))
                rect = layer["dest_rect"]
                left = round((rect["left"] + 1) * 20)
                top = round((1 - rect["top"]) * 6)
                right = round((rect["right"] + 1) * 20)
                bottom = round((1 - rect["bottom"]) * 6)
                visible.update((x, y) for y in range(top, bottom) for x in range(left, right))
            expected = set() if stage == "removed" else {
                (x, y) for y in range(3, 7) for x in range(origin, origin + 12)
            }
            require(visible == expected, (protocol, stage, visible ^ expected))
            require(screen.grman.image_count == (stage != "removed"), (protocol, stage, "resource leak"))
        elif protocol in ("Iterm2", "Sixel"):
            marker = b"\x1b]1337;File=" if protocol == "Iterm2" else b"\x1bP"
            require((marker in payload) == (stage in ("shown", "moved")), (protocol, stage, "raster output"))
        else:
            feed(screen, payload)
            require(screen.grman.image_count == 0, (protocol, stage, "unexpected image resource"))

print("Verified 20 automatic image frames, including Kitty geometry and resource cleanup.")
