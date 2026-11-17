---
title: "Handling paste"
weight: 5
---

When you paste into a terminal, the bytes can look like very fast typing, which
can be unsafe: a pasted script could trigger shortcuts. Bracketed paste fixes
that by wrapping the pasted text in markers, so you can treat it as data instead
of keystrokes. uncurses turns those markers into events, and bracketed paste is
enabled by default.

## The paste events

A paste arrives as a stream: one `Event::PasteStart`, zero or more
`Event::PasteChunk` payloads, then one `Event::PasteEnd`. The chunks are raw
bytes, because a paste can be large and a chunk might not be valid UTF-8 on its
own: a multibyte character can be split across a chunk boundary.
`PasteEnd { complete: true }` reports the terminal's paste terminator.
`complete: false` reports recovery or an input read failure.

```mermaid
flowchart TB
  accTitle: The paste event sequence
  accDescr {
    PasteStart begins a paste.
    Zero or more PasteChunk events carry its bytes.
    PasteEnd finishes the paste, including an empty paste.
  }
  s["Event::PasteStart"]
  s --> c["Event::PasteChunk(Vec&lt;u8&gt;) ... zero or more"]
  s --> e["Event::PasteEnd { complete }"]
  c --> e
```

## Collecting a paste

The common case: accumulate the chunks into a buffer, then decode once at
`PasteEnd`. This current-source sample returns the text and its completion flag.

```rust
use uncurses::event::Event;

fn collect_paste(ev: Event, paste: &mut Option<Vec<u8>>) -> Option<(String, bool)> {
    match ev {
        Event::PasteStart => {
            *paste = Some(Vec::new());
            None
        }
        Event::PasteChunk(bytes) => {
            paste.as_mut()?.extend_from_slice(&bytes);
            None
        }
        Event::PasteEnd { complete } => paste
            .take()
            .map(|buf| (String::from_utf8_lossy(&buf).into_owned(), complete)),
        _ => None,
    }
}
```

Program sync reads observe events automatically. Paste decoding does not depend
on capability tracking.

{{< callout type="warning" >}}
Decode at the end, not per chunk: a multibyte character can be split across
`PasteChunk` events, so decoding each chunk on its own would corrupt it. Pastes
also carry their own newlines, so normalize `\r\n` and lone `\r` to `\n` if your
buffer has a line model.
{{< /callout >}}

## Spilling large pastes to disk

Holding a paste in memory is fine until someone pastes a megabyte of logs. For
that, feed chunks into a sink that keeps bytes in memory up to a threshold, then
opens a spill file and streams the rest straight to disk.

```rust
use std::fs::File;
use std::io::{self, Write};
use std::path::Path;

const THRESHOLD: usize = 64 * 1024;

struct PasteSink {
    mem: Vec<u8>,
    spill: Option<File>,
}

impl PasteSink {
    fn push(&mut self, chunk: &[u8], spill_path: &Path) -> io::Result<()> {
        if let Some(file) = self.spill.as_mut() {
            return file.write_all(chunk);
        }
        self.mem.extend_from_slice(chunk);
        if self.mem.len() > THRESHOLD {
            let mut file = File::create(spill_path)?;
            file.write_all(&self.mem)?;
            self.mem.clear();
            self.spill = Some(file);
        }
        Ok(())
    }
}
```

Because chunks arrive one event at a time, you make this decision incrementally
and never hold the whole paste at once. The `paste_to_file` example implements
exactly this sink and reports whether the paste stayed in memory or landed in a
file.

## Idle recovery

The source ends an unfinished paste after two seconds without new input.
It emits any held bytes as a final `Event::PasteChunk`, then emits
`Event::PasteEnd { complete: false }`. Later bytes count as ordinary input.

Use `EventSource::with_paste_idle_timeout` to choose another timeout.
Pass `None` to wait for the real paste terminator.
For an existing `Program`, use
`program.event_source().lock().unwrap().set_paste_idle_timeout(timeout)`.
The setter restarts the idle wait during a paste.
An active async stream also refreshes its wait to use the new deadline.

Call `EventSource::end_paste()` for explicit recovery.
It delivers retained bytes before an incomplete end event.
An active async stream wakes to deliver these events.
Input read failures also deliver retained bytes and an incomplete end before
the error.

Idle recovery assumes that the paste stopped.
Keys before expiry can become payload and extend the wait.
Delayed payload after expiry can become keyboard input.
Disable the timer when your application must preserve framing through long pauses.

The `paste` example uses this setter. Press F2 between pastes to toggle
the idle timeout. Its display includes the completion flag.

## Turning it on and off

Bracketed paste is enabled by default. To start without it, pass
`ProgramOptions { bracketed_paste: false, ..ProgramOptions::default() }` to
`program.init_with`. At runtime, toggle it with
`program.enable_bracketed_paste()` and `program.disable_bracketed_paste()`. With
it off, pasted text comes through as ordinary key events instead.

See the `paste` and `paste_to_file` examples for the in-memory and
spill-to-disk versions.
