---
title: "Pause and resume"
weight: 8
---

Sometimes your app needs to step aside and give the terminal back: to drop the
user into `$EDITOR`, run a shell command that draws its own output, or handle a
`Ctrl-Z` suspend. `pause` and `resume` bracket that handoff and bring your screen
back afterward.

## Shelling out to a child

`pause` tears down staged modes and restores the terminal to the state it had
before your app took over, without dropping the `Screen`. Run your child process
with inherited stdio, then call `resume` to re-enter raw mode and refit to the
current window size. After that, draw and render a fresh frame.

```rust
use std::process::Command;

fn edit(screen: &mut Screen<Stdin, Stdout>, path: &str) -> std::io::Result<()> {
    let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".into());

    screen.pause()?; // release the terminal in its pre-init state

    Command::new(editor)
        .arg(path)
        .status()?; // child owns the terminal here

    screen.resume()?; // re-enter raw mode and refit; arms the next full repaint

    redraw(screen); // you lay out the new frame...
    screen.render() // ...and paint it; resume does not redraw for you
}
```

While paused, the child has the terminal to itself: uncurses restores the saved
pre-init state, so the child can draw and read normally while your `Screen` stays
alive. `resume` restores raw mode, re-applies your staged modes (alternate
screen, hidden cursor, mouse, and so on), refits the managed area to the current
window, and arms a full clear-and-repaint for your next `render`.

{{< callout type="warning" >}}
`resume` does not redraw the previous frame for you. The window may have been
resized while you were away, which would make the old frame wrong to replay, so
uncurses leaves the drawing to you: lay out a fresh frame and call `render` after
resuming. It also does not clear whatever the child left on screen. In inline
mode, anything drawn above your surface stays put, so clear it yourself if you
need a clean slate.
{{< /callout >}}

```mermaid
flowchart TB
  app["your screen (raw mode, alt screen, ...)"]
  app -->|pause| released["terminal restored to its pre-init state"]
  released -->|run child| child["$EDITOR draws and reads"]
  child -->|resume| app
```

## Handling Ctrl-Z

On Unix, `suspend` handles the suspend key: it pauses the screen, then stops the
process with `SIGTSTP`. The shell's job control takes over, holds your app as a
stopped job, and reclaims the terminal. When the user runs `fg`, `suspend`
returns and you call `resume`.

```rust
// in your event loop, on Ctrl-Z:
#[cfg(unix)]
{
    screen.suspend()?; // pause + SIGTSTP; returns when foregrounded
    screen.resume()?;  // re-acquire the terminal
}
```

Raw mode turns off the terminal's signal keys, so `Ctrl-Z` no longer suspends
your app on its own; it just arrives as a key event. `suspend` opts into that
stop, restoring the terminal before it stops the process so the shell never
inherits raw mode.

## Async note

If you are driving input through the low-level [async stream]({{< relref
"async-events.md" >}}), drop your `EventStream` before `pause` so its helper
thread does not fight the child for input, then build a fresh one after `resume`.

See the `screen_toggle` example for a live inline/alt-screen toggle that also
suspends and resumes on `Ctrl-Z`.
