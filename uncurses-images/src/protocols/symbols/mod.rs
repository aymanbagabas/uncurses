//! Ordinary text approximations of images.

mod catalog;
mod render;
mod selection;
#[cfg(test)]
mod tests;

pub use selection::SymbolSet;
use selection::Tags;
