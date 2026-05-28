//! Bounded byte buffer with head/tail cursors used by the event
//! source's read path.
//!
//! The buffer is allocated once at construction to its full capacity
//! and never resizes. Read paths write into the spare slice at the
//! tail; parse paths consume from the head. The buffer compacts
//! lazily — only when the wasted prefix reaches half the capacity, or
//! when the tail has hit the end of the buffer and there is room to
//! shift the unread region down.
//!
//! This is a private helper for `event::source` / `event::source_windows`.

/// Fixed-capacity byte buffer with head/tail cursors.
///
/// `slice()` exposes the unread region; `spare_mut()` exposes a
/// writeable tail. The cursor pair removes the per-event `Vec::drain`
/// memmove from the parse loop, and `spare_mut` lets the caller pass
/// the writable slice straight to `Read::read` without a borrow conflict
/// with the surrounding `Source`.
pub(super) struct Pending {
    buf: Vec<u8>,
    head: usize,
    tail: usize,
}

#[allow(dead_code)] // some helpers are only used on Windows
impl Pending {
    pub(super) fn with_capacity(cap: usize) -> Self {
        Self {
            buf: vec![0u8; cap],
            head: 0,
            tail: 0,
        }
    }

    /// Total backing capacity (the hard cap on any single sequence).
    pub(super) fn capacity(&self) -> usize {
        self.buf.len()
    }

    /// Number of unread bytes currently buffered.
    pub(super) fn len(&self) -> usize {
        self.tail - self.head
    }

    pub(super) fn is_empty(&self) -> bool {
        self.head == self.tail
    }

    /// `true` when the unread region fills the entire backing buffer.
    pub(super) fn is_full(&self) -> bool {
        self.tail - self.head == self.buf.len()
    }

    /// Unread bytes as a contiguous slice.
    pub(super) fn slice(&self) -> &[u8] {
        &self.buf[self.head..self.tail]
    }

    /// First unread byte, or `None` if empty.
    pub(super) fn first(&self) -> Option<u8> {
        if self.is_empty() {
            None
        } else {
            Some(self.buf[self.head])
        }
    }

    /// Mark `n` leading bytes as consumed by the parser. Compacts
    /// lazily when the wasted prefix grows past half of capacity.
    pub(super) fn consume(&mut self, n: usize) {
        debug_assert!(self.head + n <= self.tail);
        self.head += n;
        if self.head == self.tail {
            self.head = 0;
            self.tail = 0;
        } else if self.head >= self.buf.len() / 2 {
            self.compact();
        }
    }

    /// Discard all buffered bytes.
    pub(super) fn clear(&mut self) {
        self.head = 0;
        self.tail = 0;
    }

    /// Writeable spare slice at the tail. Compacts first if the tail
    /// has reached the end of the backing buffer, so the returned
    /// slice always exposes the maximum currently-available room.
    pub(super) fn spare_mut(&mut self) -> &mut [u8] {
        if self.tail == self.buf.len() && self.head > 0 {
            self.compact();
        }
        &mut self.buf[self.tail..]
    }

    /// Commit `n` bytes written into the slice previously returned by
    /// [`Self::spare_mut`].
    pub(super) fn advance_written(&mut self, n: usize) {
        debug_assert!(self.tail + n <= self.buf.len());
        self.tail += n;
    }

    /// Append `bytes` to the tail, capped at available room. Returns
    /// how many bytes were actually appended. Compacts the buffer
    /// first if needed.
    pub(super) fn append(&mut self, bytes: &[u8]) -> usize {
        let spare = self.spare_mut();
        let n = bytes.len().min(spare.len());
        spare[..n].copy_from_slice(&bytes[..n]);
        self.tail += n;
        n
    }

    fn compact(&mut self) {
        self.buf.copy_within(self.head..self.tail, 0);
        self.tail -= self.head;
        self.head = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_state() {
        let p = Pending::with_capacity(8);
        assert_eq!(p.capacity(), 8);
        assert_eq!(p.len(), 0);
        assert!(p.is_empty());
        assert!(!p.is_full());
        assert_eq!(p.first(), None);
        assert_eq!(p.slice(), b"");
    }

    #[test]
    fn append_and_consume() {
        let mut p = Pending::with_capacity(8);
        assert_eq!(p.append(b"hello"), 5);
        assert_eq!(p.slice(), b"hello");
        assert_eq!(p.first(), Some(b'h'));
        p.consume(2);
        assert_eq!(p.slice(), b"llo");
        p.consume(3);
        assert!(p.is_empty());
    }

    #[test]
    fn append_caps_at_capacity() {
        let mut p = Pending::with_capacity(4);
        assert_eq!(p.append(b"abcdef"), 4);
        assert_eq!(p.slice(), b"abcd");
        assert!(p.is_full());
    }

    #[test]
    fn compacts_when_head_past_halfway() {
        let mut p = Pending::with_capacity(4);
        p.append(b"abcd");
        // Consume 2 (= half capacity): triggers compaction.
        p.consume(2);
        assert_eq!(p.slice(), b"cd");
        // Now appending exercises the spare room exposed by compaction.
        assert_eq!(p.append(b"ef"), 2);
        assert_eq!(p.slice(), b"cdef");
    }

    #[test]
    fn spare_mut_compacts_when_tail_at_end() {
        let mut p = Pending::with_capacity(4);
        p.append(b"abcd");
        p.consume(1);
        // tail == capacity, head > 0 → spare_mut compacts.
        let spare = p.spare_mut();
        assert_eq!(spare.len(), 1);
        spare[0] = b'e';
        p.advance_written(1);
        assert_eq!(p.slice(), b"bcde");
    }

    #[test]
    fn clear_resets_cursors() {
        let mut p = Pending::with_capacity(4);
        p.append(b"abcd");
        p.consume(1);
        p.clear();
        assert!(p.is_empty());
        assert_eq!(p.spare_mut().len(), 4);
    }
}
