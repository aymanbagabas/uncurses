"""Native parser helpers and a replay check for placements --dump.

    target/debug/examples/placements --dump |
      kitty +runpy 'import runpy; runpy.run_path(
        "examples/tests/placements_native.py", run_name="__main__")'
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


if __name__ == "__main__":
    records = [record.rstrip("\n").split("\t") for record in sys.stdin]
    require([name for name, _ in records] == ["0", "1", "2", "3"], "unexpected stages")
    screen = Screen(None, 10, 40, 100, 8, 16)
    for index, (_, encoded) in enumerate(records):
        payload = bytes.fromhex(encoded)
        require(b"\0p" not in payload, "placement metadata escaped")
        feed(screen, payload)
        expected = list("." * 40)
        if index < 3:
            x = 12 if index == 2 else 2
            expected[x:x + 9] = "PLACEMENT" if index == 0 else "PLATOPENT"
        actual = [screen.line(3)[x] for x in range(40)]
        require(actual == expected, (index, actual, expected))
        require((screen.cursor.x, screen.cursor.y) == (0, 9), (index, "cursor rest"))
    print("Verified four portable placement stages and background restoration.")
