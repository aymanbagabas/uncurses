//! OSC 8 hyperlink support.

/// An OSC 8 hyperlink attached to a cell.
///
/// An empty [`url`](Self::url) represents "no hyperlink"; this matches
/// the OSC 8 wire encoding, where `OSC 8 ; ; ST` (empty URL) closes
/// the current hyperlink.
#[derive(Debug, Default, Clone, PartialEq, Eq, Hash)]
pub struct Link {
    /// The URL target. Empty when no hyperlink is set.
    pub url: String,
    /// Optional parameters (e.g., `id=...`).
    pub params: String,
}

impl Link {
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            params: String::new(),
        }
    }

    /// An empty link (no hyperlink). Equivalent to [`Link::default`]
    /// and usable in `const` contexts.
    pub const EMPTY: Self = Self {
        url: String::new(),
        params: String::new(),
    };

    pub fn with_params(mut self, params: impl Into<String>) -> Self {
        self.params = params.into();
        self
    }

    /// True when no hyperlink is set (empty URL).
    pub fn is_empty(&self) -> bool {
        self.url.is_empty()
    }
}
