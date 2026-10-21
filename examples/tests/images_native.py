"""Check `images --kitty --dump` with Kitty's native graphics parser.

Add --inline to this script when the example also uses --inline.
Run with kitty +runpy and runpy.run_path, as in sized_text_native.py.
"""

import re
import sys

from kitty.fast_data_types import Screen


def require(condition, detail):
    if not condition:
        raise AssertionError(detail)


def feed(screen, data):
    while data:
        count = screen.test_commit_write_buffer(data, screen.test_create_write_buffer())
        require(count > 0, "native parser made no progress")
        data = data[count:]
        screen.test_parse_written_data()


def verify(records, inline=False):
    screen = Screen(None, 24 if inline else 12, 40, 100, 8, 16)
    origin = 3 if inline else 0
    if inline:
        feed(screen, b"history-0\r\nhistory-1\r\nhistory-2\r\n")
        feed(screen, b"\x1b[1;39H\x1b_Ga=T,f=24,s=1,v=1,i=1,p=1,C=1,q=2;/wD/\x1b\\\x1b[4;1H")
    image_id = None
    count = 0
    empty = {"remove", "handoff", "cleanup", "placeholder cleanup"}
    for name, data in records:
        count += 1
        if name in {"replay", "placeholder replay", "attached replay"} and inline:
            origin += 1
        feed(screen, data)
        screen.update_only_line_graphics_data()
        layers = screen.grman.update_layers(
            0, -1, 1, 2 / 40, 2 / screen.lines, 40, screen.lines, 8, 16,
        )
        if inline:
            external = screen.grman.image_for_client_id(1)
            require(external is not None and external["data"] == b"\xff\0\xff",
                    (name, "unrelated terminal image was changed"))
            layers = [layer for layer in layers if layer["image_id"] != external["internal_id"]]
        uploads = re.findall(rb"a=t,[^;]*,i=(\d+)", data)
        if count > 26:
            verify_group(name, data, screen, layers, origin, uploads, int(inline))
            continue
        if uploads:
            image_id = int(uploads[-1])
        require(screen.grman.image_count == (0 if name in empty else 1) + int(inline),
                (name, "image count", screen.grman.image_count))
        if not name.startswith("placeholder"):
            require(len(layers) == (
            0 if name in empty else
            2 if name in {"shared image", "move"} else
            5 if name == "text overlay" else
            6 if name in {"partial clear", "replay", "pending edit"} else 1
            ), (name, "layers", layers))
        if name not in empty:
            image = screen.grman.image_for_client_id(image_id)
            require(image is not None, (name, image_id))
            require((image["width"], image["height"]) == (96, 64), (name, image))
            require(len(image["data"]) == 96 * 64 * 3, (name, "pixel count"))
        if name in {"identical rebuild", "placeholder rebuild"}:
            require(not data, (name, "redundant output"))
        if name in {"shared image", "move", "text overlay", "partial clear", "pending edit"}:
            require(not uploads, (name, "redundant upload"))
        if name in {"contain", "identical rebuild", "shared image"}:
            require(screen.grman.image_for_client_id(image_id)["data"][:3] == b"\xff\0\0",
                    (name, "source pixels"))
        if name == "move":
            require(str(screen.line(origin + 2))[2:16] == "underlying tex",
                    (name, "underlying text", str(screen.line(origin + 2))))
        if name in {"text overlay", "partial clear", "replay", "pending edit"}:
            expected = "EDIT" if name == "pending edit" else "TEXT"
            require(str(screen.line(origin + 7))[5:9] == expected,
                    (name, str(screen.line(origin + 7))))
            for layer in layers:
                r = layer["dest_rect"]
                left = round((r["left"] + 1) * 20)
                right = round((r["right"] + 1) * 20)
                top = round((1 - r["top"]) * screen.lines / 2) - origin
                bottom = round((1 - r["bottom"]) * screen.lines / 2) - origin
                require(not (left < 9 and right > 5 and top <= 7 < bottom),
                        (name, "image covers text", layer))
        if name in {"crop", "invalidate", "resume"}:
            pixels = screen.grman.image_for_client_id(image_id)["data"]
            center = (32 * 96 + 48) * 3
            require(pixels[center:center + 3] == b"\0\xff\0", (name, "crop pixels"))
        if name.startswith("placeholder") and name not in empty:
            area = 0
            for layer in layers:
                r = layer["dest_rect"]
                left = round((r["left"] + 1) * 20)
                right = round((r["right"] + 1) * 20)
                top = round((1 - r["top"]) * screen.lines / 2) - origin
                bottom = round((1 - r["bottom"]) * screen.lines / 2) - origin
                area += (right - left) * (bottom - top)
                if name in {"placeholder text", "placeholder replay", "placeholder edit"}:
                    require(not (left < 9 and right > 5 and top <= 7 < bottom),
                            (name, "placeholder covers text", layer))
            expected = 92 if name in {"placeholder text", "placeholder replay", "placeholder edit"} else 96
            require(area == expected, (name, "visible placeholder cells", area, layers))
    require(count == 42, ("missing stages", count))
    print(f"Verified {count} native image frames ({'inline' if inline else 'fullscreen'}).")

def verify_group(name, data, screen, layers, origin, uploads, external):
    absent = name in {"relative remove", "anchor erase", "relative cleanup"}
    require(screen.grman.image_count == (1 if name == "anchor erase" else 0 if absent else 2) + external,
            (name, "group resources", screen.grman.image_count))
    if absent:
        require(not layers, (name, "stale native children", layers))
        return
    anchor = (
        (22, 6) if name in {"relative move", "relative text"} else
        (2, 1) if name == "anchor row move" else
        (5, 1) if name in {"anchor column move", "attached text"} else
        (2, 6) if name in {"anchor return", "attached replay", "attached edit", "attached shrink", "attached grow"} else (2, 2)
    )
    x, y = anchor
    expected = {(x + col, y + row) for col in range(8) for row in range(3)}
    expected |= {(x + 4 + col, y + 1 + row) for col in range(8) for row in range(3)}
    if name in {"relative text", "attached text", "attached edit", "attached shrink", "attached grow"}:
        expected -= {(x + 4 + col, y + 1) for col in range(4)}
        text = str(screen.line(origin + y + 1))
        require(("EDIT" if name in {"attached edit", "attached shrink", "attached grow"} else "TEXT") in text, (name, text))
    if name == "attached shrink":
        expected = {(col, row) for col, row in expected if col < 10 and row < 8}
    actual = set()
    for layer in layers:
        r = layer["dest_rect"]
        left = round((r["left"] + 1) * 20)
        right = round((r["right"] + 1) * 20)
        top = round((1 - r["top"]) * screen.lines / 2) - origin
        bottom = round((1 - r["bottom"]) * screen.lines / 2) - origin
        actual |= {(col, row) for col in range(left, right) for row in range(top, bottom)}
    require(actual == expected, (name, "group coverage", actual ^ expected))
    if name == "relative rebuild":
        require(not data, (name, "redundant output"))
    if name in {"relative move", "relative text", "anchor row move", "anchor column move", "attached text", "attached edit"}:
        require(not uploads, (name, "redundant image upload"))


if __name__ == "__main__":
    records = []
    for record in sys.stdin:
        name, payload = record.rstrip("\n").split("\t")
        records.append((name, bytes.fromhex(payload)))
    verify(records, "--inline" in sys.argv)
