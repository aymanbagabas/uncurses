---
title: Documentation
weight: 1
sidebar:
  open: true
---

uncurses is a low-level terminal library for Rust. It hands you the building
blocks to build a terminal UI and then steps out of the way: you own the
event loop and decide when bytes hit the wire. No terminfo, no widget tree,
no hidden global state.

These docs are organized in four parts. If you are new, read **Getting
Started** top to bottom; come back to **Concepts** when you want the why,
reach for **Guides** when you have a specific job to do, and dip into
**Explanations** for the background.

{{< cards >}}
  {{< card link="getting-started" title="Getting Started" subtitle="Install, write hello-world, learn the four layers." >}}
  {{< card link="concepts" title="Concepts" subtitle="Screen, Canvas, events, styling, text, and more." >}}
  {{< card link="guides" title="Guides" subtitle="How-to walkthroughs for inline, mouse, paste, async, and more." >}}
  {{< card link="explanations" title="Explanations" subtitle="How terminals work, the rendering pipeline, and design principles." >}}
  {{< card link="examples" title="Examples" subtitle="Runnable demos grouped by use case." >}}
  {{< card link="../api/" title="API reference" subtitle="Generated rustdoc for every module and type." >}}
{{< /cards >}}
