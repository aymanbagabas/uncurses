---
title: "Keyboard input"
weight: 4
---

Keyboard input arrives as `Event::KeyPress` carrying a `Key`. A `Key` has three
parts: a `code` (which key), `modifiers` (Ctrl, Alt, Shift, and friends), and the
`text` it would produce. You can match keys two ways, and turn on the kitty
protocol when you need releases, repeats, and disambiguation.

## Matching keys the easy way

`Key` parses from strings, so the readable way to check for a shortcut is to
parse it once and compare. Equality compares the canonical chord, so this is
exactly right for keybindings.

```rust
use uncurses::event::{Event, Key};

let quit: Key = "ctrl+c".parse().unwrap();
let help: Key = "f1".parse().unwrap();

match screen.read_event()? {
    Event::KeyPress(ref k) if *k == quit => return Ok(()),
    Event::KeyPress(ref k) if *k == help => show_help(),
    _ => {}
}
```

The grammar covers plain characters (`"q"`), named keys (`"enter"`, `"esc"`,
`"up"`, `"f1"`), and modifier chords (`"ctrl+c"`, `"alt+shift+left"`). Collect a
set of bindings into an array and ask `contains`:

```rust
let quit: [Key; 3] = ["q", "esc", "ctrl+c"].map(|s| s.parse().unwrap());
// ...
Event::KeyPress(ref k) if quit.contains(k) => break,
```

## Matching keys structurally

When you want to branch on the key itself, match on `KeyCode` and inspect
`modifiers`. `modifiers` is a bitset, so test it with `contains` and
`is_empty`.

```rust
use uncurses::event::{Event, Key, KeyCode, KeyModifiers};

match screen.read_event()? {
    Event::KeyPress(Key { code: KeyCode::Char('q'), modifiers, .. })
        if modifiers.is_empty() => break,
    Event::KeyPress(Key { code: KeyCode::Char('c'), modifiers, .. })
        if modifiers.contains(KeyModifiers::CTRL) => break,
    Event::KeyPress(Key { code: KeyCode::Left, .. }) => move_left(),
    _ => {}
}
```

The modifier flags are `SHIFT`, `ALT`, `CTRL`, `META`, `HYPER`, `SUPER`,
`CAPS_LOCK`, and `NUM_LOCK`.

A `Key` carries `text` only for printable input: a character typed on its own.
Shift, Caps Lock, and Num Lock keep it; Ctrl, Alt, and named keys (`Enter`,
arrows, function keys) clear it. So a present `text` means the key would type
something, already in the right layout (`"!"` for `shift+1`), which is what you
want for text entry rather than the physical `code`.

```rust
if let Event::KeyPress(k) = screen.read_event()? {
    if let Some(text) = &k.text {
        buffer.push_str(text); // a printable key: insert what it typed
    }
}
```

## Presses, repeats, and releases

By default a terminal reports each key once, as a press, and that is all most
apps need. The kitty keyboard protocol can report more, but what you get depends
on which flags you turn on with `set_kitty_keyboard`. Pass `Some(flags)` to
enable the bits you want, or `None` to switch every enhancement back off.

```rust
use uncurses::ansi::KittyKeyboardFlags;

screen.set_kitty_keyboard(Some(
    KittyKeyboardFlags::DISAMBIGUATE_ESCAPE_CODES
        | KittyKeyboardFlags::REPORT_EVENT_TYPES,
))?;

// ... now your loop can also see, for keys sent as escape codes:
//   Event::KeyRepeat(k)
//   Event::KeyRelease(k)
```

The flags stack, and each one buys a specific thing:

- `DISAMBIGUATE_ESCAPE_CODES` makes keys that the legacy encoding blurs together
  distinct: `Esc` on its own, `Ctrl+I` versus `Tab`, and `Ctrl+M` versus
  `Enter` all become unambiguous escape codes.
- `REPORT_EVENT_TYPES` adds `KeyRepeat` and `KeyRelease`, but only for keys the
  terminal sends as escape codes: arrows, function keys, and modifier chords.
  Keys that produce text are still delivered as plain UTF-8, so a held letter
  keeps arriving as repeated `KeyPress` events with no release.
- `REPORT_ALL_KEYS_AS_ESCAPE` reports every key as an escape code, printable
  ones included. Combine it with `REPORT_EVENT_TYPES` when you want repeats and
  releases for letters and digits too, for instance to track a held `w` in a
  game.
- `REPORT_ALTERNATE_KEYS` and `REPORT_ASSOCIATED_TEXT` add the shifted and base
  layout key codes and the produced text to each report.

A terminal that does not speak the protocol ignores the request, so you keep
getting plain presses and it is safe to ask unconditionally.

See the `keylog` example for a live readout of every key, modifier, repeat, and
release.
