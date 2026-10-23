"""Replay images_drag or sized_text_drag --dump-dialog through Kitty."""

import re
import sys

from kitty.fast_data_types import Screen


def require(condition, detail):
    if not condition:
        raise AssertionError(detail)


def feed(screen, pending):
    while pending:
        count = screen.test_commit_write_buffer(pending, screen.test_create_write_buffer())
        require(count > 0, "native parser made no progress")
        pending = pending[count:]
        screen.test_parse_written_data()


def cells(rect):
    x, y, width, height = rect
    return {(column, row) for row in range(y, y + height) for column in range(x, x + width)}


records = [record.rstrip("\n").split("\t") for record in sys.stdin]
require([name for name, _ in records] == [
    "initial", "dialog", "dialog unchanged", "dialog move",
    "dialog origin", "dialog close", "remove",
], "unexpected dialog stages")
sized = "--sized-text" in sys.argv
inline = "--inline" in sys.argv
for origin in (0, 8, 12) if inline else (0,):
    height = 24 if inline else 12
    screen = Screen(None, height, 40, 100, 8, 16)
    if inline:
        feed(screen, f"\x1b[{origin + 1};1H".encode())
    image_rect = (2, 3, 12, 3 if sized else 4)
    for name, encoded in records:
        payload = bytes.fromhex(encoded)
        feed(screen, payload)
        require(b"\0p" not in payload, (name, "packed metadata escaped"))
        if name == "dialog unchanged":
            require(not payload, (name, "unchanged frame emitted bytes"))
        dialog = (
            (5, 4, 10, 3) if name in ("dialog", "dialog unchanged") else
            (8, 5, 10, 3) if name == "dialog move" else
            (2, 3, 10, 3) if name == "dialog origin" else None
        )
        if dialog:
            x, y, _, _ = dialog
            text = "".join(screen.line(origin + y)[column] for column in range(x, x + 6))
            require(text == "Dialog", (name, "dialog moved or disappeared", text))
        if sized:
            rows = [y for y in range(12) if "\x1b]66;" in screen.line(origin + y).as_ansi()]
            require(rows == ([3] if name in ("initial", "dialog close") else []),
                    (name, "atomic sized-text visibility", rows))
            if dialog:
                for x, y in cells(image_rect) - cells(dialog):
                    style = screen.line(origin + y).cursor_from(x)
                    require(style.bold and style.fg == 0xffffff02 and style.bg == 0x00000002,
                            (name, "erased sized text lost its style", x, y))
        else:
            require((b"a=t," in payload) == (name == "initial"), (name, "unnecessary upload"))
            screen.update_only_line_graphics_data()
            layers = screen.grman.update_layers(0, -1, 1, 2 / 40, 2 / height, 40, height, 8, 16)
            visible = set()
            for layer in layers:
                rect = layer["dest_rect"]
                left = round((rect["left"] + 1) * 20)
                top = round((1 - rect["top"]) * height / 2) - origin
                right = round((rect["right"] + 1) * 20)
                bottom = round((1 - rect["bottom"]) * height / 2) - origin
                occupied = cells((left, top, right - left, bottom - top))
                require(not visible.intersection(occupied), (name, "overlapping fragments"))
                visible.update(occupied)
            expected = set() if name == "remove" else cells(image_rect)
            if dialog:
                expected -= cells(dialog)
            require(visible == expected, (name, "incorrect visible image cells", visible ^ expected))
            require(screen.grman.image_count == (name != "remove"), (name, "resource leak"))
            for command in re.findall(rb"\x1b_Ga=p,([^\x1b]+)", payload):
                values = dict((key.decode(), int(value)) for key, value in re.findall(rb"(\w+)=(\d+)", command))
                require(values["w"] == values["c"] * 8 and values["h"] == values["r"] * 16,
                        (name, "fragment pixel dimensions", values))
    print(f"Verified seven dialog frames: sized={sized}, inline={inline}, origin={origin}.")
