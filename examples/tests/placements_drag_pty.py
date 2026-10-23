"""Verify image geometry through a real PTY without cell-size replies.

Run after building images_drag:
    python3 examples/tests/placements_drag_pty.py target/debug/examples/images_drag
"""

import fcntl
import os
import pty
import select
import signal
import struct
import subprocess
import sys
import termios
import time


def read_until(master, marker):
    result = bytearray()
    deadline = time.monotonic() + 15
    while marker not in result:
        remaining = deadline - time.monotonic()
        if remaining <= 0 or not select.select([master], [], [], remaining)[0]:
            raise AssertionError(("missing terminal output", marker, bytes(result)))
        chunk = os.read(master, 65536)
        if not chunk:
            raise AssertionError(("unexpected EOF", bytes(result)))
        result.extend(chunk)
    return bytes(result)


def verify_resize(executable):
    master, slave = pty.openpty()
    before = termios.tcgetattr(slave)
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 24, 60, 480, 384))
    process = None
    try:
        process = subprocess.Popen(
            [executable, "--kitty"],
            stdin=slave,
            stdout=slave,
            stderr=subprocess.PIPE,
            env={**os.environ, "TERM": "xterm-kitty", "COLORTERM": "truecolor"},
        )
        initial = read_until(master, b"\x1b[24H")
        if b"a=t" not in initial:
            raise AssertionError("initial frame did not upload the image")
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 25, 60, 480, 400))
        process.send_signal(signal.SIGWINCH)
        resized = read_until(master, b"\x1b[25H")
        if b"Waiting for cell pixels" in resized or b"a=p," not in resized:
            raise AssertionError(("resize lost known pixel geometry", resized))
        if b"a=t," in resized:
            raise AssertionError("unchanged cell geometry uploaded the image again")
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 26, 60, 600, 416))
        process.send_signal(signal.SIGWINCH)
        changed = read_until(master, b"\x1b[26H")
        if b"a=t," not in changed or b"w=120,h=64," not in changed:
            raise AssertionError(("changed cell geometry did not prepare new pixels", changed))
        os.write(master, b"q")
        process.wait(timeout=15)
        if process.returncode != 0:
            raise AssertionError((process.returncode, process.stderr.read()))
        after = termios.tcgetattr(slave)
        pending = getattr(termios, "PENDIN", 0)
        before[3] &= ~pending
        after[3] &= ~pending
        if before != after:
            raise AssertionError(("terminal state changed", before, after))
        print("Resize retained usable window-derived image geometry and restored terminal state.")
    finally:
        if process is not None:
            if process.poll() is None:
                process.kill()
                process.wait()
            process.stderr.close()
        termios.tcsetattr(slave, termios.TCSANOW, before)
        os.close(slave)
        os.close(master)


def verify_symbol_override(executable):
    def initial_frame(geometry):
        output = subprocess.check_output(
            [executable, "--symbols", "--dump", "--cell-pixels", geometry],
            timeout=30,
        )
        return output.splitlines()[0]

    if initial_frame("8x16") == initial_frame("16x16"):
        raise AssertionError("symbol output ignored explicit cell geometry")
    print("Symbol preparation used the explicit cell geometry.")


if __name__ == "__main__":
    verify_resize(sys.argv[1])
    verify_symbol_override(sys.argv[1])
