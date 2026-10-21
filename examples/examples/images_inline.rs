//! Drag an image in the normal screen, below existing terminal output.
//!
//! Run `cargo run -p examples --example images_inline`.
//! Append `-- image.png` to load a file instead of the generated pattern.
//! The viewport uses up to 16 rows. Drag with the left mouse button.
//! Space inserts a log line above the viewport. Keys 0-5 select Auto, symbols,
//! iTerm2, Sixel, Kitty native, and Kitty placeholders.
//! Tab cycles protocols; s cycles symbol sets; d cycles dithers; f cycles fit modes.
//! Native selections require a compatible terminal. Press q or Esc to quit.

#[path = "support/image_drag.rs"]
mod image_drag;

fn main() -> std::io::Result<()> {
    image_drag::run(true)
}
