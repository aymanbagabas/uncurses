//! Image representations and backend implementations.

mod capabilities;
pub(crate) mod iterm2;
pub(crate) mod kitty;
pub(crate) mod kitty_placeholders;
mod kitty_relative;
pub(crate) mod sixel;
mod symbols;

pub use capabilities::ImageCapabilities;
pub use kitty_placeholders::Placeholders;
pub use kitty_relative::ImageGroup;
pub use symbols::SymbolSet;

/// A terminal image representation.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Protocol {
    /// Select a detected graphics protocol, otherwise use default symbols.
    ///
    /// Kitty uses Unicode placeholders with true color and at most 297 cells
    /// per dimension, or native placements otherwise. This assumes Unicode
    /// support when Kitty graphics support is detected.
    #[default]
    Auto,
    /// Ordinary character cells from the selected symbol families.
    Symbols(SymbolSet),
    /// The iTerm2 inline image protocol.
    Iterm2,
    /// Sixel graphics with a quantized palette.
    Sixel,
    /// Native Kitty placements with shared terminal resources.
    Kitty,
    /// Kitty graphics through ordinary Unicode placeholder cells.
    ///
    /// Requires true color and terminal support for Kitty Unicode placeholders.
    /// Clear old cells explicitly when moving them.
    KittyPlaceholders,
}
