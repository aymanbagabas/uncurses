---
title: uncurses
layout: hextra-home
---

{{< hextra/hero-badge >}}
  <div class="hx:w-2 hx:h-2 hx:rounded-full hx:bg-primary-400"></div>
  A terminal toolkit library for Rust
{{< /hextra/hero-badge >}}

<div class="hx:mt-6 hx:mb-6">
{{< hextra/hero-headline >}}
  Build terminal UIs,&nbsp;<br class="hx:sm:block hx:hidden" />without the curses
{{< /hextra/hero-headline >}}
</div>

<div class="hx:mb-12">
{{< hextra/hero-subtitle >}}
  A modern, VT100/xterm-compatible terminal toolkit library.&nbsp;<br class="hx:sm:block hx:hidden" />
  You own the event loop; uncurses keeps the bytes correct and minimal.
{{< /hextra/hero-subtitle >}}
</div>

<div class="hx:mb-6">
{{< hextra/hero-button text="Get Started" link="/docs/getting-started/" >}}
</div>

{{< hextra/feature-grid >}}
  {{< hextra/feature-card title="Layered, not a framework"
    subtitle="Start with Screen, or grab TextBuffer, EventSource, and Terminal directly. Nothing is hidden." >}}
  {{< hextra/feature-card title="Cell-diffing renderer"
    subtitle="Screen diffs frames against the terminal and writes only the cells that changed." >}}
  {{< hextra/feature-card title="Typed events"
    subtitle="Keys, mouse, paste, focus, resize, and query replies decoded from raw terminal input." >}}
  {{< hextra/feature-card title="Degrades gracefully"
    subtitle="Write true color once; uncurses maps it to 256-color, ANSI, or plain text when needed." >}}
  {{< hextra/feature-card title="Inline or fullscreen"
    subtitle="Run in the normal buffer by default; switch to the alternate screen when you want it." >}}
  {{< hextra/feature-card title="Async when you want it"
    subtitle="Enable the async feature for a runtime-agnostic futures Stream of events." >}}
{{< /hextra/feature-grid >}}
