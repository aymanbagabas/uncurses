---
title: "Images"
weight: 9
---

`uncurses-image` adds pictures to a terminal interface. An `Image` holds shared
pixels. Its options control how those pixels fit the space you choose.
Keep the image across frames to reuse its data.

The crate can draw pictures as native terminal graphics or as ordinary cells.
Native graphics preserve more detail. Symbols approximate pictures with ordinary
text and provide a fallback when graphics support is unknown.
Choose blocks, Braille, letters, or a mixture to control their appearance.

Native images use the screen's regions. Later text can replace part of an image.
Removing the image restores the text beneath it. Both inline and fullscreen
screens use the same methods.

Kitty placeholders represent pictures with ordinary cells. They move with text
and can also anchor a group of relative images. When the last anchor disappears,
the dependent images disappear too.

See the [image guide]({{< relref "../guides/images.md" >}}) for protocol selection,
fit modes, text overlays, and relative groups.
