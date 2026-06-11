//! Two-pane file explorer with a live preview pane.
//!
//! A real-world end-to-end demo that exercises a large part of the crate:
//!
//! - reads directory entries from the filesystem (`std::fs`),
//! - lazily loads a preview of the highlighted file (capped at 64 KiB),
//! - draws a two-pane TUI through the cell-based `Screen` so the
//!   renderer can diff between frames and only emit the cells that
//!   actually changed,
//! - handles keyboard navigation, mouse clicks, mouse-wheel scrolling,
//!   and terminal resize events.
//!
//! Input is read on a dedicated background thread doing blocking
//! [`Source::read`] calls; events are forwarded to the main thread
//! through an [`mpsc::channel`]. On exit, the main thread fires the
//! reader's paired [`Waker`](uncurses::event::Waker) so the blocking
//! read returns [`io::ErrorKind::Interrupted`] and the thread shuts
//! down cleanly.
//!
//! Run with `cargo run --example file_explorer [directory]`. If no
//! directory is given the current working directory is used.
//!
//! Keys:
//!   ↑/↓ or k/j   move selection
//!   PgUp/PgDn    scroll preview
//!   Enter        descend into directory
//!   Backspace    go up one directory
//!   r            refresh listing
//!   g / G        jump preview to top / bottom
//!   q / Esc      quit

use std::env;
use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;

use uncurses::SurfaceMut;
use uncurses::color::{BasicColor, Color};
use uncurses::event::{Event, Key, MouseButton, Source};
use uncurses::screen::Screen;
use uncurses::style::Style;
use uncurses::terminal::{disable_raw_mode, enable_raw_mode, get_window_size, stdin, stdout};
use uncurses::text::WrapMode;

const PREVIEW_LIMIT: usize = 64 * 1024;

#[derive(Debug, Clone)]
struct Entry {
    name: String,
    path: PathBuf,
    is_dir: bool,
    size: u64,
}

struct App {
    cwd: PathBuf,
    entries: Vec<Entry>,
    selected: usize,
    /// First visible row index in the file list.
    list_scroll: usize,
    /// First visible line index in the preview.
    preview_scroll: usize,
    preview_lines: Vec<String>,
    preview_label: String,
    status: String,
}

impl App {
    fn new(cwd: PathBuf) -> Self {
        let mut app = Self {
            cwd,
            entries: Vec::new(),
            selected: 0,
            list_scroll: 0,
            preview_scroll: 0,
            preview_lines: Vec::new(),
            preview_label: String::new(),
            status: String::new(),
        };
        app.refresh();
        app
    }

    fn refresh(&mut self) {
        self.entries.clear();
        match fs::read_dir(&self.cwd) {
            Ok(rd) => {
                let mut items: Vec<Entry> = rd
                    .filter_map(|res| res.ok())
                    .filter_map(|de| {
                        let meta = de.metadata().ok()?;
                        Some(Entry {
                            name: de.file_name().to_string_lossy().into_owned(),
                            path: de.path(),
                            is_dir: meta.is_dir(),
                            size: meta.len(),
                        })
                    })
                    .collect();
                // Dirs first, then alphabetical.
                items.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.cmp(&b.name)));
                self.entries = items;
                self.status = format!("{}  ·  {} entries", self.cwd.display(), self.entries.len());
            }
            Err(e) => {
                self.status = format!("error reading {}: {}", self.cwd.display(), e);
            }
        }
        if self.selected >= self.entries.len() {
            self.selected = self.entries.len().saturating_sub(1);
        }
        self.list_scroll = 0;
        self.load_preview();
    }

    fn load_preview(&mut self) {
        self.preview_lines.clear();
        self.preview_scroll = 0;
        let Some(entry) = self.entries.get(self.selected) else {
            self.preview_label = String::from("(empty)");
            return;
        };
        self.preview_label = entry.name.clone();
        if entry.is_dir {
            // For directories show their contents as the "preview".
            match fs::read_dir(&entry.path) {
                Ok(rd) => {
                    self.preview_lines
                        .push(format!("📁 {}", entry.path.display()));
                    self.preview_lines.push(String::new());
                    for de in rd.flatten().take(500) {
                        let m = de.metadata().ok();
                        let is_dir = m.as_ref().is_some_and(|m| m.is_dir());
                        let size = m.as_ref().map(|m| m.len()).unwrap_or(0);
                        let name = de.file_name().to_string_lossy().into_owned();
                        let glyph = if is_dir { "📁" } else { "📄" };
                        self.preview_lines.push(format!(
                            "{}  {:>10}  {}",
                            glyph,
                            format_size(size),
                            name
                        ));
                    }
                }
                Err(e) => self.preview_lines.push(format!("error: {e}")),
            }
            return;
        }
        match fs::File::open(&entry.path).and_then(|mut f| {
            use std::io::Read;
            let mut buf = Vec::with_capacity(PREVIEW_LIMIT.min(entry.size as usize + 1));
            (&mut f).take(PREVIEW_LIMIT as u64).read_to_end(&mut buf)?;
            Ok(buf)
        }) {
            Ok(bytes) => {
                let truncated = entry.size as usize > bytes.len();
                let text = if is_probably_binary(&bytes) {
                    self.preview_lines
                        .push(format!("<binary file, {} bytes>", entry.size));
                    self.preview_lines.push(String::new());
                    hex_dump(&bytes[..bytes.len().min(2048)])
                } else {
                    String::from_utf8_lossy(&bytes).into_owned()
                };
                for line in text.lines() {
                    // Replace tabs with 4 spaces for predictable rendering.
                    self.preview_lines.push(line.replace('\t', "    "));
                }
                if truncated {
                    self.preview_lines
                        .push(format!("… (truncated at {} bytes)", PREVIEW_LIMIT));
                }
            }
            Err(e) => self.preview_lines.push(format!("error: {e}")),
        }
    }

    fn move_selection(&mut self, delta: isize) {
        if self.entries.is_empty() {
            return;
        }
        let n = self.entries.len() as isize;
        let new = (self.selected as isize + delta).clamp(0, n - 1) as usize;
        if new != self.selected {
            self.selected = new;
            self.load_preview();
        }
    }

    fn enter(&mut self) {
        let Some(entry) = self.entries.get(self.selected) else {
            return;
        };
        if entry.is_dir {
            self.cwd = entry.path.clone();
            self.selected = 0;
            self.refresh();
        }
    }

    fn go_up(&mut self) {
        if let Some(parent) = self.cwd.parent() {
            self.cwd = parent.to_path_buf();
            self.selected = 0;
            self.refresh();
        }
    }
}

fn format_size(n: u64) -> String {
    const UNITS: &[&str] = &["B", "K", "M", "G", "T"];
    let mut v = n as f64;
    let mut u = 0;
    while v >= 1024.0 && u + 1 < UNITS.len() {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{} B", n)
    } else {
        format!("{:.1} {}", v, UNITS[u])
    }
}

fn is_probably_binary(bytes: &[u8]) -> bool {
    bytes.iter().take(1024).any(|&b| b == 0)
}

fn hex_dump(bytes: &[u8]) -> String {
    let mut out = String::new();
    for (i, chunk) in bytes.chunks(16).enumerate() {
        use std::fmt::Write as _;
        let _ = write!(out, "{:08x}  ", i * 16);
        for b in chunk {
            let _ = write!(out, "{:02x} ", b);
        }
        for _ in chunk.len()..16 {
            out.push_str("   ");
        }
        out.push(' ');
        for &b in chunk {
            out.push(if (0x20..=0x7e).contains(&b) {
                b as char
            } else {
                '.'
            });
        }
        out.push('\n');
    }
    out
}

// -- Rendering ---------------------------------------------------------------

fn draw<W: std::io::Write>(app: &App, screen: &mut Screen<W>) {
    let w = screen.width();
    let h = screen.height();
    if w < 20 || h < 5 {
        screen.clear();
        {
            screen.set_str((0, 0), "terminal too small", WrapMode::Truncate);
        };
        return;
    }

    screen.clear();

    let list_w: u16 = w / 3;
    let preview_x: u16 = list_w + 1;
    let preview_w: u16 = w - preview_x;
    let body_h: u16 = h.saturating_sub(2);

    let header = Style::EMPTY
        .fg(Color::Basic(BasicColor::Black))
        .bg(Color::Basic(BasicColor::Cyan))
        .bold();
    let normal = Style::EMPTY;
    let dim = Style::EMPTY.faint();
    let dir_style = Style::EMPTY.fg(Color::Basic(BasicColor::BrightBlue)).bold();
    let selected = Style::EMPTY
        .bg(Color::Basic(BasicColor::Blue))
        .fg(Color::Basic(BasicColor::BrightWhite));
    let selected_dir = selected.clone().bold();

    // Header bar across full width.
    {
        let pad = format!(
            " file_explorer · {:<width$}",
            app.preview_label,
            width = (w as usize).saturating_sub(17)
        );
        let pad = clip_to(&pad, w);
        {
            screen.set_str_with((0, 0), &pad, WrapMode::Truncate, header.clone());
        };
    }

    // -- Left pane: file list -----------------------------------------------
    let visible_rows = body_h as usize;
    let scroll = visible_rows
        .min(app.selected + 1)
        .saturating_sub(visible_rows)
        .max(app.list_scroll)
        // Keep selection visible: scroll down so selected is the last row
        // when it would otherwise fall off the bottom.
        .max(app.selected.saturating_sub(visible_rows.saturating_sub(1)));

    for row in 0..body_h {
        let idx = scroll + row as usize;
        let y = row + 1;
        if let Some(entry) = app.entries.get(idx) {
            let glyph = if entry.is_dir { "▸ " } else { "  " };
            let line = format!("{}{}", glyph, entry.name);
            let line = clip_to(&line, list_w);
            // pad the row so the selection bar covers the whole list column
            let padded = pad_to(&line, list_w);
            let style = match (idx == app.selected, entry.is_dir) {
                (true, true) => selected_dir.clone(),
                (true, false) => selected.clone(),
                (false, true) => dir_style.clone(),
                (false, false) => normal.clone(),
            };
            {
                screen.set_str_with((0, y), &padded, WrapMode::Truncate, style.clone());
            };
        }
    }

    // Vertical divider.
    for row in 0..body_h {
        {
            screen.set_str_with((list_w, row + 1), "│", WrapMode::Truncate, dim.clone());
        };
    }

    // -- Right pane: preview ------------------------------------------------
    let preview_height = body_h as usize;
    for row in 0..preview_height {
        let idx = app.preview_scroll + row;
        if let Some(line) = app.preview_lines.get(idx) {
            let s = clip_to(line, preview_w);
            {
                screen.set_str_with(
                    (preview_x, row as u16 + 1),
                    &s,
                    WrapMode::Truncate,
                    normal.clone(),
                );
            };
        }
    }

    // -- Footer -------------------------------------------------------------
    let status_y = h - 1;
    let help = "↑↓:move  ⏎:open  ⌫:up  PgUp/PgDn:scroll  r:refresh  q:quit";
    let status_line = format!(" {}  ·  {}", app.status, help);
    let status_line = pad_to(&clip_to(&status_line, w), w);
    screen.set_str_with(
        (0, status_y),
        &status_line,
        WrapMode::Truncate,
        Style::EMPTY
            .bg(Color::Basic(BasicColor::BrightBlack))
            .fg(Color::Basic(BasicColor::White)),
    );
}

fn clip_to(s: &str, width: u16) -> String {
    let mut out = String::new();
    let mut w = 0usize;
    for ch in s.chars() {
        let cw = unicode_char_width(ch);
        if w + cw > width as usize {
            break;
        }
        out.push(ch);
        w += cw;
    }
    out
}

fn pad_to(s: &str, width: u16) -> String {
    let mut out = s.to_string();
    let cur: usize = s.chars().map(unicode_char_width).sum();
    if cur < width as usize {
        out.extend(std::iter::repeat_n(' ', width as usize - cur));
    }
    out
}

fn unicode_char_width(ch: char) -> usize {
    // Cheap stand-in: assume ASCII and emoji-style glyphs render as 1 cell
    // per char for layout purposes; the underlying buffer uses proper
    // unicode-width when it stores grapheme clusters.
    if (ch as u32) < 0x20 { 0 } else { 1 }
}

// -- Main loop ---------------------------------------------------------------

fn run() -> std::io::Result<()> {
    let start_dir = env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    let start_dir = fs::canonicalize(&start_dir).unwrap_or(start_dir);

    let mut app = App::new(start_dir);

    let state = enable_raw_mode(stdin(), stdout())?;

    let size = get_window_size(stdout()).unwrap_or_default();
    let mut screen = Screen::new(stdout()).with_size(size.col, size.row);

    // Enter the alt screen, hide the cursor, and enable SGR-encoded
    // mouse tracking via the screen API so internal state stays in
    // sync with the actual terminal mode flags.
    screen.set_alt_screen(true)?;
    screen.set_cursor_visible(false)?;
    screen.set_mouse_mode(
        uncurses::ansi::mode::MouseMode::Normal,
        uncurses::ansi::mode::MouseEncoding::Sgr,
    )?;
    screen.flush()?;

    draw(&app, &mut screen);
    screen.render()?;
    screen.flush()?;

    let mut events = Source::new(stdin())?;
    let waker = events.waker();

    let (tx, rx) = mpsc::channel::<Event>();
    let reader_thread = thread::spawn(move || {
        loop {
            match events.read() {
                Ok(ev) => {
                    if tx.send(ev).is_err() {
                        // Main thread dropped the receiver.
                        break;
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {
                    // Woken by the main thread for shutdown.
                    break;
                }
                Err(_) => break,
            }
        }
    });

    // Parse key bindings once. `Key` implements `FromStr`, so
    // `"ctrl+c".parse::<Key>()` produces a canonical `Key` value, and
    // `PartialEq` compares only the chord identity (`code` +
    // `modifiers`, with `CAPS_LOCK`/`NUM_LOCK` ignored) — so plain
    // `==` is the right operator for keyboard-shortcut matching.
    let quit_keys: [Key; 3] = ["q", "esc", "ctrl+c"].map(|s| s.parse().unwrap());
    let up_keys: [Key; 2] = ["up", "k"].map(|s| s.parse().unwrap());
    let down_keys: [Key; 2] = ["down", "j"].map(|s| s.parse().unwrap());
    let pgup_key: Key = "pageup".parse().unwrap();
    let pgdn_key: Key = "pagedown".parse().unwrap();
    // `g` and `G` parse to different canonical chords: `g` is the
    // bare lowercase code, `G` normalizes to `g + SHIFT`. `==`
    // distinguishes them, which is what vim-style `g`/`G` bindings
    // want.
    let top_key: Key = "g".parse().unwrap();
    let bottom_key: Key = "G".parse().unwrap();
    let enter_key: Key = "enter".parse().unwrap();
    let backspace_key: Key = "backspace".parse().unwrap();
    let refresh_key: Key = "r".parse().unwrap();

    while let Ok(ev) = rx.recv() {
        let mut dirty = true;
        match ev {
            Event::KeyPress(ref key) if quit_keys.contains(key) => break,

            Event::KeyPress(key) => {
                if up_keys.contains(&key) {
                    app.move_selection(-1);
                } else if down_keys.contains(&key) {
                    app.move_selection(1);
                } else if key == pgup_key {
                    app.preview_scroll = app.preview_scroll.saturating_sub(10);
                } else if key == pgdn_key {
                    app.preview_scroll =
                        (app.preview_scroll + 10).min(app.preview_lines.len().saturating_sub(1));
                } else if key == top_key {
                    app.preview_scroll = 0;
                } else if key == bottom_key {
                    app.preview_scroll = app.preview_lines.len().saturating_sub(1);
                } else if key == enter_key {
                    app.enter();
                } else if key == backspace_key {
                    app.go_up();
                } else if key == refresh_key {
                    app.refresh();
                } else {
                    dirty = false;
                }
            }

            Event::MouseClick(m) => {
                let list_w = screen.width() / 3;
                if m.x < list_w && m.y >= 1 {
                    let row = (m.y - 1) as usize;
                    // Recompute scroll the same way draw() does so clicks land.
                    let body_h = screen.height().saturating_sub(2) as usize;
                    let scroll = app
                        .selected
                        .saturating_sub(body_h.saturating_sub(1))
                        .max(app.list_scroll);
                    let idx = scroll + row;
                    if idx < app.entries.len() {
                        app.selected = idx;
                        app.load_preview();
                    }
                } else {
                    dirty = false;
                }
            }
            Event::MouseWheel(m) => match m.button {
                MouseButton::WheelUp => {
                    app.preview_scroll = app.preview_scroll.saturating_sub(3);
                }
                MouseButton::WheelDown => {
                    app.preview_scroll =
                        (app.preview_scroll + 3).min(app.preview_lines.len().saturating_sub(1));
                }
                _ => dirty = false,
            },

            Event::Resize(ws) => {
                screen.resize(ws.col, ws.row);
            }

            _ => dirty = false,
        }

        if dirty {
            draw(&app, &mut screen);
            screen.render()?;
            screen.flush()?;
        }
    }

    // Wake the reader thread out of its blocking read, drop the
    // receiver so the channel send fails if the wake races, and join.
    waker.wake().ok();
    drop(rx);
    reader_thread.join().ok();

    // Restore terminal.
    screen.reset()?;
    screen.flush()?;
    disable_raw_mode(stdin(), stdout(), &state)?;
    Ok(())
}

fn main() {
    if let Err(e) = run() {
        eprintln!("file_explorer: {e}");
        std::process::exit(1);
    }
}
