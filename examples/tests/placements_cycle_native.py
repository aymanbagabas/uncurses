"""Check Kitty resource lifetimes across interactive image transitions.

    target/debug/examples/images_drag --dump-keys p,p,p,p,p,s,f,r,d,k,c |
      kitty +runpy 'import runpy; runpy.run_path(
        "examples/tests/placements_cycle_native.py", run_name="__main__")'

This checks native Kitty and placeholders. Other protocols must retire Kitty
resources, but their graphical output needs their own supporting terminal.
"""

from pathlib import Path
import re
import runpy
import sys

helpers = runpy.run_path(str(Path(__file__).with_name("placements_native.py")))
Screen, feed, require = (helpers[name] for name in ("Screen", "feed", "require"))


def verify(records, inline, start):
    height = 24 if inline else 12
    screen = Screen(None, height, 40, 100, 8, 16)
    if inline:
        feed(screen, "".join(f"history-{row}\r\n" for row in range(start)).encode())
        feed(screen, f"\x1b[{start + 1};1H".encode())
    origin = min(start, height - 12)
    for name, payload in records:
        feed(screen, payload)
        match = re.match(r"Images: (\w+)", str(screen.line(origin)))
        require(match is not None, (name, "missing protocol label", str(screen.line(origin))))
        mode = match.group(1)
        if name == "unchanged":
            require(not payload, (mode, "unchanged frame produced output"))
        screen.update_only_line_graphics_data()
        layers = screen.grman.update_layers(
            0, -1, 1, 2 / 40, 2 / height, 40, height, 8, 16
        )
        visible = mode in ("kitty", "placeholders") and name != "remove"
        require(screen.grman.image_count == int(visible),
                (name, mode, "resource count", screen.grman.image_count))
        expected = (
            [(2, 3 + row, 12, 1) for row in range(4)] if mode == "placeholders"
            else [(2, 3, 12, 4)]
        ) if visible else []
        rectangles = []
        for layer in layers:
            rect = layer["dest_rect"]
            left = round((rect["left"] + 1) * 20)
            top = round((1 - rect["top"]) * height / 2)
            right = round((rect["right"] + 1) * 20)
            bottom = round((1 - rect["bottom"]) * height / 2)
            rectangles.append((left, top - origin, right - left, bottom - top))
        require(sorted(rectangles) == expected, (name, mode, rectangles, expected))
        if mode != "placeholders" or name == "remove":
            require(all(
                "\U0010eeee" not in str(screen.line(origin + row))
                for row in range(2, 11)
            ), (name, "old placeholder cells remain"))
    require(records[-1][0] == "remove", "missing cleanup frame")
    print(f"Verified {len(records)} cycling frames, inline={inline}, origin={origin}.")


if __name__ == "__main__":
    records = [
        (name, bytes.fromhex(payload))
        for name, payload in (line.rstrip("\n").split("\t") for line in sys.stdin)
    ]
    inline = "--inline" in sys.argv
    for start in (0, 8, 23) if inline else (0,):
        verify(records, inline, start)
