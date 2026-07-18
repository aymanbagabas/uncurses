---
title: Documentation
weight: 1
sidebar:
  open: true
---

uncurses is a Rust library for building terminal user interfaces. It provides a
direct, framework-free way to draw to the terminal and read input, giving you
control over every cell and your own event loop, whether you run inline, take
over the full screen, mix the two, or leave the console unmanaged and just
shape your output. No terminfo, no widget tree, no hidden global state.

These docs are organized into three parts. If you are new, read **Getting
Started** from top to bottom. Come back to **Concepts** when you want the
reasoning behind the model, and use **Guides** when you have a specific task.

{{< cards >}}
  {{< card link="getting-started/" title="Getting Started" subtitle="Install uncurses, write hello world, and learn the layers." >}}
  {{< card link="concepts/" title="Concepts" subtitle="Terminals, cells, buffers, width, surfaces, events, and the screen." >}}
  {{< card link="guides/" title="Guides" subtitle="How-to walkthroughs for inline rendering, mouse input, paste handling, async events, and more." >}}
  {{< card link="/api/uncurses/" title="API reference" subtitle="Generated rustdoc for all public modules and types." >}}
{{< /cards >}}
