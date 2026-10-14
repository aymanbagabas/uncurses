//! Replay the renderer's own bytes through an independent screen model
//! and compare the result against what the frame asked for.
//!
//! Every other test here pins bytes. Byte goldens lock the planner's
//! choices in place, but they cannot answer the question that matters to
//! someone looking at a terminal: after these bytes arrive, does the
//! screen hold what the application drew? The renderer keeps its own
//! model of the terminal in `cur_buf` and diffs against it, so a mistake
//! *in that model* is invisible to the diff. It writes bytes it believes
//! are correct, records what it believes they did, and the two agree with
//! each other while both disagree with the terminal.
//!
//! [`TermModel`] is a second, independent model built only from the
//! escape sequences the renderer actually emits. It is deliberately naive:
//! it applies each sequence the way a terminal would and holds no opinion
//! about what the renderer intended.
//!
//! The model is itself under test. Every case runs first with
//! optimizations off, where the renderer emits little more than absolute
//! positioning and text, and asserts the replay matches. A model that
//! mis-implements the basics fails there, loudly, before any conclusion is
//! drawn about the optimized path.

use super::*;
use crate::cell::Cell;
use crate::renderer::RenderBuffer;

/// A minimal terminal: a grid of glyphs, a cursor, and a scrolling region.
///
/// Only the sequences the renderer emits are implemented. Anything else is
/// a bug in this model or a new sequence in the renderer, so unknown input
/// panics rather than being skipped: silently ignoring a sequence would
/// make the screen quietly wrong and the comparison meaningless.
struct TermModel {
    w: u16,
    h: u16,
    cells: Vec<char>,
    cx: u16,
    cy: u16,
    /// Scrolling region, inclusive, as set by DECSTBM.
    top: u16,
    bot: u16,
    /// Whether the host translates `\n` into `\r\n`, which the renderer
    /// plans around when [`Optimizations::ONLCR`] is granted.
    onlcr: bool,
    /// DECAWM. The renderer turns this off to paint the last cell of a
    /// row without triggering a wrap, then turns it back on.
    autowrap: bool,
    /// Set after painting the last column with autowrap on: the cursor
    /// stays put and the *next* glyph wraps. Without this a terminal
    /// could never fill a row without scrolling.
    pending_wrap: bool,
}

/// Tab stops sit every this many columns from column 0.
const TAB_INTERVAL: u16 = 8;

impl TermModel {
    fn new(w: u16, h: u16) -> Self {
        Self {
            w,
            h,
            cells: vec![' '; w as usize * h as usize],
            cx: 0,
            cy: 0,
            top: 0,
            bot: h.saturating_sub(1),
            onlcr: false,
            autowrap: true,
            pending_wrap: false,
        }
    }

    /// Column of the `n`th tab stop at or after the cursor.
    fn tab_forward(&self, n: u16) -> u16 {
        let mut x = self.cx;
        for _ in 0..n.max(1) {
            x = (x / TAB_INTERVAL + 1) * TAB_INTERVAL;
        }
        x.min(self.w - 1)
    }

    /// Column of the `n`th tab stop at or before the cursor.
    fn tab_backward(&self, n: u16) -> u16 {
        let mut x = self.cx;
        for _ in 0..n.max(1) {
            x = x.saturating_sub(1) / TAB_INTERVAL * TAB_INTERVAL;
        }
        x
    }

    fn idx(&self, x: u16, y: u16) -> usize {
        y as usize * self.w as usize + x as usize
    }

    fn row(&self, y: u16) -> String {
        let start = self.idx(0, y);
        self.cells[start..start + self.w as usize].iter().collect()
    }

    fn put(&mut self, ch: char) {
        if self.pending_wrap {
            self.cx = 0;
            self.line_feed();
            self.pending_wrap = false;
        }
        if self.cx < self.w && self.cy < self.h {
            let i = self.idx(self.cx, self.cy);
            self.cells[i] = ch;
        }
        if self.cx + 1 >= self.w {
            // The cursor stays on the last column either way; with
            // autowrap on it is the *next* glyph that moves to a new row.
            self.pending_wrap = self.autowrap;
        } else {
            self.cx += 1;
        }
    }

    /// Scroll `[top..=bot]` up by `n`, filling the exposed rows with blanks.
    fn scroll_up(&mut self, n: u16) {
        let (top, bot) = (self.top, self.bot);
        for _ in 0..n {
            for y in top..bot {
                for x in 0..self.w {
                    let (from, to) = (self.idx(x, y + 1), self.idx(x, y));
                    self.cells[to] = self.cells[from];
                }
            }
            self.blank_row(bot);
        }
    }

    fn scroll_down(&mut self, n: u16) {
        let (top, bot) = (self.top, self.bot);
        for _ in 0..n {
            for y in (top..bot).rev() {
                for x in 0..self.w {
                    let (from, to) = (self.idx(x, y), self.idx(x, y + 1));
                    self.cells[to] = self.cells[from];
                }
            }
            self.blank_row(top);
        }
    }

    fn blank_row(&mut self, y: u16) {
        let start = self.idx(0, y);
        for i in start..start + self.w as usize {
            self.cells[i] = ' ';
        }
    }

    /// DL: delete `n` lines at the cursor row. Rows below shift up within
    /// the scrolling region; blanks come in at the region bottom.
    fn delete_lines(&mut self, n: u16) {
        if self.cy < self.top || self.cy > self.bot {
            return;
        }
        let (saved_top, bot) = (self.top, self.bot);
        self.top = self.cy;
        self.scroll_up(n.min(bot - self.cy + 1));
        self.top = saved_top;
    }

    /// IL: insert `n` blank lines at the cursor row, pushing rows down
    /// within the scrolling region.
    fn insert_lines(&mut self, n: u16) {
        if self.cy < self.top || self.cy > self.bot {
            return;
        }
        let (saved_top, bot) = (self.top, self.bot);
        self.top = self.cy;
        self.scroll_down(n.min(bot - self.cy + 1));
        self.top = saved_top;
    }

    fn erase_to_eol(&mut self) {
        for x in self.cx..self.w {
            let i = self.idx(x, self.cy);
            self.cells[i] = ' ';
        }
    }

    fn erase_below(&mut self) {
        self.erase_to_eol();
        for y in self.cy + 1..self.h {
            self.blank_row(y);
        }
    }

    /// Advance one row, scrolling the region when already at its bottom.
    fn line_feed(&mut self) {
        if self.cy == self.bot {
            self.scroll_up(1);
        } else {
            self.cy = (self.cy + 1).min(self.h - 1);
        }
    }

    /// RI: retreat one row, scrolling the region when already at its top.
    fn reverse_index(&mut self) {
        if self.cy == self.top {
            self.scroll_down(1);
        } else {
            self.cy = self.cy.saturating_sub(1);
        }
    }

    /// CUD. A cursor at or above the bottom margin stops there rather than
    /// leaving the scrolling region.
    fn cursor_down(&self, n: u16) -> u16 {
        let limit = if self.cy <= self.bot {
            self.bot
        } else {
            self.h - 1
        };
        (self.cy + n).min(limit)
    }

    /// CUU. Mirror of [`Self::cursor_down`] against the top margin.
    fn cursor_up(&self, n: u16) -> u16 {
        let limit = if self.cy >= self.top { self.top } else { 0 };
        self.cy.saturating_sub(n).max(limit)
    }

    fn feed(&mut self, bytes: &[u8]) {
        let s = String::from_utf8_lossy(bytes).into_owned();
        let b: Vec<char> = s.chars().collect();
        let mut i = 0;
        while i < b.len() {
            match b[i] {
                '\r' => {
                    self.cx = 0;
                    self.pending_wrap = false;
                    i += 1;
                }
                '\n' => {
                    self.line_feed();
                    // The tty driver turns this into CRLF, which is why the
                    // planner treats a line feed as resetting the column.
                    if self.onlcr {
                        self.cx = 0;
                    }
                    self.pending_wrap = false;
                    i += 1;
                }
                '\t' => {
                    self.cx = self.tab_forward(1);
                    self.pending_wrap = false;
                    i += 1;
                }
                '\x08' => {
                    self.cx = self.cx.saturating_sub(1);
                    self.pending_wrap = false;
                    i += 1;
                }
                '\x1b' => i += self.escape(&b, i),
                ch if (ch as u32) < 0x20 || ch == '\x7f' => {
                    panic!("unmodelled control byte: {:?}", ch as u32)
                }
                ch => {
                    self.put(ch);
                    i += 1;
                }
            }
        }
    }

    /// Apply the escape sequence starting at `i`; returns its length.
    fn escape(&mut self, b: &[char], i: usize) -> usize {
        match b.get(i + 1) {
            // RI.
            Some('M') => {
                self.reverse_index();
                self.pending_wrap = false;
                2
            }
            Some('[') => self.csi(b, i),
            // OSC: run to the terminator. Styles and links do not move the
            // cursor or change a glyph, so the payload is irrelevant here.
            Some(']') => {
                let mut j = i + 2;
                while j < b.len() && b[j] != '\x07' {
                    if b[j] == '\x1b' && b.get(j + 1) == Some(&'\\') {
                        return j + 2 - i;
                    }
                    j += 1;
                }
                (j + 1 - i).min(b.len() - i)
            }
            other => panic!("unmodelled escape: ESC {other:?}"),
        }
    }

    fn csi(&mut self, b: &[char], i: usize) -> usize {
        let mut j = i + 2;
        // Private-parameter CSI (mode set/reset) carries a `?` prefix.
        let private = b.get(j) == Some(&'?');
        if private {
            j += 1;
        }
        let start = j;
        while j < b.len() && (b[j].is_ascii_digit() || b[j] == ';') {
            j += 1;
        }
        let params: Vec<u16> = b[start..j]
            .iter()
            .collect::<String>()
            .split(';')
            .map(|p| p.parse().unwrap_or(0))
            .collect();
        let p = |n: usize, default: u16| -> u16 {
            match params.get(n) {
                Some(&0) | None => default,
                Some(&v) => v,
            }
        };
        let Some(&final_byte) = b.get(j) else {
            panic!("truncated CSI");
        };
        let len = j + 1 - i;

        // Modes: only autowrap changes how glyphs land.
        if private {
            if params.first() == Some(&7) {
                self.autowrap = final_byte == 'h';
                self.pending_wrap = false;
            }
            return len;
        }

        // Any explicit positioning or editing settles a pending wrap.
        if final_byte != 'm' && final_byte != 't' {
            self.pending_wrap = false;
        }

        match final_byte {
            // SGR and window ops leave the glyph grid alone.
            'm' | 't' => {}
            'H' => {
                self.cy = p(0, 1).saturating_sub(1).min(self.h - 1);
                self.cx = p(1, 1).saturating_sub(1).min(self.w - 1);
            }
            'G' | '`' => self.cx = p(0, 1).saturating_sub(1).min(self.w - 1),
            'd' => self.cy = p(0, 1).saturating_sub(1).min(self.h - 1),
            'A' => self.cy = self.cursor_up(p(0, 1)),
            'B' => self.cy = self.cursor_down(p(0, 1)),
            'C' => self.cx = (self.cx + p(0, 1)).min(self.w - 1),
            'D' => self.cx = self.cx.saturating_sub(p(0, 1)),
            'I' => self.cx = self.tab_forward(p(0, 1)),
            'Z' => self.cx = self.tab_backward(p(0, 1)),
            'S' => self.scroll_up(p(0, 1)),
            'T' => self.scroll_down(p(0, 1)),
            'L' => self.insert_lines(p(0, 1)),
            'M' => self.delete_lines(p(0, 1)),
            'K' => match params.first().copied().unwrap_or(0) {
                // EL defaults to 0, "erase to end of line", which is the
                // only variant the renderer emits.
                0 => self.erase_to_eol(),
                other => panic!("unmodelled EL variant: {other}"),
            },
            'J' => match params.first().copied().unwrap_or(0) {
                0 => self.erase_below(),
                2 => {
                    for y in 0..self.h {
                        self.blank_row(y);
                    }
                }
                other => panic!("unmodelled ED variant: {other}"),
            },
            'X' => {
                let n = p(0, 1);
                for x in self.cx..(self.cx + n).min(self.w) {
                    let idx = self.idx(x, self.cy);
                    self.cells[idx] = ' ';
                }
            }
            'P' => {
                let n = p(0, 1) as usize;
                let row = self.idx(0, self.cy);
                let from = row + self.cx as usize;
                let end = row + self.w as usize;
                self.cells.copy_within((from + n).min(end)..end, from);
                for idx in end.saturating_sub(n).max(from)..end {
                    self.cells[idx] = ' ';
                }
            }
            '@' => {
                let n = p(0, 1) as usize;
                let row = self.idx(0, self.cy);
                let from = row + self.cx as usize;
                let end = row + self.w as usize;
                self.cells
                    .copy_within(from..end.saturating_sub(n).max(from), from + n);
                for idx in from..(from + n).min(end) {
                    self.cells[idx] = ' ';
                }
            }
            // REP repeats the last printed glyph.
            'b' => {
                let n = p(0, 1);
                // With a wrap pending the last glyph sits *at* the cursor,
                // which has not moved off the final column yet.
                let last_x = if self.pending_wrap {
                    Some(self.cx)
                } else {
                    self.cx.checked_sub(1)
                };
                let last = last_x.map_or(' ', |x| self.cells[self.idx(x, self.cy)]);
                for _ in 0..n {
                    self.put(last);
                }
            }
            'r' => {
                // DECSTBM with no parameters resets to the full screen.
                if params.iter().all(|&v| v == 0) {
                    self.top = 0;
                    self.bot = self.h - 1;
                } else {
                    self.top = p(0, 1).saturating_sub(1);
                    self.bot = p(1, self.h).saturating_sub(1).min(self.h - 1);
                }
                // DECSTBM homes the cursor. With origin mode reset, which
                // is the default and what the renderer assumes it cannot
                // know, home is the top left of the screen rather than of
                // the region.
                self.cx = 0;
                self.cy = 0;
            }
            other => panic!("unmodelled CSI final byte: {other:?} params {params:?}"),
        }
        len
    }
}

/// Render `frames` through a renderer with `opts`, replaying every byte
/// into a [`TermModel`], and assert the model matches the last frame.
#[track_caller]
fn assert_replay_matches(w: u16, h: u16, opts: Optimizations, frames: &[Vec<String>]) {
    assert_replay_matches_labelled(w, h, opts, frames, "");
}

#[track_caller]
fn assert_replay_matches_labelled(
    w: u16,
    h: u16,
    opts: Optimizations,
    frames: &[Vec<String>],
    label: &str,
) {
    let mut renderer = Renderer::new();
    renderer.set_optimizations(opts);
    renderer.set_fullscreen(true);
    // Scroll detection only runs on a frame the caller presents atomically.
    renderer.set_sync_output(true);
    renderer.set_scroll_optimize(true);

    let mut term = TermModel::new(w, h);
    term.onlcr = opts.contains(Optimizations::ONLCR);
    let mut buf = RenderBuffer::new(w, h);
    let mut last = Vec::new();
    let mut wire: Vec<String> = Vec::new();

    for rows in frames {
        for (y, row) in rows.iter().enumerate() {
            for (x, ch) in row.chars().enumerate() {
                buf.set_cell((x as u16, y as u16), &Cell::narrow(ch.to_string()));
            }
        }
        let mut out = Vec::new();
        renderer.render(&mut out, &mut buf).unwrap();
        term.feed(&out);
        wire.push(String::from_utf8_lossy(&out).replace('\x1b', "<ESC>"));
        last = rows.clone();
    }

    for (y, expected) in last.iter().enumerate() {
        assert_eq!(
            term.row(y as u16),
            *expected,
            "row {y} on screen does not match the frame ({opts:?}) {label}\n\
             frames: {frames:#?}\nwire: {wire:#?}"
        );
    }
}

/// A scrolling body above a block pinned to the bottom rows: the shape of
/// a dashboard whose list scrolls while its footer stays put.
fn scrolling_body_with_pinned_footer(offset: usize) -> Vec<String> {
    const W: usize = 20;
    const BODY: usize = 8;
    let mut rows: Vec<String> = (0..BODY)
        .map(|i| format!("{:<W$}", format!("item {}", i + offset)))
        .collect();
    rows.push(format!("{:<W$}", ""));
    rows.push(format!("{:<W$}", "search: >"));
    rows.push(format!("{:<W$}", "footer  q:quit"));
    rows
}

#[test]
fn replay_matches_the_frame_without_optimizations() {
    // Validates the model itself. With nothing enabled the renderer emits
    // little beyond absolute positioning and text, so a failure here is a
    // defect in `TermModel` rather than in the renderer.
    let frames: Vec<Vec<String>> = (0..4)
        .map(|i| scrolling_body_with_pinned_footer(i * 3))
        .collect();
    assert_replay_matches(20, 11, Optimizations::empty(), &frames);
}

#[test]
fn a_pinned_footer_survives_a_scrolling_body() {
    // The prowl shape: a body that scrolls under a block pinned to the
    // bottom rows. Every line-scrolling optimization is on, so the planner
    // may reach for CSR, SU/SD, or the DL+IL pair.
    let frames: Vec<Vec<String>> = (0..4)
        .map(|i| scrolling_body_with_pinned_footer(i * 3))
        .collect();
    assert_replay_matches(
        20,
        11,
        Optimizations::CSR | Optimizations::SU_SD | Optimizations::IL_DL,
        &frames,
    );
}

#[test]
fn a_pinned_footer_survives_a_scrolling_body_under_every_preset() {
    let frames: Vec<Vec<String>> = (0..4)
        .map(|i| scrolling_body_with_pinned_footer(i * 3))
        .collect();
    for opts in [
        Optimizations::default(),
        Optimizations::modern(),
        Optimizations::xterm(),
        Optimizations::all(),
    ] {
        assert_replay_matches(20, 11, opts, &frames);
    }
}

/// A body that scrolls beside a column that must not move: a list with a
/// scrollbar, where the thumb travels the other way as the rows slide.
fn scrolling_body_with_scrollbar(offset: usize) -> Vec<String> {
    const W: usize = 24;
    const H: usize = 10;
    (0..H)
        .map(|y| {
            let body = format!("item {}", y + offset);
            let thumb = if y == offset % H { '#' } else { '|' };
            format!("{body:<w$}{thumb}", w = W - 1)
        })
        .collect()
}

#[test]
fn a_scrollbar_column_survives_a_scrolling_body() {
    let frames: Vec<Vec<String>> = (0..6)
        .map(|i| scrolling_body_with_scrollbar(i * 2))
        .collect();
    for opts in [
        Optimizations::empty(),
        Optimizations::CSR | Optimizations::SU_SD | Optimizations::IL_DL,
        Optimizations::default(),
        Optimizations::all(),
    ] {
        assert_replay_matches(24, 10, opts, &frames);
    }
}

/// Deterministic pseudo-random sequence. A fixed seed keeps a failure
/// reproducible; there is no need for a generator crate to shake out
/// shapes a handful of hand-written fixtures never reach.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

#[test]
fn replay_matches_the_frame_across_random_edits() {
    // Hand-written fixtures only reach the shapes their author thought
    // of. This sweeps arbitrary edits — full-view scrolls, partial ones,
    // rewrites, and rows that shift while their neighbours hold — against
    // every optimization set, which is where a planner that moves a row it
    // should have left alone shows up.
    const W: u16 = 16;
    const H: u16 = 9;
    let alphabet: Vec<char> = "abcdefgh".chars().collect();

    for seed in 1..=200u64 {
        let mut rng = Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1);
        // Vary the geometry too: a planner that is right at one size can
        // still mishandle a region that reaches an edge at another.
        let w = W - (seed % 5) as u16;
        let h = H - (seed % 4) as u16;
        let mut rows: Vec<String> = (0..h)
            .map(|y| {
                let ch = alphabet[y as usize % alphabet.len()];
                std::iter::repeat_n(ch, w as usize).collect()
            })
            .collect();

        let mut frames = vec![rows.clone()];
        for _ in 0..5 {
            match rng.below(5) {
                // Scroll the whole view.
                0 => {
                    let n = 1 + rng.below(3);
                    rows.rotate_left(n.min(h as usize));
                }
                // Scroll a sub-region, leaving the rest pinned.
                1 => {
                    let top = rng.below(h as usize - 2);
                    let bot = top + 2 + rng.below(h as usize - top - 2);
                    let n = 1 + rng.below(2);
                    rows[top..=bot].rotate_left(n.min(bot - top));
                }
                // Rewrite one row.
                2 => {
                    let y = rng.below(h as usize);
                    let ch = alphabet[rng.below(alphabet.len())];
                    rows[y] = std::iter::repeat_n(ch, w as usize).collect();
                }
                // Scroll the body while a bottom block stays pinned: the
                // shape of a dashboard with a footer.
                3 => {
                    let pinned = 1 + rng.below(3);
                    let body = h as usize - pinned.min(h as usize - 1);
                    let n = 1 + rng.below(2);
                    rows[..body].rotate_left(n.min(body.saturating_sub(1)).max(1));
                }
                // Change part of one row, so the row moves but not wholly.
                _ => {
                    let y = rng.below(h as usize);
                    let x = rng.below(w as usize);
                    let ch = alphabet[rng.below(alphabet.len())];
                    let mut chars: Vec<char> = rows[y].chars().collect();
                    chars[x] = ch;
                    rows[y] = chars.into_iter().collect();
                }
            }
            frames.push(rows.clone());
        }

        for opts in [
            Optimizations::empty(),
            Optimizations::CSR | Optimizations::SU_SD | Optimizations::IL_DL,
            Optimizations::default(),
            Optimizations::modern(),
            Optimizations::xterm(),
            Optimizations::all(),
        ] {
            let label = format!("seed {seed}");
            assert_replay_matches_labelled(w, h, opts, &frames, &label);
        }
    }
}
