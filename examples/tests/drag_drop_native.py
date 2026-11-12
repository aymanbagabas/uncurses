"""Connect the drag_drop example's PTY to Kitty's native OSC 72 engine.

Run after cargo build -p examples --example drag_drop:
kitty +runpy 'import runpy; runpy.run_path("examples/tests/drag_drop_native.py", run_name="__main__")'

For the controller example, build drag_drop_high_level instead and run the
same command with UNCURSES_DND_HIGH_LEVEL=1.
The controller run also checks exit before delivery of the drag startup reply.

Desktop gestures are simulated through Kitty's test hooks. This checks native
protocol state and PTY input, not operating-system drag visuals.
"""

import errno
import os
import pty
import re
import select
import struct
import subprocess
import termios
import time
import fcntl

from kitty import fast_data_types as kitty
from kitty.options.types import defaults


def require(condition, message):
    if not condition:
        raise AssertionError(message)


kitty.set_options(defaults)
high_level = os.environ.get("UNCURSES_DND_HIGH_LEVEL") == "1"
example = "drag_drop_high_level" if high_level else "drag_drop"
master, slave = pty.openpty()
fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 24, 80, 640, 384))
original = termios.tcgetattr(slave)


def start_example():
    return subprocess.Popen(
        [f"target/debug/examples/{example}"],
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

    # Native pixel offsets can be negative even when the cell is in the window.
    kitty.dnd_test_start_drag_offer(window, 5, 3, 40, -1)
    pump_until(lambda: b"\x1b]72;t=P:x=-1\x1b\\" in output)
    require(probe("drag_operations") == 1,
            f"Drag offer failed: {probe('drag_operations')}, {bytes(replies[-1000:])!r}")
    if high_level:
        require(probe("drag_thumbnail_size") == 0, "Unexpected thumbnail")
    else:
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
    kitty.dnd_test_drag_finish(window, True)
    pump_until(lambda: "canceled" in visible())

    # Quit during an accepted drop, so cleanup must cancel and unregister.
    kitty.dnd_test_fake_drop_event(window, True, ["text/plain"], 5, 3, 2)
    pump_until(lambda: probe("drop_getting_data_for_mime") == "text/plain")
    reply(window, b"q")
    # Leave cleanup output unread until exit to exercise buffered PTY output.
    require(process.wait(timeout=5) == 0, "Example failed")
    pump_until(lambda: b"\x1b]72;t=o:x=2\x1b\\" in output)
    require(not probe("drop_wanted") and not probe("drag_can_offer"), "Registration leaked")
    require(probe("last_drop_action") == 0, "Pending drop was not canceled")
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
        require(process.wait(timeout=5) == 0, "Example failed during drag startup")
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
    require(process.wait(timeout=5) == 0, "Example did not quit before DA1")
    require(not probe("drop_wanted") and not probe("drag_can_offer"),
            "Registration leaked before DA1")
    check_restored()
    print("Verified rendering and keyboard input before DA1.")
    print("Verified native drops, chunked offers, cancellation, registration cleanup, and PTY restoration.")
finally:
    if process.poll() is None:
        process.terminate()
        process.wait(timeout=5)
    kitty.dnd_set_test_write_func(None)
    kitty.dnd_test_cleanup_fake_window(os_window)
    os.close(master)
    os.close(slave)
