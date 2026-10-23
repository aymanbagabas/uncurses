"""Verify full raster payloads precede dialog text.

Run after building images_drag:
    python3 examples/tests/raster_stream_order.py target/debug/examples/images_drag

This checks encoded output. Native Rio and iTerm2 visual checks remain separate.
"""

import base64
import re
import struct
import subprocess
import sys


def require(condition, detail):
    if not condition:
        raise AssertionError(detail)


for protocol in ("sixel", "iterm2"):
    for inline in (False, True):
        command = [sys.argv[1], "--" + protocol, "--dump-dialog"]
        if inline:
            command.append("--inline")
        records = subprocess.check_output(command, timeout=30).decode().splitlines()
        require(len(records) == 7, "missing dialog stages")
        for record in records:
            name, encoded = record.split("\t")
            payload = bytes.fromhex(encoded)
            if protocol == "sixel":
                images = list(re.finditer(rb"\x1bP[0-9;]*q(.*?)\x1b\\", payload, re.S))
            else:
                images = list(re.finditer(rb"\x1b\]1337;File=([^:\x07]+):([^\x07]+)\x07", payload))
            expected = 0 if name in ("dialog unchanged", "remove") else 1
            require(len(images) == expected, (protocol, name, "image count", len(images)))
            if name == "dialog unchanged":
                require(not payload, (protocol, name, "unexpected output"))
            for image in images:
                if protocol == "sixel":
                    require(image.group(1).startswith(b'"1;1;96;64'), (name, "cropped sixel"))
                else:
                    require(b"width=12;height=4;" in image.group(1), (name, "cropped IIP footprint"))
                    png = base64.b64decode(image.group(2), validate=True)
                    require(struct.unpack(">II", png[16:24]) == (96, 64), (name, "cropped PNG"))
                if name in ("dialog", "dialog move", "dialog origin"):
                    require(payload.rfind(b"Dialog") >= image.end(), (name, "dialog precedes image"))
            require(b"\0p" not in payload, (name, "placement metadata escaped"))
        print(f"Verified seven full-image dialog stages: protocol={protocol}, inline={inline}.")
