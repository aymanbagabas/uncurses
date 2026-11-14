"""Verify placements across terminal growth and shrinkage.

Run after building placements:
    python3 examples/tests/placements_resize_pty.py target/debug/examples/placements
"""

import fcntl
import os
import pty
import signal
import struct
import subprocess
import sys
import termios

from placements_drag_pty import read_until


def verify(executable):
    master, slave = pty.openpty()
    before = termios.tcgetattr(slave)
    process = None
    try:
        for rows, columns in [(6, 20), (12, 40), (6, 20), (10, 32)]:
            fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", rows, columns, 0, 0))
            if process is None:
                process = subprocess.Popen(
                    [executable],
                    stdin=slave,
                    stdout=slave,
                    stderr=subprocess.PIPE,
                    env={**os.environ, "TERM": "xterm-256color"},
                )
            else:
                process.send_signal(signal.SIGWINCH)
            marker = b"Original" if columns >= 24 else b"Resize to at least"
            output = read_until(master, marker)
            if columns >= 24 and b"PLACEMENT" not in output:
                raise AssertionError(("resize did not restore the placement", output))
        os.write(master, b"q")
        process.wait(timeout=15)
        if process.returncode != 0:
            raise AssertionError((process.returncode, process.stderr.read()))
        print("Terminal growth restored placements; shrinkage restored the resize warning.")
    finally:
        if process is not None:
            if process.poll() is None:
                process.kill()
                process.wait()
            process.stderr.close()
        termios.tcsetattr(slave, termios.TCSANOW, before)
        os.close(slave)
        os.close(master)


if __name__ == "__main__":
    verify(sys.argv[1])
