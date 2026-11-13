"""Replay remote file transfers through Kitty's native OSC 72 engine.

Build: cargo build -p examples --example drag_drop_files
Run: kitty +runpy 'import runpy; runpy.run_path("examples/tests/drag_drop_files_native.py", run_name="__main__")'

The application and Kitty use separate processes and synthetic machine identity.
Set UNCURSES_DND_COMMAND to an SSH command that runs drag_drop_files --native
to check SSH transport as well. The command uses shell-style quoting but runs
without a shell. This test does not check desktop drag visuals.
Set UNCURSES_DND_FILES_LIVE=1 to run the interactive example in a PTY with real
source paths. The harness substitutes a distinct registration identity to
exercise remote transfers between processes on the same host.
"""

import base64
from collections import deque
import os
from pathlib import Path
import pty
import re
import select
import shlex
import struct
import subprocess
import tempfile
import termios
import time
import fcntl
from urllib.parse import unquote, urlsplit

from kitty import fast_data_types as kitty
from kitty.options.types import defaults


def require(condition, message):
    if not condition:
        raise AssertionError(message)


kitty.set_options(defaults)
live = os.environ.get("UNCURSES_DND_FILES_LIVE") == "1"
local = os.environ.get("UNCURSES_DND_FILES_LOCAL") == "1"
replies = deque()
received = bytearray()
response_metadata = None
response_body = bytearray()
selection_commands = []
frames = re.compile(rb"\x1b]72;([^\x1b]*)(?:\x1b\\)")


def capture(_window, data):
    global response_metadata
    for frame in frames.finditer(data):
        metadata, _, payload = frame[1].partition(b";")
        options = dict(field.split(b"=", 1) for field in metadata.split(b":"))
        replies.append((metadata, payload))
        if response_metadata is None and options.get(b"t") == b"r":
            response_metadata = options
        if response_metadata is None or options.get(b"t", b"r") != b"r":
            continue
        response_body.extend(payload)
        if not payload and options.get(b"m", b"0") == b"0":
            first = response_metadata
            if (first.get(b"t") == b"r"
                    and int(first.get(b"X", b"0")) in (0, 1)
                    and (int(first.get(b"y", b"0")) or int(first.get(b"Y", b"0")))):
                received.extend(base64.b64decode(response_body + b"=" * (-len(response_body) % 4)))
            response_metadata = None
            response_body.clear()


os_window, window = kitty.dnd_test_create_fake_window()
screen = kitty.Screen(None, 24, 80, 100, 8, 16, window)
kitty.dnd_set_test_write_func(capture)


def probe(name):
    return kitty.dnd_test_probe_state(window, name)


with tempfile.TemporaryFile() as logs, tempfile.TemporaryDirectory(
    prefix="dnd-native-", dir="target"
) as directory:
    root = Path(directory).resolve()
    sources = root / "source # tree"
    sources.mkdir()
    (sources / "movie.bin").write_bytes(b"Z" * 10_000)
    (sources / "link").symlink_to("movie.bin")
    (sources / "nested").mkdir()
    (sources / "nested" / "empty").write_bytes(b"")
    standalone_source = root / "standalone.bin"
    standalone_source.write_bytes(b"standalone")
    broken_link = root / "broken-link"
    broken_link.symlink_to("missing-target")
    command = shlex.split(os.environ.get(
        "UNCURSES_DND_COMMAND",
        "target/debug/examples/drag_drop_files" + ("" if live else " --native"),
    ))
    if live:
        command.extend([str(sources), str(standalone_source), str(broken_link)])
        master, slave = pty.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 24, 100, 800, 384))
        original = termios.tcgetattr(slave)
        os.set_blocking(master, False)
    process = subprocess.Popen(
        command,
        stdin=slave if live else subprocess.PIPE,
        stdout=slave if live else subprocess.PIPE,
        stderr=slave if live else logs,
        env={**os.environ, "TERM": "xterm-kitty"},
    )
    output = bytearray()
    pending_output = bytearray()
    replied_da = False
    pty_pending = bytearray()

    def diagnostics():
        if live:
            text = re.sub(rb"\x1b\[[0-?]*[ -/]*[@-~]", b"", bytes(output))
            return frames.sub(b"", text).decode("utf-8", "replace")
        logs.flush()
        logs.seek(0)
        return logs.read().decode("utf-8", "replace")

    def feed_native(pending):
        if live and not local:
            metadata, _, payload = pending[5:-2].partition(b";")
            if metadata in (b"t=a:x=1", b"t=o:x=1"):
                pending = b"\x1b]72;" + metadata + b";1:" + b"0" * 64 + b"\x1b\\"
        for frame in frames.finditer(pending):
            metadata = frame[1].partition(b";")[0]
            if metadata.startswith(b"t=P:"):
                selection_commands.append(metadata)
        while pending:
            count = screen.test_commit_write_buffer(pending, screen.test_create_write_buffer())
            require(count > 0, "Kitty parser made no progress")
            pending = pending[count:]
            screen.test_parse_written_data()

    def parse_output():
        ready, _, _ = select.select([process.stdout], [], [], 5)
        require(ready, f"No application response:\n{diagnostics()}")
        line = process.stdout.readline()
        require(line, f"Application exited:\n{diagnostics()}")
        feed_native(base64.b64decode(line))

    def write_pty(data):
        pty_pending.extend(data)

    def pump(condition=lambda: True):
        global replied_da
        if live:
            deadline = time.monotonic() + (120 if "UNCURSES_DND_DROP_PATH" in os.environ else 5)
            delayed_fragment = False
            while replies or pty_pending or not condition():
                if process.poll() is not None and (
                    replies or pty_pending or not select.select([master], [], [], 0)[0]
                ):
                    raise AssertionError(f"Application exited:\n{diagnostics()[-2000:]}")
                if replies and not pty_pending:
                    metadata, payload = replies.popleft()
                    write_pty(payload if metadata is None else
                              b"\x1b]72;" + metadata + b";" + payload + b"\x1b\\")
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    raise AssertionError(f"Timed out; {len(replies)} replies remain:\n{diagnostics()[-2000:]}")
                ready, writable, _ = select.select([master], [master] if pty_pending else [], [], remaining)
                if not ready and not writable:
                    raise AssertionError(f"No PTY progress:\n{diagnostics()[-2000:]}")
                if writable:
                    delay = (os.environ.get("UNCURSES_DND_FRAGMENT_DELAY") == "1"
                             and not delayed_fragment and len(pty_pending) > 1024
                             and pty_pending.startswith(b"\x1b]72;t=r"))
                    count = os.write(master, pty_pending[:512] if delay else pty_pending)
                    del pty_pending[:count]
                    if delay:
                        # Exercise the default 50 ms ESC deadline mid-payload.
                        time.sleep(0.1)
                        delayed_fragment = True
                if not ready:
                    continue
                data = os.read(master, 65536)
                if not data:
                    raise AssertionError(f"PTY closed:\n{diagnostics()[-2000:]}")
                output.extend(data)
                pending_output.extend(data)
                while True:
                    start = pending_output.find(b"\x1b]72;")
                    if start < 0:
                        del pending_output[:-5]
                        break
                    del pending_output[:start]
                    end = pending_output.find(b"\x1b\\")
                    if end < 0:
                        break
                    frame = bytes(pending_output[:end + 2])
                    del pending_output[:end + 2]
                    feed_native(frame)
                if not replied_da and b"\x1b[c" in output:
                    replies.append((None, b"\x1b[?62c"))
                    replied_da = True
            return
        while replies:
            metadata, payload = replies.popleft()
            process.stdin.write(metadata + b"\t" + base64.b64encode(payload) + b"\n")
            process.stdin.flush()
            parse_output()

    try:
        if live:
            pump(lambda: "Ready for file drops" in diagnostics())
        else:
            parse_output()
            pump()
        incoming = root / "incoming"
        incoming.mkdir()
        content = bytes(range(256)) * 512
        (incoming / "binary").write_bytes(content)
        (incoming / "link").symlink_to("binary")
        (incoming / "nested").mkdir()
        (incoming / "nested" / "empty").write_bytes(b"")
        byte_name = os.fsencode(incoming) + b"/line\nbreak"
        with open(byte_name, "wb") as file:
            file.write(b"arbitrary filename")
        kitty.dnd_test_set_mouse_pos(window, 5, 3, 40, 48)
        kitty.dnd_test_fake_drop_event(window, False, ["text/uri-list"], 5, 3, 2)
        pump(lambda: probe("drop_action") == 2)
        require(probe("drop_action") == 2, diagnostics())
        kitty.dnd_test_fake_drop_event(window, True, ["text/uri-list"], 5, 3, 2)
        pump(lambda: probe("drop_getting_data_for_mime") == "text/uri-list")
        require(probe("drop_getting_data_for_mime") == "text/uri-list", diagnostics())
        # A second drag replaces the drop whose URI request is still pending.
        kitty.dnd_test_fake_drop_event(window, False, ["text/uri-list"], 5, 3, 2)
        pump(lambda: probe("drop_action") == 2 and "Drop canceled by a new drag" in diagnostics())
        require(probe("drop_action") == 2, diagnostics())
        require("Drop canceled by a new drag" in diagnostics(), diagnostics())
        kitty.dnd_test_fake_drop_event(window, True, ["text/uri-list"], 5, 3, 2)
        pump(lambda: probe("drop_getting_data_for_mime") == "text/uri-list")
        drop_path = Path(os.environ.get("UNCURSES_DND_DROP_PATH", incoming)).resolve()
        kitty.dnd_test_fake_drop_data(
            window, "text/uri-list", (drop_path.as_uri() + "/\r\n").encode()
        )
        pump(lambda: b"Drop complete:" in output if live else "Drop complete:" in diagnostics())
        require(probe("last_drop_action") == 2, diagnostics())
        require(probe("drop_num_dir_handles") == 0, "Directory handles leaked")
        require(probe("drop_file_fd_plus_one") == 0, "File descriptor leaked")
        checksum = 0xcbf29ce484222325
        for byte in received:
            checksum = ((checksum ^ byte) * 0x100000001b3) & ((1 << 64) - 1)
        text = diagnostics()
        require(f"URI list: remote={str(not local).lower()}" in text, text)
        require(f"checksum={checksum:016x}" in text, text)
        require(f"{len(received)} bytes" in text, text)
        if "UNCURSES_DND_DROP_PATH" not in os.environ:
            require(len(received) == len(content) + len(b"binaryarbitrary filename"), text)
            require("line\\nbreak" in text, text)
            require("6 entries" in text, text)

        kitty.dnd_test_start_drag_offer(window, 5, 3, 40, 48)
        pump(lambda: "Drag started" in diagnostics())
        require(probe("drag_thumbnail_size") == 16 * 16 * 4 + (0 if live else 8), diagnostics())
        if not live:
            require(selection_commands[-3:] == [b"t=P:x=-1", b"t=P:x=3", b"t=P:x=0"],
                    repr(selection_commands))
        kitty.dnd_test_force_drag_dropped(window)
        if local:
            try:
                kitty.dnd_test_drag_get_data(window, "text/uri-list")
                raise AssertionError("Expected a request for fresh URI data")
            except BlockingIOError:
                pass
        else:
            kitty.dnd_test_request_drag_data(window, 0)
        pump(lambda: b"\x1b]72;t=e:y=0\x1b\\" in output if local
             else probe("drag_remote_data_complete") is None)
        require(probe("drag_remote_data_complete") is None, diagnostics())
        local_uris = kitty.dnd_test_drag_get_data(window, "text/uri-list")
        paths = []
        for uri in local_uris.decode().splitlines():
            if uri.startswith("file:"):
                paths.append(Path(unquote(urlsplit(uri).path)))
        require(len(paths) == (3 if live else 2), repr(local_uris))
        tree, standalone = paths[:2]
        if live:
            require(tree.name == sources.name, "Source URI escaping changed the name")
            require(paths[2].is_symlink(), "Broken root symlink was followed")
            require(os.readlink(paths[2]) == "missing-target", "Broken link target changed")
        require((tree / "movie.bin").read_bytes() == b"Z" * 10_000, "File content changed")
        require(os.readlink(tree / "link") == "movie.bin", "Symlink target changed")
        require((tree / "nested" / "empty").read_bytes() == b"", "Empty file changed")
        require(standalone.read_bytes() == b"standalone", "Second root content changed")
        kitty.dnd_test_drag_finish(window, False)
        pump(lambda: "finished: canceled=" in diagnostics())
        if live:
            write_pty(b"q")
            pump(lambda: b"\x1b[?1049l" in output)
        else:
            process.stdin.close()
        require(process.wait(timeout=5) == 0, diagnostics())
        if live:
            restored = termios.tcgetattr(slave)
            original[3] &= ~termios.PENDIN
            restored[3] &= ~termios.PENDIN
            require(original == restored, "Terminal attributes changed")
        print("Native remote replay passed: replacement drops, bytes, filenames, "
              "symlinks, trees, handles, and image thumbnails.")
    finally:
        if process.poll() is None:
            process.terminate()
            process.wait(timeout=5)
        if live:
            os.close(master)
            os.close(slave)
        else:
            process.stdout.close()
            if not process.stdin.closed:
                process.stdin.close()
        kitty.dnd_set_test_write_func(None)
        kitty.dnd_test_cleanup_fake_window(os_window)
