use std::io;

use super::Cell;

const PREFIX: &str = "\0p1:";

impl Cell {
    /// Whether this cell holds a screen-local placement reference.
    ///
    /// A placement occupies one grid slot. Its content is private metadata.
    #[inline]
    pub(crate) fn is_placement(&self) -> bool {
        self.width == 0 && self.content.starts_with("\0p")
    }

    pub(crate) fn placement(id: u64) -> Self {
        Self::new(format!("{PREFIX}{id:016x}"), 0)
    }

    pub(crate) fn placement_ref(&self) -> io::Result<Option<u64>> {
        if !self.is_placement() {
            return Ok(None);
        }
        let invalid = || io::Error::new(io::ErrorKind::InvalidInput, "invalid p1 placement cell");
        let encoded = self.content.strip_prefix(PREFIX).ok_or_else(invalid)?;
        if encoded.len() != 16 || !encoded.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(invalid());
        }
        let id = u64::from_str_radix(encoded, 16).map_err(|_| invalid())?;
        if id == 0 {
            return Err(invalid());
        }
        Ok(Some(id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_only_markers_round_trip_inline() {
        for id in [1, u64::from(u32::MAX), u64::MAX] {
            let cell = Cell::placement(id);
            assert_eq!(cell.placement_ref().unwrap(), Some(id));
            assert_eq!(cell.width(), 1);
            assert!(!cell.is_continuation());
            if cfg!(target_pointer_width = "64") {
                assert!(!cell.content.is_heap_allocated());
            }
        }
        for value in ["\0p2:0000000000000001", "\0p1:0000000000000000", "\0p1:x"] {
            assert!(Cell::new(value, 0).placement_ref().is_err());
        }
        assert!(Cell::new("", 0).is_continuation());
        assert_eq!(Cell::new("\0p1:literal", 1).placement_ref().unwrap(), None);
    }
}
