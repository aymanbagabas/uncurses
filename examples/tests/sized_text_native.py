"""Replay `sized_text --dump` through Kitty's native parser.

Run from the repository root:
    cargo run -p examples --example sized_text -- --dump |
      kitty +runpy 'import runpy; runpy.run_path("examples/tests/sized_text_native.py", run_name="__main__")'
"""

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


def verify(records, inline=False, start=0):
    height = 24 if inline else 8
    screen = Screen(None, height, 40)
    if inline:
        feed(screen, "".join(f"history-{row}\r\n" for row in range(start)).encode())
        feed(screen, f"\x1b[{start + 1};8H".encode())
    origin = min(start, height - 8)
    history = [f"history-{row}" for row in range(start - origin, start)]

    def line(y):
        return screen.line(origin + y)

    stages = [
        "initial",
        "identical rebuild",
        "origin overwrite",
        "incremental edit",
        "explicit replacement",
        "lower row overwrite",
        "before view overwrite",
        "view overwrite",
        "before move",
        "move and restore base",
        "remove",
        "prepared custom region",
        "move prepared region",
        "remove prepared region",
        "overlay base",
        "disjoint caption edits",
        "text over caption",
        "clear part of caption",
        "fill part of caption",
        "replay overlaid region",
        "pending overlay edit",
        "remove overlaid region",
        "width and fractional scale",
        "cleanup",
    ]
    resized = ("inline grow insertion", "inline shrink insertion", "inline resized frame")
    if inline:
        stages[-1:-1] = resized
    blocks = {
        "initial",
        "identical rebuild",
        "explicit replacement",
        "before view overwrite",
        "before move",
        "move and restore base",
        "width and fractional scale",
        "prepared custom region",
        "move prepared region",
        "overlay base",
        "disjoint caption edits",
        "text over caption",
        "clear part of caption",
        "fill part of caption",
        "replay overlaid region",
        "pending overlay edit",
    }
    if inline:
        blocks.update(resized)
    seen = []
    for record in records:
        name, payload = record.rstrip("\n").split("\t")
        seen.append(name)
        if name == "replay overlaid region" and inline:
            origin = min(origin + 1, height - 8)
            history = (history + ["overlay log"])[-origin:]
        if name in resized[:2]:
            band_height = 10 if name == resized[0] else 6
            origin = min(origin + 1, height - band_height)
            history = (history + [name])[-origin:]
        feed(screen, bytes.fromhex(payload))

        row = str(line(2))
        encoded = line(2).as_ansi()
        require(("66;" in encoded) == (name in blocks),
                (name, origin, [screen.line(y).as_ansi() for y in range(height)]))
        if name in blocks:
            title = "Ready" if name in (
                "prepared custom region", "move prepared region", "overlay base",
                "disjoint caption edits",
                "text over caption", "clear part of caption", "fill part of caption",
                "replay overlaid region", "pending overlay edit",
            ) else (
                "ab" if name == "width and fractional scale" or name in resized else "Build"
            )
            require(title in encoded, (name, encoded))
        if name == "identical rebuild":
            require(not payload, (name, payload))
        if name not in ("width and fractional scale", "cleanup"):
            label = "initial / identical rebuild" if name in stages[:2] else name
            if name in resized:
                label = "width and fractional scale"
            if name == "replay overlaid region":
                label = "fill part of caption"
            require(str(line(0)).rstrip() == label, (name, str(line(0))))
        if name == "origin overwrite":
            require(row.rstrip() == "  X          cells", (name, row))
        if name == "incremental edit":
            require(row.rstrip() == "  Y          cells", (name, row))
        if name in ("lower row overwrite", "view overwrite"):
            require(row.rstrip() == "             cells", (name, row))
        if name in ("remove", "remove overlaid region"):
            require(row.rstrip() == "  Ynderlying cells", (name, row))
        if name == "move and restore base":
            require(row[2:18] == "Ynderlying cells", (name, row))

        lower = "".join(line(3)[x].replace("\0", " ") or " " for x in range(40)).rstrip()
        expected = ""
        if name == "lower row overwrite":
            expected = "   abc"
        elif name == "view overwrite":
            expected = "   X"
        elif name in ("move and restore base", "remove", "move prepared region", "remove prepared region", "remove overlaid region"):
            expected = "   Xbc"
        require(lower == expected, (name, lower, expected))
        caption = str(line(4)).rstrip()
        expected_caption = {
            "prepared custom region": "  prepared",
            "move prepared region": "                    prepared",
            "overlay base": "  prepared",
            "disjoint caption edits": "  prXpYred",
            "text over caption": "  prLIVEed",
            "clear part of caption": "  prLI  ed",
            "fill part of caption": "  pr++  ed",
            "replay overlaid region": "  pr++  ed",
            "pending overlay edit": "  pr??  ed",
            "remove overlaid region": "    ??",
        }.get(name, "")
        require(caption == expected_caption, (name, caption, expected_caption))
        if expected_caption and name != "remove overlaid region":
            x = 20 if name == "move prepared region" else 2
            require(line(2).cursor_from(x).bold, (name, "title lost bold"))
            style = line(4).cursor_from(x)
            require(style.dim and not style.bold, (name, "caption style", repr(style)))
        if name == "cleanup":
            require(not row.strip(), row)
        if name in resized:
            require(str(line(5)).rstrip() == ("pending" if name == resized[2] else ""), name)
        expected_cursor = (
            ((0, 5) if inline else (2, 2)) if name == "cleanup" else
            (0, 0) if name == "replay overlaid region" else
            (0, 0) if name in resized[:2] else
            (0, 5) if name == resized[2] else (0, 7)
        )
        expected_cursor = (expected_cursor[0], expected_cursor[1] + origin)
        require((screen.cursor.x, screen.cursor.y) == expected_cursor,
                (name, (screen.cursor.x, screen.cursor.y), expected_cursor))
        if inline:
            for y, expected in enumerate(history):
                require(str(screen.line(y)).rstrip() == expected, (name, y, str(screen.line(y)), expected))
        print(f"{name} (origin {origin}): verified")

    require(seen == stages, seen)


def main():
    records = list(sys.stdin)
    if "--inline" in sys.argv:
        for start in (0, 8, 23):
            verify(records, inline=True, start=start)
    else:
        verify(records)


if __name__ == "__main__":
    main()
