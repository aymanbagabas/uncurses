---
title: Documentation
weight: 1
sidebar:
  open: true
---

uncurses is a terminal toolkit library for Rust. It gives you the building
blocks for terminal UIs, then gets out of the way: you own the event loop and
decide when bytes are written. No terminfo, no widget tree, no hidden global
state.

These docs are organized into three parts. If you are new, read **Getting
Started** from top to bottom. Come back to **Concepts** when you want the
reasoning behind the model, and use **Guides** when you have a specific task.

{{< cards >}}
  {{< card link="getting-started/" title="Getting Started" subtitle="Install uncurses, write hello world, and learn the layers." >}}
  {{< card link="concepts/" title="Concepts" subtitle="Terminals, cells, buffers, width, surfaces, events, and the screen." >}}
  {{< card link="guides/" title="Guides" subtitle="How-to walkthroughs for inline rendering, mouse input, paste handling, async events, and more." >}}
  {{< card link="/api/uncurses/" title="API reference" subtitle="Generated rustdoc for all public modules and types." >}}
{{< /cards >}}
