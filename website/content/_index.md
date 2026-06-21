---
title: uncurses
layout: hextra-home
---

{{< hextra/hero-badge >}}
  <div class="hx:w-2 hx:h-2 hx:rounded-full hx:bg-primary-400"></div>
  A terminal library for Rust
{{< /hextra/hero-badge >}}

<div class="hx:mt-6 hx:mb-6">
{{< hextra/hero-headline >}}
  Build terminal UIs,&nbsp;<br class="hx:sm:block hx:hidden" />without the curses
{{< /hextra/hero-headline >}}
</div>

<div class="hx:mb-12">
{{< hextra/hero-subtitle >}}
  A low-level, modern, VT100/xterm-style terminal library.&nbsp;<br class="hx:sm:block hx:hidden" />
  You own the event loop; it makes the bytes correct and minimal.
{{< /hextra/hero-subtitle >}}
</div>

<div class="hx:mb-6">
{{< hextra/hero-button text="Get Started" link="docs" >}}
</div>

{{< hextra/feature-grid >}}
  {{< hextra/feature-card title="Layered, not a framework"
    subtitle="Reach for Screen to ship fast, or drop to Canvas, EventSource, and Terminal. Nothing is hidden." >}}
  {{< hextra/feature-card title="Cell-diffing renderer"
    subtitle="Render to a terminal, or any Write sink, and ship only the bytes that changed." >}}
  {{< hextra/feature-card title="Typed events"
    subtitle="Keys, mouse, paste, focus, resize, and query replies, decoded from the raw byte soup." >}}
  {{< hextra/feature-card title="Degrades gracefully"
    subtitle="24-bit color downsamples to 256/16/none; you write true color once." >}}
  {{< hextra/feature-card title="Inline or fullscreen"
    subtitle="Starts inline in the normal buffer; opt into the alternate screen when you want it." >}}
  {{< hextra/feature-card title="Async optional"
    subtitle="A runtime-agnostic futures Stream of events, behind a feature flag." >}}
{{< /hextra/feature-grid >}}
