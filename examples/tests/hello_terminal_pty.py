"""Check animation delays after non-key events with the tui-test CLI.

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
    run("expect", "text", "Hello, terminal!", "--timeout", "5000")
    started = time.monotonic()
    run("resize", "81", "24")
    run("write", "\x1b[200~pasted text\x1b[201~")
    run("expect", "text", "Hello, terminal!", "--not", "--timeout", "6500")
    elapsed = time.monotonic() - started
    if elapsed < 4.5:
        raise AssertionError(f"A non-key event ended the five-second hold after {elapsed:.2f}s")
    run("expect", "text", "Hello, terminal!", "--timeout", "2000")
    run("type", "q")
    run("wait", "exit", "--timeout", "1000")
    exit_code = json.loads(run("--json", "state"))["data"]["exited"]
    if exit_code != 0:
        raise AssertionError(f"Expected exit code 0, got {exit_code}")
    print(f"Verified the hold after non-key events ({elapsed:.2f}s), loop restart, and keyboard exit.")
finally:
    run("close")
