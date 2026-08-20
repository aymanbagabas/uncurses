---
title: "Capabilities"
weight: 10
---

A terminal will tell you about itself if you ask. `Capabilities` is where
[`Program`]({{< relref "program.md" >}}) keeps those answers, exactly as the
terminal gave them rather than boiled down to a verdict.

Asking is always something you do on purpose, and the answers come back as
ordinary events that `Program` notices on the way past. [Querying the
terminal]({{< relref "../guides/querying-the-terminal.md" >}}) covers how to ask
and how long to wait.

## Answers, not verdicts

"The terminal said no" and "the terminal never replied" are different things,
and a plain yes-or-no cannot tell them apart. Silence is common: terminals are
free to ignore a question they do not recognize. So anything that might go
unanswered comes back as an `Option`, and `None` always means you never heard
back.

```rust
use uncurses::ansi::mode::{Mode, ModeSetting};
use uncurses::program::Program;

fn main() -> std::io::Result<()> {
    let mut program = Program::stdio()?;
    program.init()?;
    program.query_capabilities(&[])?;
    // ... read events until the answers arrive ...

    let caps = program.capabilities();

    match caps.mode(Mode::SYNCHRONIZED_OUTPUT) {
        Some(ModeSetting::NotRecognized) => {}    // a definite no
        Some(ModeSetting::PermanentlyReset) => {} // known, but never usable
        Some(_) => {}                             // supported, in some form
        None => {}                                // never answered
    }

    // Or just the yes-or-no question.
    let _pixels = caps.supports(Mode::MOUSE_SGR_PIXEL);

    program.finish()
}
```

`supports()` collapses the five states into the one answer most callers want.
It is `true` where the mode is usable, so `PermanentlyReset` reads as a no: the
terminal knows the mode and will never let it be set.

A few answers are a plain `bool`, because for those the silence is the answer: a
terminal that does not support the feature simply never responds. The
[`Capabilities` API
reference](/api/uncurses/program/struct.Capabilities.html) lists everything that
gets recorded.

## One kind of question

Because it holds answers, `Capabilities` tells you one thing only: what the
terminal said. Anything that has to weigh a reply against other evidence lives
elsewhere.

Color is the case worth knowing. Whether the terminal handles full color is
often clear from the environment alone, without asking it anything, so the
answer lives with the [color]({{< relref "color.md" >}}) settings on the screen,
which take everything into account. What the terminal actually replied stays
here, for when you want to know how that conclusion was reached.

## What the terminal can tell you

Every reply that says what it is about lands here, not just the answers the
program acts on, so a question you send yourself is readable afterwards
alongside the built-in ones. Where there are many answers of a kind, such as
palette entries, you can read the whole set.

A DECRQSS setting report is the reply that cannot. A success repeats the setting
and its parameters together, so `0;1m` and `>4;2m` carry nothing that could key
a record, and a refusal is empty. Only the request you sent says what was asked.
`Program` never sends DECRQSS, so it has no request to match against and hands
the reply through as `Event::SettingReport` untouched.

Sizes are the other exception, because they keep changing. The window and cell
dimensions arrive as replies too, but they are superseded by every resize, so
they live on `Program` as `window_cells()`, `window_pixels()` and
`cell_pixels()` rather than as a recorded answer.

The other half of what a program knows about its surroundings never arrives as
an answer at all. `program.env()` reads the environment, so `TERM`,
`COLORTERM`, and `TERM_PROGRAM` are readable without reaching for `std::env`.
`program.terminal()` gets you the terminal itself, for its size. Both are
read-only: an environment is only ever read, and the terminal is the program's
to change, since it keeps its own record of what it changed so it can put
everything back.

## Answers that keep arriving

Most questions are answered once. Three keep updating. The color scheme follows
the user switching between dark and light mode, and terminal visibility follows
the view being covered or uncovered, for as long as you leave those updates
turned on, and graphics support is confirmed by any graphics response,
including the ones a terminal sends back while you are transmitting an image.

The colors recorded here are the terminal's own, which is the opposite of what
`Program` remembers. Set the background color and `capabilities()` still reports
what the terminal originally told you, while the value you sent is what
`Program` knows to undo.

## Settings that wait for an answer

Some startup settings hold off until the terminal confirms it supports the
feature, so they do nothing until you ask. The ones that switch on a terminal
mode are switched on the same way you would switch them on yourself, so
teardown puts them back like anything else. Synchronized output is the
exception: adopting it sets a render property rather than a mode, so there is
nothing to put back.

You can turn this off, in which case the answer is still recorded but acting on
it is left to you. [Querying the
terminal]({{< relref "../guides/querying-the-terminal.md" >}}) has the details.
