"""Replay sized_text_overlap --dump through Kitty's native terminal parser."""

import sys

from kitty.fast_data_types import Screen


def require(condition, detail):
    if not condition:
        raise AssertionError(detail)


screen = Screen(None, 24, 80, 100, 8, 16)
stages = []
for record in sys.stdin:
    name, encoded = record.rstrip("\n").split("\t")
    stages.append(name)
    pending = bytes.fromhex(encoded)
    while pending:
        count = screen.test_commit_write_buffer(
            pending, screen.test_create_write_buffer()
        )
        require(count > 0, "native parser made no progress")
        pending = pending[count:]
        screen.test_parse_written_data()
    top = [screen.line(5)[x] for x in range(80)]
    lower = [screen.line(6)[x] for x in range(80)]
    sized_text = screen.line(5).as_ansi()
    if name == "before":
        require(sized_text.count("Demo") == 3, sized_text)
    elif name == "after":
        require(sized_text.count("Demo") == 1, "only the first block must remain")
        require(lower[12] == "X", "the lower-row write must skip to column 13")
        require(top[31] == "X", "the top-row write must stay at column 32")
        require(lower[57] == "X", "the explicit erase must allow column 58")
        require(top[30] in (" ", "\0") and top[56] in (" ", "\0"),
                "the other blocks must be erased")
        require([x for x in range(80) if lower[x] == "X"] == [12, 57],
                "unexpected lower-row overlay positions")
    else:
        raise AssertionError(("unexpected stage", name))
require(stages == ["before", "after"], stages)
print("Verified: lower-row skip, top-row erasure, and explicit erasure.")
