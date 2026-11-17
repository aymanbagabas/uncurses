---
title: Features
description: "Explore rendering, input, and terminal integration, with links to concepts, guides, and API details."
weight: 1
---

Explore the features listed in the [project README](https://github.com/aymanbagabas/uncurses/blob/main/README.md#features).
The links below lead to concepts, practical guides, and API details.

Feature support depends on the terminal.
Use [capability queries]({{< relref "guides/querying-the-terminal.md" >}}) when your application needs to check support.

## Drawing and screens

| Feature | Documentation |
| --- | --- |
| Diff rendering compares frames and updates the terminal. | [Screen]({{< relref "concepts/screen.md" >}}) |
| ANSI placements combine opaque output and text in one frame. | [Placements]({{< relref "guides/placements.md" >}}) |
| Draw inline or on the alt screen, with pause and resume. | [Inline rendering]({{< relref "guides/inline-rendering.md" >}}), [pause and resume]({{< relref "guides/pause-and-resume.md" >}}) |
| Truecolor adapts to 256 colors, ANSI colors, or monochrome. | [Color profiles]({{< relref "concepts/color.md#profiles-and-downsampling" >}}) |
| Styled underlines include curly, dotted, dashed, and double shapes. | [Building a style]({{< relref "guides/styling-text.md#building-a-style" >}}) |
| Clickable hyperlinks attach destinations to text. | [Hyperlinks]({{< relref "guides/styling-text.md#hyperlinks" >}}) |
| Unicode Core support measures text by grapheme cluster. | [Text width]({{< relref "concepts/width.md" >}}), [grapheme mode API](/api/uncurses/program/struct.Program.html#method.enable_grapheme_clusters) |
| Synchronized output groups frame updates to prevent tearing. | [Capability probing]({{< relref "guides/querying-the-terminal.md#capability-probing" >}}) |

## Input and events

| Feature | Documentation |
| --- | --- |
| Enhanced keyboard input supports Kitty keyboard and modifyOtherKeys. | [Keyboard input]({{< relref "guides/keyboard-input.md" >}}) |
| Win32 input mode reports key presses, repeats, and releases. | [Keyboard input]({{< relref "guides/keyboard-input.md" >}}), [Win32 input mode API](/api/uncurses/ansi/mode/enum.Mode.html#associatedconstant.WIN32_INPUT) |
| Mouse input includes clicks, motion, the wheel, and pixel positions. | [Mouse input]({{< relref "guides/mouse-input.md" >}}) |
| Pointer shapes change the mouse cursor. | [Mouse input]({{< relref "guides/mouse-input.md" >}}), [pointer shape API](/api/uncurses/program/struct.Program.html#method.set_pointer_shape) |
| Bracketed paste separates pasted text from individual keystrokes. | [Handling paste]({{< relref "guides/handling-paste.md" >}}) |
| Focus events report when the terminal gains or loses focus. | [Events]({{< relref "concepts/events.md" >}}), [focus reporting API](/api/uncurses/program/struct.Program.html#method.enable_focus_events) |
| In-band resize reports window changes through the event stream. | [Events]({{< relref "concepts/events.md" >}}), [in-band resize API](/api/uncurses/program/struct.Program.html#method.enable_in_band_resize) |
| Typed events cover keys, mouse input, paste, focus, and resize. | [Reading events]({{< relref "concepts/events.md#reading-through-program" >}}) |

## Terminal integration

| Feature | Documentation |
| --- | --- |
| Capability discovery asks the terminal what it supports. | [Capabilities]({{< relref "concepts/capabilities.md" >}}), [querying the terminal]({{< relref "guides/querying-the-terminal.md" >}}) |
| Visibility reports let applications pause work while hidden. | [Events]({{< relref "concepts/events.md" >}}), [visibility reporting API](/api/uncurses/program/struct.Program.html#method.enable_visibility_reports) |
| Read, set, and reset foreground, background, cursor, and palette colors. | [Terminal queries]({{< relref "guides/querying-the-terminal.md#asking-one-question" >}}), [color controls API](/api/uncurses/program/struct.Program.html#method.set_palette_color) |
| Color scheme updates follow changes between light and dark. | [Capability updates]({{< relref "concepts/capabilities.md#answers-that-keep-arriving" >}}), [color scheme API](/api/uncurses/program/struct.Program.html#method.enable_color_scheme_updates) |
| Read and write the system and primary clipboards. | [Terminal queries]({{< relref "guides/querying-the-terminal.md#asking-one-question" >}}), [clipboard API](/api/uncurses/program/struct.Program.html#method.set_system_clipboard) |
| Drag and drop exchanges messages, formats, and data with the desktop. | [Drag and drop]({{< relref "guides/drag-and-drop.md" >}}) |
| Set the window title and icon name. | [Program]({{< relref "concepts/program.md" >}}), [title controls API](/api/uncurses/program/struct.Program.html#method.set_window_title) |
| Taskbar progress reports progress and activity states. | [Progress lifetime]({{< relref "guides/program-status.md#record-lifetime" >}}), [taskbar progress API](/api/uncurses/program/struct.Program.html#method.set_progress_state) |
| Program status reports task states, messages, and child records. | [Program status]({{< relref "guides/program-status.md" >}}) |
