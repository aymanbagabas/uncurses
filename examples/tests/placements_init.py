"""Verify terminal restoration after example initialization fails.

Run on Unix after building the example:
    python3 examples/tests/placements_init.py target/debug/examples/placements
"""

import fcntl
import os
import pty
import struct
import subprocess
import sys
import termios


def verify(executable, arguments):
    master, slave = pty.openpty()
    reader, writer = os.pipe()
    os.close(reader)
    before = termios.tcgetattr(slave)
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 24, 40, 320, 384))
    try:
        result = subprocess.run(
            [executable, *arguments],
            stdin=slave,
            stdout=writer,
            stderr=subprocess.PIPE,
            env={**os.environ, "TERM": "xterm-256color"},
            timeout=20,
        )
        after = termios.tcgetattr(slave)
        # macOS can mark pending input after canonical mode returns.
        pending = getattr(termios, "PENDIN", 0)
        expected = before.copy()
        expected[3] &= ~pending
        after[3] &= ~pending
        if result.returncode == 0 or b"BrokenPipe" not in result.stderr:
            raise AssertionError((result.returncode, result.stderr))
        if after != expected:
            raise AssertionError(("terminal state changed", expected, after))
        print("Failed initialization restored all terminal settings.")
    finally:
        try:
            termios.tcsetattr(slave, termios.TCSANOW, before)
        finally:
            os.close(writer)
            os.close(slave)
            os.close(master)


if __name__ == "__main__":
    verify(sys.argv[1], sys.argv[2:])
