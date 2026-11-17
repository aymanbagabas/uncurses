"""Replay kitty_stream_order --dump with Kitty's actual parser and graphics layers."""

from pathlib import Path
import runpy
import sys

helpers = runpy.run_path(str(Path(__file__).with_name("placements_native.py")))
Screen, feed, require = (helpers[name] for name in ("Screen", "feed", "require"))

records = [line.rstrip("\n").split("\t") for line in sys.stdin]
require([name for name, _ in records] == ["image", "text", "deleted"],
        "unexpected experiment stages")
screen = Screen(None, 24, 80, 100, 8, 16)
panels = [(4, 5, 0), (42, 5, 0), (4, 14, -1), (42, 14, -1073741825)]
original_layers = None
for stage, (name, encoded) in enumerate(records):
    payload = bytes.fromhex(encoded)
    if stage:
        require(b"a=p," not in payload and b"a=t," not in payload,
                (name, "image must not be replayed"))
    if stage == 2:
        require(b"BEFORE IMAGE" not in payload and b"AFTER IMAGE" not in payload,
                "deletion must not rewrite the text")
    feed(screen, payload)
    for x, y, _ in panels:
        for row, text in [(y + 1, "BEFORE IMAGE"), (y + 2, "AFTER IMAGE")]:
            actual = "".join(
                screen.line(row)[column] for column in range(x + 7, x + 7 + len(text))
            ).replace("\0", " ")
            expected = " " * len(text) if stage == 0 and row == y + 2 else text
            require(actual == expected, (name, x, row, actual, expected))
            if expected == text:
                style = screen.line(row).cursor_from(x + 7)
                require(style.fg == 0xffffff02 and style.bg == 0x00000002,
                        (name, "text must be white with an explicit black background"))
    screen.update_only_line_graphics_data()
    layers = screen.grman.update_layers(0, -1, 1, 2 / 80, 2 / 24, 80, 24, 8, 16)
    actual_layers = []
    for layer in layers:
        rect = layer["dest_rect"]
        x = round((rect["left"] + 1) * 40)
        y = round((1 - rect["top"]) * 12)
        width = round((rect["right"] - rect["left"]) * 40)
        height = round((rect["top"] - rect["bottom"]) * 12)
        actual_layers.append((x, y, width, height, layer["z_index"]))
    expected_layers = [] if stage == 2 else [(x, y, 30, 4, z) for x, y, z in panels]
    require(sorted(actual_layers) == sorted(expected_layers),
            (name, "incorrect placement or z-order", actual_layers))
    require(screen.grman.image_count == (0 if stage == 2 else 1),
            (name, "incorrect image lifetime"))
    if stage == 0:
        original_layers = actual_layers
    elif stage == 1:
        require(actual_layers == original_layers, "later text changed the image layers")
    require((screen.cursor.x, screen.cursor.y) == (0, 0), "draw must restore the cursor")

print("Verified: later text preserves image layers; image deletion reveals both text writes.")
