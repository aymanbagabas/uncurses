"""Connect the drag_drop example's PTY to Kitty's native OSC 72 engine.

Run after cargo build -p examples --example drag_drop:
kitty +runpy 'import runpy; runpy.run_path("examples/tests/drag_drop_native.py", run_name="__main__")'

For the raw example, build drag_drop_raw instead and run the
same command with UNCURSES_DND_RAW=1.
The controller run also checks exit before delivery of the drag startup reply.
Set UNCURSES_DND_COMMAND to an SSH command with forced PTY allocation that runs
the selected example to check raw OSC 72 over SSH. Shell-style quoting is
supported; the command runs without a shell.

Desktop gestures are simulated through Kitty's test hooks. This checks native
protocol state and PTY input, not operating-system drag visuals.
"""

import errno
import os
import pty
import re
import select
import shlex
import struct
import subprocess
import termios
import time
import fcntl
import tempfile
from pathlib import Path

from kitty import fast_data_types as kitty
from kitty.options.types import defaults


def require(condition, message):
    if not condition:
        raise AssertionError(message)


kitty.set_options(defaults)
high_level = os.environ.get("UNCURSES_DND_RAW") != "1"
example = "drag_drop" if high_level else "drag_drop_raw"
destination = tempfile.TemporaryDirectory(prefix="dnd-mime-", dir="target")
save = high_level and "UNCURSES_DND_COMMAND" not in os.environ
master, slave = pty.openpty()
fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 24, 80, 640, 384))
original = termios.tcgetattr(slave)


def start_example():
    command = shlex.split(os.environ.get(
        "UNCURSES_DND_COMMAND", f"target/debug/examples/{example}"
    ))
    if save:
        command.extend(["--receive-dir", str(Path(destination.name).resolve())])
    return subprocess.Popen(
        command,
        stdin=slave, stdout=slave, stderr=slave,
        env={**os.environ, "TERM": "xterm-kitty"},
    )


process = start_example()
os_window, window = kitty.dnd_test_create_fake_window()
screen = kitty.Screen(None, 24, 80, 100, 8, 16, window)
output = bytearray()
replies = bytearray()
protocol_pending = bytearray()
replied_da = False
hold_da_reply = True
hold_start_reply = False
held_replies = bytearray()


def reply(_window, data):
    replies.extend(data)
    if hold_start_reply and data.startswith(b"\x1b]72;t=E"):
        held_replies.extend(data)
        return
    view = memoryview(data)
    while view:
        view = view[os.write(master, view):]


def probe(name):
    return kitty.dnd_test_probe_state(window, name)


def visible():
    text = re.sub(rb"\x1b\[[0-?]*[ -/]*[@-~]", b"", bytes(output))
    text = re.sub(rb"\x1b]72;.*?\x1b\\", b"", text, flags=re.DOTALL)
    return text.decode("utf-8", "replace")


def check_restored():
    restored = termios.tcgetattr(slave)
    # PENDIN is kernel-maintained pending-input state, not an application mode.
    original[3] &= ~termios.PENDIN
    restored[3] &= ~termios.PENDIN
    require(restored == original, f"Terminal attributes changed: {original!r} -> {restored!r}")


def pump_until(condition):
    global replied_da
    deadline = time.monotonic() + 5
    while not condition():
        remaining = deadline - time.monotonic()
        require(remaining > 0, f"Timed out:\n{visible()}\n{bytes(output[-2000:])!r}")
        ready, _, _ = select.select(
            [master], [], [], remaining if process.poll() is None else 0
        )
        if not ready:
            require(process.poll() is None,
                    f"Example exited: {process.returncode}\n{bytes(output)!r}\n"
                    f"Replies: {bytes(replies[-2000:])!r}")
        require(ready, f"No output:\n{visible()}")
        data = os.read(master, 65536)
        require(data, f"PTY closed:\n{visible()}")
        output.extend(data)
        # Feed only OSC 72 to the fake window. Other queries need a real
        # Kitty child and window callbacks, which this protocol test omits.
        protocol_pending.extend(data)
        while True:
            start = protocol_pending.find(b"\x1b]72;")
            if start < 0:
                del protocol_pending[:-5]
                break
            del protocol_pending[:start]
            end = protocol_pending.find(b"\x1b\\")
            if end < 0:
                break
            pending = bytes(protocol_pending[:end + 2])
            del protocol_pending[:end + 2]
            while pending:
                count = screen.test_commit_write_buffer(pending, screen.test_create_write_buffer())
                require(count > 0, "Kitty parser made no progress")
                pending = pending[count:]
                screen.test_parse_written_data()
        # A fake screen has no child writer for ordinary terminal queries.
        if not replied_da and b"\x1b[c" in output:
            if not hold_da_reply:
                reply(window, b"\x1b[?62c")
            replied_da = True


kitty.dnd_set_test_write_func(reply)


def wait_for_exit(message):
    if "UNCURSES_DND_COMMAND" in os.environ:
        # SSH must drain the remote PTY before its session can close.
        pump_until(lambda: b"\x1b[?1049l" in output)
    require(process.wait(timeout=5) == 0, message)


try:
    initial_drop_wanted = probe("drop_wanted")
    pump_until(lambda: replied_da and "Checking drag and drop support" in visible())
    require(probe("drop_wanted") == initial_drop_wanted
            and not probe("drag_can_offer") and b"\x1b]72;t=a" not in output,
            f"Drag and drop was enabled before DA1:\n{bytes(output)!r}\n"
            f"Replies: {bytes(replies)!r}")
    hold_da_reply = False
    reply(window, b"\x1b[?62c")
    pump_until(lambda: probe("drag_can_offer") and "Ready for text" in visible())
    kitty.dnd_test_set_mouse_pos(window, 5, 3, 40, 48)
    # GLFW uses 2 for copy; OSC 72 uses 1.
    formats = ["UTF8_STRING", "text/html", "text/plain"]
    kitty.dnd_test_fake_drop_event(window, False, formats, 5, 3, 2)
    pump_until(lambda: probe("drop_action") == 2)
    kitty.dnd_test_fake_drop_event(window, True, formats, 5, 3, 2)
    pump_until(lambda: probe("drop_getting_data_for_mime") == "text/plain")
    incoming = b"Incoming text!\n" * 700
    kitty.dnd_test_fake_drop_data(window, "text/plain", incoming)
    pump_until(lambda: "copy complete" in visible())
    require(f"{len(incoming)} bytes; copy complete" in visible(), ascii(visible()))
    require(probe("last_drop_action") == 2, "The drop must complete as a copy")
    if save:
        saved = list(Path(destination.name).glob("drop-*/text.txt"))
        require(len(saved) == 1 and saved[0].read_bytes() == incoming, "Saved text changed")

    if high_level:
        for mime, filename, payload in [
            ("text/plain;charset=utf-8", "text.txt", "UTF-8: caf\u00e9\n".encode()),
            ("UTF8_STRING", "text.txt", b"Native text alias\n"),
            ("text/html", "content.html", b"<script>do_not_execute()</script>\x1b[31m"),
            ("image/png", "image.png", Path("examples/examples/support/drag_drop.png").read_bytes()),
            ("image/jpeg", "image.jpg", Path("examples/examples/support/drag_drop.jpg").read_bytes()),
        ]:
            completed = output.count(b"\x1b]72;t=r:o=1\x1b\\")
            previous = set(Path(destination.name).iterdir())
            kitty.dnd_test_fake_drop_event(window, False, [mime], 5, 3, 2)
            pump_until(lambda: probe("drop_action") == 2)
            kitty.dnd_test_fake_drop_event(window, True, [mime], 5, 3, 2)
            pump_until(lambda: probe("drop_getting_data_for_mime") == mime)
            kitty.dnd_test_fake_drop_data(window, mime, payload)
            pump_until(lambda: output.count(b"\x1b]72;t=r:o=1\x1b\\") > completed)
            if save:
                created = set(Path(destination.name).iterdir()) - previous
                require(len(created) == 1, f"No fresh directory for {mime}")
                require((created.pop() / filename).read_bytes() == payload, f"Saved {mime} changed")

    # Native pixel offsets can be negative even when the cell is in the window.
    kitty.dnd_test_start_drag_offer(window, 5, 3, 40, -1)
    pump_until(lambda: b"\x1b]72;t=P:x=-1\x1b\\" in output)
    require(probe("drag_operations") == 1,
            f"Drag offer failed: {probe('drag_operations')}, {bytes(replies[-1000:])!r}")
    require(probe("drag_thumbnail_size") == 16 * 16 * 4, "Thumbnail dimensions changed")
    kitty.dnd_test_force_drag_dropped(window)
    try:
        kitty.dnd_test_drag_get_data(window, "text/plain")
        raise AssertionError("Expected a request for fresh drag data")
    except BlockingIOError as error:
        require(error.errno == errno.EAGAIN, str(error))
    pump_until(lambda: b"\x1b]72;t=e:y=0\x1b\\" in output)
    outgoing = bytearray()
    while True:
        block = kitty.dnd_test_drag_get_data(window, "text/plain")
        if not block:
            break
        outgoing.extend(block)
    require(outgoing == b"Hello from uncurses!\n" * 512, "Requested data mismatch")
    if high_level:
        for index, mime in enumerate(("text/plain;charset=utf-8", "UTF8_STRING"), 1):
            try:
                kitty.dnd_test_drag_get_data(window, mime)
                raise AssertionError(f"Expected fresh {mime}")
            except BlockingIOError:
                pass
            acknowledgment = f"\x1b]72;t=e:y={index}\x1b\\".encode()
            pump_until(lambda: acknowledgment in output)
            data = bytearray()
            while True:
                block = kitty.dnd_test_drag_get_data(window, mime)
                if not block:
                    break
                data.extend(block)
            require(data == outgoing, f"Outgoing {mime} changed")
    output.clear()
    kitty.dnd_test_drag_finish(window, True)
    pump_until(lambda: "canceled" in visible())

    if high_level:
        for key, mime, expected in [
            (b"h", "text/html", b'<!doctype html><meta charset="utf-8"><p>Hello from uncurses!</p>\n'),
            (b"p", "image/png", Path("examples/examples/support/drag_drop.png").read_bytes()),
            (b"j", "image/jpeg", Path("examples/examples/support/drag_drop.jpg").read_bytes()),
        ]:
            output.clear()
            reply(window, key)
            pump_until(lambda: mime.rsplit("/", 1)[1] in visible())
            kitty.dnd_test_start_drag_offer(window, 5, 3, 40, 48)
            pump_until(lambda: b"\x1b]72;t=P:x=-1\x1b\\" in output)
            kitty.dnd_test_force_drag_dropped(window)
            try:
                kitty.dnd_test_drag_get_data(window, mime)
                raise AssertionError(f"Expected a request for fresh {mime}")
            except BlockingIOError:
                pass
            pump_until(lambda: b"\x1b]72;t=e:y=0\x1b\\" in output)
            data = bytearray()
            while True:
                block = kitty.dnd_test_drag_get_data(window, mime)
                if not block:
                    break
                data.extend(block)
            require(data == expected, f"Outgoing {mime} changed")
            output.clear()
            kitty.dnd_test_drag_finish(window, True)
            pump_until(lambda: "canceled" in visible())

    # Quit during an accepted drop, so cleanup must cancel and unregister.
    kitty.dnd_test_fake_drop_event(window, True, ["text/plain"], 5, 3, 2)
    pump_until(lambda: probe("drop_getting_data_for_mime") == "text/plain")
    reply(window, b"q")
    # Leave cleanup output unread until exit to exercise buffered PTY output.
    wait_for_exit("Example failed")
    pump_until(lambda: b"\x1b]72;t=o:x=2\x1b\\" in output)
    require(not probe("drop_wanted") and not probe("drag_can_offer"), "Registration leaked")
    require(probe("last_drop_action") == 0, "Pending drop was not canceled")
    if save:
        require(len(list(Path(destination.name).iterdir())) == 6, "Partial drop directory leaked")
    check_restored()
    if high_level:
        output.clear()
        replies.clear()
        protocol_pending.clear()
        replied_da = False
        hold_start_reply = True
        process = start_example()
        pump_until(lambda: probe("drag_can_offer") and "Ready for text" in visible())
        kitty.dnd_test_start_drag_offer(window, 5, 3, 40, 48)
        pump_until(lambda: b"\x1b]72;t=P:x=-1\x1b\\" in output)
        require(b";OK\x1b\\" in held_replies, "Native drag startup reply was not withheld")
        require(probe("drag_operations") == 1, "Native drag did not start")
        reply(window, b"q")
        wait_for_exit("Example failed during drag startup")
        pump_until(lambda: not probe("drop_wanted") and not probe("drag_can_offer"))
        cancel = b"\x1b]72;t=E:y=-1\x1b\\"
        unregister = b"\x1b]72;t=o:x=2\x1b\\"
        require(cancel in output, "Pending drag startup was not canceled")
        require(output.index(cancel) < output.index(unregister),
                "The drag must be canceled before unregistration")
        check_restored()
        print("Verified cancellation before delivery of the native drag startup reply.")
    output.clear()
    replies.clear()
    protocol_pending.clear()
    replied_da = False
    hold_da_reply = True
    hold_start_reply = False
    process = start_example()
    pump_until(lambda: replied_da and "Checking drag and drop support" in visible())
    reply(window, b"q")
    try:
        process.wait(timeout=0.2)
        raise AssertionError("Example exited before it drained the pending capability reply")
    except subprocess.TimeoutExpired:
        pass
    reply(window, b"\x1b[?62c")
    wait_for_exit("Example did not quit after it drained DA1")
    pump_until(lambda: b"\x1b[?1049l" in output)
    require(not probe("drop_wanted") and not probe("drag_can_offer"),
            "Registration leaked before DA1")
    require(b"\x1b]72;t=a" not in output and b"\x1b]72;t=o\x1b\\" not in output,
            "The example registered after a startup quit request")
    check_restored()
    output.clear()
    replies.clear()
    protocol_pending.clear()
    replied_da = False
    process = start_example()
    pump_until(lambda: replied_da and "Checking drag and drop support" in visible())
    pump_until(lambda: b"capability query timed out" in output)
    require(process.wait(timeout=5) != 0, "Missing DA1 must report a timeout")
    check_restored()
    print("Verified delayed DA1 cleanup and a bounded capability-query timeout.")
    print("Verified native drops, chunked offers, cancellation, registration cleanup, and PTY restoration.")
finally:
    if process.poll() is None:
        process.terminate()
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=5)
    kitty.dnd_set_test_write_func(None)
    kitty.dnd_test_cleanup_fake_window(os_window)
    os.close(master)
    os.close(slave)
    destination.cleanup()
