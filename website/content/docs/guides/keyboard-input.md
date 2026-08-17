---
title: "Keyboard input"
weight: 4
---

Keyboard input arrives as key events. A press is `Event::KeyPress` carrying a
`Key`; with the kitty keyboard protocol enabled, repeats and releases arrive as
`Event::KeyRepeat` and `Event::KeyRelease`. The `Key` fields you usually use are
`code` (which key), `modifiers` (Ctrl, Alt, Shift, and friends), and the optional
`text` it would produce.

{{< callout type="info" >}}
`Program` owns input, terminal modes, and capability tracking. Draw through
`program.screen_mut()`, but read through `program.read_event()` or
`program.try_read_event()?`. The sync readers observe events automatically, so
capability replies and resize tracking stay current as you read. If you take
events from `program.event_stream()`, feed each one to `program.observe_event(&ev)?`
as shown in the async guide.
{{< /callout >}}

## Matching keys the easy way

For keybindings, check the shortcut text directly with `matches` or
`matches_any`.

```rust
use uncurses::event::Event;

let ev = program.read_event()?;

match ev {
    Event::KeyPress(ref k) if k.matches("ctrl+c") => return Ok(()),
    Event::KeyPress(ref k) if k.matches("f1") => show_help(),
    _ => {}
}
```

The grammar covers plain characters (`"q"`), named keys (`"enter"`, `"esc"`,
`"up"`, `"f1"`), and modifier chords (`"ctrl+c"`, `"alt+shift+left"`). Do not
compare `k.to_string()` to binding strings; display is structural. Under the
kitty keyboard protocol, a shifted glyph like `}` may display as `shift+]`, while
`k.matches("}")` still matches the produced text.

```rust
let ev = program.read_event()?;

match ev {
    Event::KeyPress(ref k) if k.matches_any(["q", "esc", "ctrl+c"]) => break,
    _ => {}
}
```

## Matching keys structurally

When you want to branch on the key itself, match on `KeyCode` and inspect
`modifiers`. `modifiers` is a bitset, so test it with `contains` and
`is_empty`.

```rust
use uncurses::event::{Event, Key, KeyCode, KeyModifiers};

let ev = program.read_event()?;

match ev {
    Event::KeyPress(Key { code: KeyCode::Char('q'), modifiers, .. })
        if modifiers.is_empty() => break,
    Event::KeyPress(Key { code: KeyCode::Char('c'), modifiers, .. })
        if modifiers.contains(KeyModifiers::CTRL) => break,
    Event::KeyPress(Key { code: KeyCode::Left, .. }) => move_left(),
    _ => {}
}
```

{{< callout type="warning" >}}
Space is its own variant, `KeyCode::Space`, not `KeyCode::Char(' ')`. Match on
`KeyCode::Space` (it still reports `text` of `" "`); a `Char(' ')` arm never
fires. The other printable-but-named keys follow the same rule: `Enter`, `Tab`,
and `Backspace` are `KeyCode::Enter` / `Tab` / `Backspace`, not `Char('\n')` /
`Char('\t')` / `Char('\x7f')`.

Rust has no variant aliases, so you cannot make one arm match both. When you
care about the typed character rather than the physical key, use `key.char()`,
which returns `Some(' ')` for both `Space` and `Char(' ')`, or check
`key.text`.
{{< /callout >}}

The modifier flags are `SHIFT`, `ALT`, `CTRL`, `META`, `HYPER`, `SUPER`,
`CAPS_LOCK`, and `NUM_LOCK`.

A `Key` carries `text` only for printable input: a character or space with no
modifiers other than Shift, Caps Lock, or Num Lock. Ctrl, Alt, Meta, Hyper, Super,
and named keys (`Enter`, arrows, function keys) clear it. So a present `text`
means the key would type something, already in the right layout (`"!"` for
`shift+1`), which is what you want for text entry rather than the physical
`code`.

```rust
let ev = program.read_event()?;

if let Event::KeyPress(k) = ev {
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
use uncurses::ansi::kitty::KittyKeyboardFlags;

program.set_kitty_keyboard(Some(
    KittyKeyboardFlags::DISAMBIGUATE_ESCAPE_CODES
        | KittyKeyboardFlags::REPORT_EVENT_TYPES,
))?;

// ... now your loop can also see, for keys sent as escape codes:
//   Event::KeyRepeat(k)
//   Event::KeyRelease(k)
```

The flags combine, and each one enables a specific behavior:

- `DISAMBIGUATE_ESCAPE_CODES` makes keys that the legacy encoding blurs together
  distinct: `Esc` on its own, `Ctrl+I` versus `Tab`, and `Ctrl+M` versus
  `Enter` all become unambiguous escape codes.
- `REPORT_EVENT_TYPES` adds `KeyRepeat` and `KeyRelease`, but only for keys the
  terminal sends as escape codes: arrows, function keys, and modified keys that
  are not sent as text. Keys that produce text are still delivered as plain
  UTF-8, so a held letter keeps arriving as repeated `KeyPress` events with no
  release.
- `REPORT_ALL_KEYS_AS_ESCAPE` reports every key as an escape code, including
  printable keys. Combine it with `REPORT_EVENT_TYPES` when you want repeats and
  releases for letters and digits too, for instance to track a held `w` in a
  game.
- `REPORT_ALTERNATE_KEYS` adds shifted and base key values.
- `REPORT_ASSOCIATED_TEXT` adds the produced text.

A terminal that does not speak the protocol ignores the request, so you keep
getting plain presses and it is safe to ask unconditionally.

See the `keylog` example in `examples/examples/` for a live readout of every key,
modifier, repeat, and release.
