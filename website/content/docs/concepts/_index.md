---
title: Concepts
weight: 2
sidebar:
  open: true
---

The core ideas behind uncurses, one per page. Read these to understand how
the pieces fit together and why the API is shaped the way it is.

{{< cards >}}
  {{< card link="screen" title="The Screen facade" subtitle="Lifecycle, inline vs fullscreen, options, and teardown." >}}
  {{< card link="canvas-and-rendering" title="Canvas and rendering" subtitle="Cells, the diffing renderer, and render/flush/present." >}}
  {{< card link="events-and-input" title="Events and input" subtitle="The decode pipeline, keys, mouse, paste, resize, and queries." >}}
  {{< card link="styling-and-color" title="Styling and color" subtitle="Style, SGR, hyperlinks, and graceful color downsampling." >}}
  {{< card link="text-and-width" title="Text and width" subtitle="Grapheme clusters, wide cells, and measuring text." >}}
  {{< card link="terminal" title="The terminal handle" subtitle="Raw mode, window size, and talking to the tty." >}}
  {{< card link="geometry" title="Geometry" subtitle="Position, Size, and Rect on the cell grid." >}}
  {{< card link="capabilities-and-queries" title="Capabilities and queries" subtitle="Asking the terminal what it can do, instead of guessing." >}}
{{< /cards >}}
