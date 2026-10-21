//! Drag an image in the alt screen.
//!
//! Run `cargo run -p examples --example images_drag`.
//! Append `-- image.png` to load a file instead of the generated pattern.
//! Drag with the left mouse button. Keys 0-5 select Auto, symbols,
//! iTerm2, Sixel, Kitty native, and Kitty placeholders.
//! Tab cycles protocols; s cycles symbol sets; d cycles dithers; f cycles fit modes.
//! Native selections require a compatible terminal. Press q or Esc to quit.

#[path = "support/image_drag.rs"]
mod image_drag;

fn main() -> std::io::Result<()> {
    image_drag::run(false)
}
