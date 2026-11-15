"""Check animation resizing and delays with the tui-test CLI.

Run after cargo build -p examples --example hello_terminal:
python3 examples/tests/hello_terminal_pty.py
"""

import json
from pathlib import Path
import subprocess
import time
import uuid


session = f"uncurses-hello-terminal-{uuid.uuid4().hex}"
binary = Path(__file__).resolve().parents[2] / "target/debug/examples/hello_terminal"


def run(*arguments):
    result = subprocess.run(
        ["tui-test", "--session", session, *arguments],
        capture_output=True, text=True,
    )
    if result.returncode:
        raise AssertionError(f"{arguments}: {result.stderr}\n{result.stdout}")
    return result.stdout


run("run", str(binary), "--backend", "alacritty", "--cols", "80", "--rows", "24")
try:
    run("expect", "text", "Hello, terminal!", "--whitespace", "normalize", "--timeout", "5000")
    started = time.monotonic()
    run("resize", "12", "24")
    # Send the resize report directly instead of relying on SIGWINCH delivery.
    run("write", "\x1b[48;24;12;384;96t")
    run("expect", "cursor", "--x", "11", "--y", "1", "--timeout", "1000")
    narrow_text = json.loads(run("--json", "state"))["data"]["text"]
    # The emulator stores a cursor-movement tab in one blank cell.
    narrow_text = narrow_text.replace("\t", " ")
    if narrow_text != "\n  Hello, ter":
        raise AssertionError(f"The narrow frame wrapped or moved: {narrow_text!r}")
    run("resize", "80", "24")
    run("write", "\x1b[48;24;80;384;640t")
    run("expect", "text", "Hello, terminal!", "--whitespace", "normalize", "--timeout", "1000")
    run("expect", "cursor", "--x", "18", "--y", "1", "--timeout", "1000")
    run("write", "\x1b[200~pasted text\x1b[201~")
    run("expect", "text", "Hello, terminal!", "--whitespace", "normalize", "--not", "--timeout", "6500")
    elapsed = time.monotonic() - started
    if elapsed < 4.5:
        raise AssertionError(f"A non-key event ended the five-second hold after {elapsed:.2f}s")
    run("expect", "text", "Hello, terminal!", "--whitespace", "normalize", "--timeout", "2000")
    run("type", "q")
    run("wait", "exit", "--timeout", "1000")
    exit_code = json.loads(run("--json", "state"))["data"]["exited"]
    if exit_code != 0:
        raise AssertionError(f"Expected exit code 0, got {exit_code}")
    print(f"Verified resize redraws, the hold ({elapsed:.2f}s), loop restart, and keyboard exit.")
finally:
    run("close")
