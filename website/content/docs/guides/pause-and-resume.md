---
title: "Pause and resume"
weight: 5
---

Use `Screen::pause()` when your TUI must hand the terminal to another program, such as `$EDITOR`, a pager, or a shell. `pause()` restores the terminal without consuming the `Screen`; `resume()` reacquires it and lets the app repaint.

{{< callout type="info" >}}
Run the example with `cargo run --example editor`. Press `e` to edit the scratch buffer in `$EDITOR`, then return to the app.
{{< /callout >}}

## Walk through the example

### 1. Start like any fullscreen app

`editor.rs` owns a `Screen`, enters the alternate screen, and renders a text buffer.

```rust
let mut screen = Screen::stdio()?;
screen.init()?;
screen.enter_alt_screen()?;
screen.hide_cursor()?;

let result = run(&mut screen);
screen.finish()?;
result
```

### 2. Trigger the shell handoff from the event loop

When the user presses `e`, the app calls a helper that edits the current text and then redraws.

```rust
match screen.read_event()? {
    Event::KeyPress(Key {
        code: KeyCode::Char('e'),
        ..
    }) => {
        status = match edit_in_editor(screen, &text) {
            Ok(edited) => {
                text = edited;
                "edited in $EDITOR".to_string()
            }
            Err(e) => format!("editor failed: {e}"),
        };
        render(screen, &text, &status);
    }
    Event::Resize(ws) => {
        screen.resize((ws.col, ws.row));
        render(screen, &text, &status);
    }
    _ => {}
}
```

### 3. Pause, run the child, then resume

The editor inherits normal process stdio. The `pause`/`resume` pair brackets the child so it sees a cooked terminal instead of your app's raw-mode screen.

```rust
use std::process::Command;

fn edit_in_editor(screen: &mut Screen<Stdin, Stdout>, text: &str) -> std::io::Result<String> {
    let editor = std::env::var("EDITOR")
        .or_else(|_| std::env::var("VISUAL"))
        .unwrap_or_else(|_| "vi".to_string());

    let path = std::env::temp_dir().join("uncurses_editor_example.txt");
    std::fs::write(&path, text)?;

    screen.pause()?;
    let spawn = Command::new(&editor).arg(&path).status();
    screen.resume()?;
    spawn?;

    let edited = std::fs::read_to_string(&path)?;
    let _ = std::fs::remove_file(&path);
    Ok(edited)
}
```

`resume()` re-enters raw mode, refits the canvas to the current viewport, restores tracked modes, invalidates the canvas, and flushes. Redraw your frame after it returns.

## Finish, pause, and suspend

| Method | What it does | Reuse the screen? |
| --- | --- | --- |
| `finish(self)` | Restore modes and consume the `Screen`. | No |
| `pause(&mut self)` | Restore modes but keep the `Screen` for `resume()`. Any async event stream is dropped so the next `events()` recreates it. | Yes |
| `suspend(&mut self)` | On Unix, `pause()` and then raise `SIGTSTP`; call `resume()` after the process is foregrounded. | Yes |

## Common pitfalls

{{< callout type="warning" >}}
Always call `resume()` after `pause()` even when the child command fails, then redraw. If you started an async event stream before pausing, expect the next `events()` call after resume to create a fresh stream.
{{< /callout >}}

## See also

- [The Screen facade]({{< relref "../concepts/screen.md" >}}#lifecycle)
- [Async events]({{< relref "async-events.md" >}})
- [Examples]({{< relref "../examples.md" >}}#the-full-mix-input-render)
- [API reference](../api/)
