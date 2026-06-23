---
title: Guides
weight: 3
sidebar:
  open: true
---

Task-focused, how-to walkthroughs for common jobs. Each guide is grounded in a
runnable example in the workspace.

{{< cards >}}
  {{< card link="inline-rendering" title="Inline rendering" subtitle="Draw in the normal buffer without taking over the screen." >}}
  {{< card link="mouse-input" title="Mouse input" subtitle="Clicks, motion, the wheel, and pixel-accurate tracking." >}}
  {{< card link="styling-text" title="Styling text" subtitle="Colors, attributes, and OSC 8 hyperlinks." >}}
  {{< card link="keyboard-input" title="Keyboard input" subtitle="Parse and match keys, from plain chars to kitty chords." >}}
  {{< card link="handling-paste" title="Handling paste" subtitle="Capture pasted text and spill large pastes to disk." >}}
  {{< card link="querying-the-terminal" title="Querying the terminal" subtitle="Ask for the background color, cell size, and more." >}}
  {{< card link="async-events" title="Async event loops" subtitle="Drive input through a futures Stream." >}}
  {{< card link="pause-and-resume" title="Pause and resume" subtitle="Shell out to $EDITOR and come back cleanly." >}}
  {{< card link="offscreen-rendering" title="Offscreen rendering" subtitle="Render to a buffer for snapshots and transcripts." >}}
  {{< card link="ratatui-backend" title="The ratatui backend" subtitle="Render ratatui widgets through uncurses." >}}
{{< /cards >}}
