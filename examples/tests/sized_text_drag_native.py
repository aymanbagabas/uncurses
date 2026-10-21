"""Compare every native row after an incremental drag with a fresh frame."""

import sys

from kitty.fast_data_types import Screen


def require(condition, detail):
    if not condition:
        raise AssertionError(detail)


def feed(screen, payload):
    pending = bytes.fromhex(payload)
    while pending:
        count = screen.test_commit_write_buffer(pending, screen.test_create_write_buffer())
        require(count > 0, "native parser made no progress")
        pending = pending[count:]
        screen.test_parse_written_data()


size = None
frames = 0
uniform_background = True
for record in sys.stdin:
    if record == "reset\n":
        size = None
        uniform_background = True
        continue
    if record.startswith("resize\t"):
        _, width, height = record.rstrip().split("\t")
        size = int(width), int(height)
        actual.resize(int(height), int(width))
        uniform_background = False
        continue
    if record.startswith("probe\t"):
        probe = bytes.fromhex(record.rstrip().split("\t")[1])
        parts = probe.split(b"\x1b[6n")
        require(len(parts) == 4 and parts[-1] == b"", parts)
        terminal = Screen(None, 8, 40)
        positions = []
        for part in parts[:-1]:
            feed(terminal, part.hex())
            positions.append((terminal.cursor.x, terminal.cursor.y))
        require(positions == [(0, 0), (2, 0), (4, 0)], positions)
        continue
    width, height, x, y, incremental, reference = record.rstrip().split("\t")
    width, height, x, y = int(width), int(height), int(x), int(y)
    if size != (width, height):
        actual = Screen(None, height, width)
        size = width, height
    expected = Screen(None, height, width)
    feed(actual, incremental)
    feed(expected, reference)
    blocks = [row for row in range(height) if "]66;" in actual.line(row).as_ansi()]
    require(blocks == ([y] if y >= 0 else []), (frames, x, y, blocks))
    if y >= 0:
        require("\x1b]66;w=6:s=3;拖动我" in actual.line(y).as_ansi(), (frames, x, y))
    if uniform_background and x >= 0 and x % 2:
        uncovered_row = next(row for row in range(height - 1) if not y <= row < y + 3)
        for edge in (x - 1, x + 18):
            if edge >= width:
                continue
            style = repr(expected.line(uncovered_row).cursor_from(edge))
            for row in range(y, y + 3):
                require(
                    repr(actual.line(row).cursor_from(edge)) == style,
                    (frames, "split glyph lost its style", edge, row),
                )
    for row in range(height):
        got = actual.line(row)
        want = expected.line(row)
        for col in range(width):
            got_cell = (got[col].replace("\0", " "), repr(got.cursor_from(col)))
            want_cell = (want[col].replace("\0", " "), repr(want.cursor_from(col)))
            require(got_cell == want_cell, (
                f"frame {frames}, size={size}, heading=({x}, {y}), cell=({col}, {row})\n"
                f"actual:   {got_cell!r}\nexpected: {want_cell!r}"
            ))
    frames += 1
print(f"Verified {frames} complete native frames.")
