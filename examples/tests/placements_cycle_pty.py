"""Check interactive cycling when geometry or true color is unavailable.

    python3 examples/tests/placements_cycle_pty.py target/debug/examples/images_drag
"""

import fcntl
import os
from pathlib import Path
import pty
import runpy
import struct
import subprocess
import sys
import termios

read_until = runpy.run_path(str(Path(__file__).with_name("placements_drag_pty.py")))["read_until"]


def verify(executable, pixels):
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 24, 80, *pixels))
    process = None
    try:
        process = subprocess.Popen(
            [executable, "--symbols"],
            stdin=slave,
            stdout=slave,
            stderr=subprocess.PIPE,
            env={**os.environ, "TERM": "xterm-256color", "COLORTERM": "", "NO_COLOR": "1"},
        )
        read_until(master, b"position 2,3")
        os.write(master, b"p")
        read_until(master, b"Waiting for cell pixels" if pixels == (0, 0) else b"kitty")
        os.write(master, b"p")
        if pixels == (0, 0):
            read_until(master, b"placeholders")
            # A repeated waiting notice need not produce bytes.
            for expected in (b"iterm2", b"sixel", b"position 2,3"):
                os.write(master, b"p")
                read_until(master, expected)
        else:
            read_until(master, b"Kitty placeholders require a true-color")
            os.write(master, b"f")
            output = read_until(master, b"fit: cover")
            os.write(master, b"p")
            output += read_until(master, b"iterm2")
            if process.poll() is not None:
                raise AssertionError(("profile failure stopped the application", output))
        os.write(master, b"q")
        read_until(master, b"Resources cleared.")
        process.wait(timeout=15)
        if process.returncode != 0:
            raise AssertionError((process.returncode, process.stderr.read()))
        print(f"Interactive cycling remained available with window pixels {pixels}.")
    finally:
        if process is not None:
            if process.poll() is None:
                process.kill()
                process.wait()
            process.stderr.close()
        os.close(slave)
        os.close(master)


if __name__ == "__main__":
    verify(sys.argv[1], (0, 0))
    verify(sys.argv[1], (640, 384))
