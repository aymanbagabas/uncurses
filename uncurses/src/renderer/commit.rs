use crate::cell::Cell;

use super::{RenderBuffer, Renderer};

impl Renderer {
    pub(crate) fn commit_rows(&mut self, front: &mut RenderBuffer, count: u16) {
        let width = front.width();
        let old_height = front.height();
        let count = count.min(old_height);
        if count == 0 {
            return;
        }
        let height = old_height - count;
        for buffer in [Some(front), self.cur_buf.as_mut(), Some(&mut self.back_buf)]
            .into_iter()
            .flatten()
        {
            if buffer.width() == width && buffer.height() == old_height {
                buffer
                    .buffer
                    .delete_lines(0, count, old_height, &Cell::BLANK);
                buffer.resize_height(height);
                buffer.clear_touched();
            }
        }
        self.last_width = width;
        self.last_height = height;
        for cursor in [&mut self.cur, &mut self.saved] {
            // A saved row above the new origin has no unsigned coordinate.
            cursor.y = cursor.y.and_then(|y| y.checked_sub(count));
            if cursor.y.is_none() {
                cursor.at_phantom = false;
            }
        }
        self.old_hashes.clear();
        self.new_hashes.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{buffer::Surface, layout::Position};

    #[test]
    fn releases_matching_buffers_and_rebases_cursor_state() {
        let mut renderer = Renderer::new();
        let mut front = RenderBuffer::new(8, 3);
        for (row, (value, width)) in [("a", 1), ("\u{754c}", 2), ("c", 1)]
            .into_iter()
            .enumerate()
        {
            front.buffer.set((0, row as u16), &Cell::new(value, width));
        }
        renderer.cur_buf = Some(front.clone());
        renderer.back_buf = front.clone();
        renderer.cur.set_pos(Position::new(2, 2));
        renderer.saved.set_pos(Position::new(1, 0));
        renderer.saved.at_phantom = true;
        renderer.old_hashes.push(42);
        renderer.new_hashes.push(42);
        renderer.commit_rows(&mut front, 1);
        for buffer in [
            &front,
            renderer.cur_buf.as_ref().unwrap(),
            &renderer.back_buf,
        ] {
            assert_eq!(buffer.height(), 2);
            assert!(!buffer.has_changes());
            assert_eq!(
                buffer.buffer.cell((0, 0).into()).unwrap().content(),
                "\u{754c}"
            );
            assert!(buffer.buffer.cell((1, 0).into()).unwrap().is_continuation());
        }
        assert_eq!(renderer.cur.pos(), Position::new(2, 1));
        assert_eq!(renderer.saved.y, None);
        assert!(!renderer.saved.at_phantom);
        assert_eq!(renderer.last_size(), Some((8, 2)));
        assert!(renderer.old_hashes.is_empty());
        assert!(renderer.new_hashes.is_empty());
    }

    #[test]
    fn preserves_mismatched_caches() {
        let mut renderer = Renderer::new();
        let mut front = RenderBuffer::new(8, 3);
        renderer.cur_buf = Some(RenderBuffer::new(8, 2));
        renderer.back_buf = RenderBuffer::new(7, 3);
        renderer.saved.set_pos(Position::new(2, 2));
        renderer.commit_rows(&mut front, 1);
        assert_eq!(renderer.cur_buf.as_ref().unwrap().height(), 2);
        assert_eq!(renderer.back_buf.height(), 3);
        assert_eq!(renderer.back_buf.width(), 7);
        assert_eq!(renderer.saved.pos(), Position::new(2, 1));
    }
}
