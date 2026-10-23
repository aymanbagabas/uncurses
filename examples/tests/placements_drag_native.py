"""Replay images_drag or sized_text_drag dumps through Kitty's native parser.

Example:
    target/debug/examples/images_drag --placeholders --dump |
      kitty +runpy 'import runpy; runpy.run_path(
        "examples/tests/placements_drag_native.py", run_name="__main__")' --placeholders
"""

from pathlib import Path
import runpy
import sys

helpers = runpy.run_path(str(Path(__file__).with_name("placements_native.py")))
Screen, feed, require = (helpers[name] for name in ("Screen", "feed", "require"))


def verify(records, placeholders, images, inline, start):
    height = 24 if inline else 12
    screen = Screen(None, height, 40, 100, 8, 16)
    if inline:
        feed(screen, "".join(f"history-{row}\r\n" for row in range(start)).encode())
        feed(screen, f"\x1b[{start + 1};8H".encode())
    origin = min(start, height - 12)
    stages = ["initial", "unchanged", "move", "resize-small", "resize-large", "remove"]
    require([name for name, _ in records] == stages, "unexpected drag stage sequence")
    for name, payload in records:
        managed_height = 3 if name == "resize-small" else 12
        if not inline and name in ("resize-small", "resize-large"):
            height = managed_height
            screen.resize(height, 40)
        feed(screen, payload)
        if name == "unchanged":
            require(not payload, ("unchanged frame produced bytes", payload))
        visible = name not in ("resize-small", "remove")
        position = (2, 3) if name in ("initial", "unchanged") else (20, 5)
        if images:
            screen.update_only_line_graphics_data()
            layers = screen.grman.update_layers(
                0, -1, 1, 2 / 40, 2 / height, 40, height, 8, 16
            )
            rectangles = []
            for layer in layers:
                rect = layer["dest_rect"]
                left = round((rect["left"] + 1) * 20)
                top = round((1 - rect["top"]) * height / 2) - origin
                right = round((rect["right"] + 1) * 20)
                bottom = round((1 - rect["bottom"]) * height / 2) - origin
                rectangles.append((left, top, right - left, bottom - top))
            x, y = position
            expected = [] if not visible else (
                [(x, y + row, 12, 1) for row in range(4)]
                if placeholders else [(x, y, 12, 4)]
            )
            require(sorted(rectangles) == expected, (name, rectangles, expected))
            require(screen.grman.image_count == int(visible),
                    (name, "resource count", screen.grman.image_count))
            if name == "move":
                require(b"a=t" not in payload, "movement uploaded the image again")
            if placeholders and visible:
                for row in range(4):
                    for col in range(12):
                        require(screen.line(origin + y + row)[x + col].startswith("\U0010eeee"),
                                (name, "missing placeholder", col, row))
        else:
            rows = [
                row for row in range(managed_height)
                if "\x1b]66;" in screen.line(origin + row).as_ansi()
            ]
            require(rows == ([position[1]] if visible else []), (name, "sized rows", rows))
            if visible:
                x, y = position
                require("Drag" in screen.line(origin + y).as_ansi(), "missing sized text")
                style = screen.line(origin + y).cursor_from(x)
                require(style.bold, "sized text lost bold")
                require(style.fg == 0xffffff02 and style.bg == 0x00000002,
                        (name, "sized text must use white on black", style.fg, style.bg))

        if name in ("move", "resize-large", "remove"):
            restored = "".join(screen.line(origin + 3)[col] for col in range(2, 14))
            expected = ("row 03 " + "." * 40)[2:14]
            require(restored == expected, (name, "background not restored", restored))
        require((screen.cursor.x, screen.cursor.y) == (0, origin + managed_height - 1),
                (name, "cursor", screen.cursor.x, screen.cursor.y))
    print(f"Verified {len(records)} drag frames, inline={inline}, origin={origin}.")


if __name__ == "__main__":
    records = []
    for record in sys.stdin:
        name, payload = record.rstrip("\n").split("\t")
        records.append((name, bytes.fromhex(payload)))
    placeholders = "--placeholders" in sys.argv
    inline = "--inline" in sys.argv
    for start in (0, 8, 23) if inline else (0,):
        verify(records, placeholders, placeholders or "--kitty" in sys.argv, inline, start)
