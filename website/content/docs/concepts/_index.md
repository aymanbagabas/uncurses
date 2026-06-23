---
title: Concepts
weight: 2
sidebar:
  open: true
---

The core ideas behind uncurses live here, one per page. Each page stands on its
own and links to related concepts, so you can start with the terminal itself or
jump straight to whatever you need.

{{< cards >}}
  {{< card link="terminals" title="Terminals" subtitle="TTYs, PTYs, cooked versus raw, and where uncurses plugs in." >}}
  {{< card link="cells" title="Cells" subtitle="The atomic slot: content, style, and how many columns it fills." >}}
  {{< card link="buffers" title="Buffers" subtitle="An off-screen grid of cells you paint before anything is shown." >}}
  {{< card link="width" title="Width" subtitle="Why one character is not always one column, and why it matters." >}}
  {{< card link="color" title="Color" subtitle="Color depths and how uncurses downsamples to the terminal." >}}
  {{< card link="surfaces" title="Surfaces" subtitle="The shared traits that let you draw once and reuse the code." >}}
  {{< card link="events" title="Events" subtitle="Turning the raw input stream into typed key, mouse, and terminal reply events." >}}
  {{< card link="screen" title="Screen" subtitle="The facade that unites drawing, input, the terminal, and the renderer." >}}
{{< /cards >}}
