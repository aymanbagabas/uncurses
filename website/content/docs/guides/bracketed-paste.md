---
title: "Bracketed paste"
weight: 3
---

Bracketed paste lets your app distinguish pasted bytes from fast typing. uncurses enables it by default on `Screen::init()`, then emits `PasteStart`, one or more `PasteChunk(Vec<u8>)` events, and `PasteEnd`.

{{< callout type="info" >}}
Run `cargo run --example paste` to see in-memory reassembly. Run `cargo run --example paste_to_file` to see large pastes spill to a file.
{{< /callout >}}

## Walk through the examples

### 1. Rely on the default

`ScreenOptions::default()` has `bracketed_paste: true`, so the basic example only calls `init()`.

```rust
let mut screen = Screen::stdio()?;
screen.init()?; // bracketed paste is enabled by default
screen.enter_alt_screen()?;
screen.hide_cursor()?;
```

To opt out, pass explicit options:

```rust
screen.init_with(ScreenOptions {
    bracketed_paste: false,
    ..ScreenOptions::default()
})?;
```

### 2. Assemble chunks into bytes

`paste.rs` keeps an `Option<Vec<u8>>` between `PasteStart` and `PasteEnd`, then decodes once at the boundary.

```rust
let mut last: Option<String> = None;
let mut pending: Option<Vec<u8>> = None;

match screen.read_event()? {
    Event::PasteStart => pending = Some(Vec::new()),
    Event::PasteChunk(bytes) => {
        if let Some(buf) = pending.as_mut() {
            buf.extend_from_slice(&bytes);
        }
    }
    Event::PasteEnd => {
        if let Some(buf) = pending.take() {
            last = Some(String::from_utf8_lossy(&buf).into_owned());
            render(screen, last.as_deref());
        }
    }
    _ => {}
}
```

`PasteChunk` contains bytes, not text. That is why the example waits until `PasteEnd` before using `String::from_utf8_lossy`.

### 3. Spill large pastes to a file

`paste_to_file.rs` uses a sink that starts in memory and switches to a file once the paste exceeds a threshold.

```rust
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::PathBuf;

const THRESHOLD: usize = 512;

struct PasteSink {
    mem: Vec<u8>,
    total: usize,
    spill: Option<(PathBuf, BufWriter<File>)>,
}
```

The `push` method either appends to memory, crosses the threshold and opens a file, or streams directly to the existing file.

```rust
fn push(&mut self, chunk: &[u8]) -> std::io::Result<()> {
    self.total += chunk.len();

    if let Some((_, file)) = self.spill.as_mut() {
        return file.write_all(chunk);
    }

    self.mem.extend_from_slice(chunk);
    if self.mem.len() > THRESHOLD {
        let path = std::env::temp_dir()
            .join(format!("uncurses_paste_{}.txt", std::process::id()));
        let mut file = BufWriter::new(File::create(&path)?);
        file.write_all(&self.mem)?;
        self.mem = Vec::new();
        self.spill = Some((path, file));
    }
    Ok(())
}
```

On `PasteEnd`, finish the sink and report whether the paste stayed in memory or landed on disk.

```rust
Event::PasteStart => sink = Some(PasteSink::new()),
Event::PasteChunk(bytes) => {
    if let Some(s) = sink.as_mut() {
        s.push(&bytes)?;
    }
}
Event::PasteEnd => {
    if let Some(s) = sink.take() {
        last = Some(s.finish()?);
        render(screen, last.as_ref());
    }
}
```

## Common pitfalls

{{< callout type="warning" >}}
Do not treat each `PasteChunk` as a complete string. Chunks may split UTF-8 sequences and large pastes may span many chunks. Accumulate bytes, then decode or stream them at `PasteEnd`.
{{< /callout >}}

## See also

- [Inline rendering]({{< relref "inline-rendering.md" >}})
- [The Screen facade]({{< relref "../concepts/screen.md" >}}#screenoptions)
- [Examples]({{< relref "../examples.md" >}}#the-full-mix-input-render)
- [API reference](/api/)
