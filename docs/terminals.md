# How terminals actually work

A quick mental model for the machinery uncurses sits on top of. The
[tutorial](tutorial.md) shows you *how* to drive it; this page is the
*why*. One idea carries the whole thing: a terminal is two streams of
bytes, and a TUI is a program that takes both of them over.

## 1. The terminal is a byte pipe

Your program never talks to a screen. It talks to a terminal emulator
through a pair of byte streams (a pty on Unix, a console on Windows). Two
directions, and that pair is the entire interface:

- **Output**: bytes you write. Printable text draws as-is. Anything
  starting with the escape byte `0x1b` (`ESC`) is a *command*: move the
  cursor, set a color, switch screens.
- **Input**: bytes you read. Keystrokes, mouse events, pasted text, and
  the terminal's answers to questions you asked all arrive on the same
  wire, as bytes.

uncurses wraps that pair in `Terminal`: an input half, an output half, and
the lifecycle glue that follows.

## 2. Line discipline: cooked, raw, and "sane"

Out of the box the terminal runs in **cooked** (canonical) mode, tuned for
shells. The line discipline sits between your program and the user: it
buffers a whole line before you see it, echoes what gets typed, turns
`Ctrl-C` into a kill signal, and rewrites `Enter` for you. Perfect for
`cat`. Useless for a UI, where you want every keystroke the instant it
lands and total control of the screen.

So a TUI switches to **raw** mode: no line buffering, no echo, no signal
translation, no meddling. Bytes in, bytes out, untouched. `Ctrl-C` stops
being a signal and becomes the byte `0x03`, and what happens next is your
call.

"**Sane**" is not really a third mode. It is the known-good factory reset,
the cooked settings a normal shell expects to come back to. Which is the
whole game: symmetry. Whatever you switched off, switch back on before you
leave.

The knobs differ by platform, but the idea is identical:

| Goal | Unix (`termios`) | Windows (console mode) |
| --- | --- | --- |
| stop line buffering | clear `ICANON` | clear `ENABLE_LINE_INPUT` |
| stop echo | clear `ECHO` | clear `ENABLE_ECHO_INPUT` |
| raw signals and keys | clear `ISIG`, `IEXTEN`, `IXON` | clear `ENABLE_PROCESSED_INPUT` |
| speak escape sequences | (always on) | set `ENABLE_VIRTUAL_TERMINAL_*` |

In uncurses, `Terminal::make_raw` flips all of this in a single call and
hands you the previous settings back as a `State`; `Terminal::restore`
puts them where it found them. You never touch a `termios` struct or a
console flag by hand.

## 3. Talking back: escape sequences and VT modes

Output is not just text. The escape byte `0x1b` introduces control
sequences, the VT100/xterm vocabulary every modern terminal speaks:

- **CSI** (`ESC [`): cursor moves, colors, clears. `ESC[2J` wipes the
  screen.
- **OSC** (`ESC ]`): string payloads like the window title or a
  hyperlink.
- **DCS** (`ESC P`): device control, the carrier for some queries.

The ones that make a TUI a TUI are **private modes**, toggled with
`ESC[?<n>h` to set and `ESC[?<n>l` to reset. Each number `n` is a switch
that changes how the terminal behaves. uncurses models them as
`ansi::mode::Mode` and gives `Screen` a setter for each, because a mode
you turn on is a mode you owe the terminal back.

## 4. The switches a TUI actually flips

| Mode | Number | What it does | uncurses |
| --- | --- | --- | --- |
| Alternate screen | 1049 | A fresh, scrollback-free buffer for the UI; leaving restores the shell exactly as it was | `set_alt_screen` |
| Cursor visibility | 25 | Hide the blinking cursor while you paint | `set_cursor_visible` |
| Mouse reporting | 1000 / 1006 | Report clicks and motion as input bytes, in a sane encoding | `set_mouse_mode` |
| Bracketed paste | 2004 | Wrap pasted text so you can tell it apart from typing | `set_bracketed_paste` |
| Focus events | 1004 | Tell you when the window gains or loses focus | `set_focus_events` |

Trace one. `screen.set_alt_screen(true)` stages the bytes `ESC[?1049h`. On
flush they reach the terminal, which swaps to the alternate buffer. On
exit the matching `ESC[?1049l` swaps back, and the shell prompt is sitting
right where the user left it. Skip that second sequence and you have
quietly hijacked their terminal.

## 5. The loop

With raw mode on and the modes set, a TUI is a loop:

1. Draw into an in-memory grid of cells. uncurses diffs it against the
   last frame and writes only the bytes that changed (`Screen`).
2. Read input bytes and decode them into typed `Event` values: a key, a
   mouse click, a paste, a resize, a query reply (`EventSource`).
3. React, redraw, repeat.

Decoding is section 1 run backwards. The byte `0x03` surfaces as the key
you would spell `"ctrl+c"`, `ESC[A` becomes an up-arrow `KeyPress`, and an
`ESC[<...M` blob becomes a mouse event. You match on `Event`, never on raw
bytes.

## 6. Always put it back

Terminal state is global and sticky. The modes you set outlive your
process: crash without cleaning up and the user is staring at a dead
prompt with no echo and no cursor. Teardown is not a nicety, and it is the
exact reverse of setup.

uncurses makes it two lines. `Screen::reset` emits the off-sequence for
every mode the screen turned on, and `Terminal::restore` returns the line
discipline to where `make_raw` found it. Run both even when your loop bails
out with an error. A clean exit on the worst day is the difference between
a tool people trust and one they uninstall.

## Keep going

- The [tutorial](tutorial.md) turns all of this into a working app.
- The [uncurses README](../uncurses/README.md) maps the rest of the API.
