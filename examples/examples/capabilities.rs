//! A colorful, real-world terminal capability probe.
//!
//! Terminals answer feature queries (OSC / CSI / DCS / APC requests) only
//! when they support them, and stay silent otherwise — so a probe asks two
//! questions at once: *did the terminal reply?* and, if not, *is it
//! unsupported or merely slow?* This example fires a whole battery of real
//! queries and prints a report showing, for each one: what was asked, the
//! timeout budget, how long the reply actually took, and the decoded
//! answer (with live color swatches for the color queries).
//!
//! It exercises both faces of the query API against a thread-backed
//! [`EventStream`](uncurses::event::EventStream):
//!
//!   * `query_blocking` — used once up front for a Primary Device
//!     Attributes (DA1) **sentinel**. Every terminal answers DA1, so its
//!     latency is the yardstick: a capability that stays silent well past
//!     the DA1 round-trip is unsupported, not slow.
//!   * `query` + `.await` — every other capability is issued up front
//!     (all in flight at once, each its own request) and then awaited
//!     concurrently with `tokio::join!`, timing each independently.
//!
//! Every reply also stays visible to ordinary
//! [`read`](uncurses::event::EventSource::read): a query never swallows
//! input. Run with `cargo run --example capabilities`.

use std::io::{self, Write};
use std::time::{Duration, Instant};

use uncurses::ansi::KittyKeyboardFlags;
use uncurses::ansi::mode::{Mode, ModeSetting};
use uncurses::color::Color;
use uncurses::event::source::Input;
use uncurses::event::{
    ClipboardSelection, ColorScheme, EventSource, EventStream, QueryReply, query,
};
use uncurses::terminal::Terminal;

/// Per-query budget. The DA1 sentinel makes this safe to keep short: we
/// rely on the sentinel — not a generous timeout — to tell unsupported
/// from slow.
const BUDGET: Duration = Duration::from_millis(200);

// -- SGR helpers (we print the report after leaving raw mode) ---------------
const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";
const RED: &str = "\x1b[31m";
const GREEN: &str = "\x1b[32m";
const YELLOW: &str = "\x1b[33m";
const CYAN: &str = "\x1b[36m";

#[tokio::main(flavor = "current_thread")]
async fn main() -> io::Result<()> {
    let mut term = Terminal::open()?;
    term.make_raw()?;
    let report = run(&mut term).await;
    // Always leave raw mode before printing, whatever happened.
    term.restore()?;

    match report {
        Ok(lines) => {
            for line in lines {
                println!("{line}");
            }
            Ok(())
        }
        Err(e) => Err(e),
    }
}

/// The decoded, presentable form of one reply.
struct Detail {
    text: String,
    /// An RGB swatch to render alongside the text (color queries only).
    swatch: Option<(u8, u8, u8)>,
}

impl Detail {
    fn text(s: impl Into<String>) -> Self {
        Self {
            text: s.into(),
            swatch: None,
        }
    }
    fn color(c: Color) -> Self {
        let (r, g, b) = c.to_rgb();
        Self {
            text: format!("{c:?}"),
            swatch: Some((r, g, b)),
        }
    }
}

async fn run<I, O>(term: &mut Terminal<I, O>) -> io::Result<Vec<String>>
where
    I: Input + Copy + 'static,
    O: Write + Copy,
{
    let mut out = term.output();
    let stream = EventSource::new(term.input())?.into_stream();

    // ---- Sentinel: DA1, blocking. Establishes the "answered" baseline. --
    let t = Instant::now();
    let da1 = stream.query_blocking(&mut out, query::PRIMARY_DEVICE_ATTRIBUTES, BUDGET)?;
    let da1_elapsed = t.elapsed();
    let da1_ok = da1.is_some();
    // Sixel support is advertised as attribute 4 in the DA1 reply.
    let sixel = da1.as_ref().map(|a| a.contains(&Some(4))).unwrap_or(false);

    // ---- Concurrent sweep: issue every query up front (each its own ----
    //      request, all in flight at once), then await them together.
    let t0 = Instant::now();
    let (h_ver, q_ver) = issue(&stream, &mut out, query::TERMINAL_VERSION)?;
    let (h_da2, q_da2) = issue(&stream, &mut out, query::SECONDARY_DEVICE_ATTRIBUTES)?;
    let (h_da3, q_da3) = issue(&stream, &mut out, query::TERTIARY_DEVICE_ATTRIBUTES)?;
    let (h_fg, q_fg) = issue(&stream, &mut out, query::FOREGROUND_COLOR)?;
    let (h_bg, q_bg) = issue(&stream, &mut out, query::BACKGROUND_COLOR)?;
    let (h_cur, q_cur) = issue(&stream, &mut out, query::CURSOR_COLOR)?;
    let (h_scheme, q_scheme) = issue(&stream, &mut out, query::COLOR_SCHEME)?;
    let (h_kbd, q_kbd) = issue(&stream, &mut out, query::KITTY_KEYBOARD_FLAGS)?;
    let (h_gfx, q_gfx) = issue(
        &stream,
        &mut out,
        query::kitty_graphics(&["i=1", "s=1", "v=1"]),
    )?;
    let (h_rgb, q_rgb) = issue(&stream, &mut out, query::termcap(&["RGB"]))?;
    let (h_sync, q_sync) = issue(&stream, &mut out, query::mode(Mode::SYNCHRONIZED_OUTPUT))?;
    let (h_paste, q_paste) = issue(&stream, &mut out, query::mode(Mode::BRACKETED_PASTE))?;
    let (h_cell, q_cell) = issue(&stream, &mut out, query::CELL_PIXEL_SIZE)?;
    let (h_win, q_win) = issue(&stream, &mut out, query::WINDOW_PIXEL_SIZE)?;
    let (h_clip, q_clip) = issue(
        &stream,
        &mut out,
        query::read_clipboard(ClipboardSelection::System),
    )?;

    // Await all concurrently; each future records its own reply latency.
    let (ver, da2, da3, fg, bg, cur, scheme, kbd, gfx, rgb, sync, paste, cell, win, clip) = tokio::join!(
        timed(h_ver, t0),
        timed(h_da2, t0),
        timed(h_da3, t0),
        timed(h_fg, t0),
        timed(h_bg, t0),
        timed(h_cur, t0),
        timed(h_scheme, t0),
        timed(h_kbd, t0),
        timed(h_gfx, t0),
        timed(h_rgb, t0),
        timed(h_sync, t0),
        timed(h_paste, t0),
        timed(h_cell, t0),
        timed(h_win, t0),
        timed(h_clip, t0),
    );

    // Capture booleans needed for the derived section before the typed
    // results are consumed by the row renderer.
    let truecolor = rgb.0.is_some();

    // ---- Build the report ----------------------------------------------
    let mut lines = Vec::new();
    lines.push(format!(
        "{BOLD}{CYAN}╓─ terminal capability probe ──────────────────────────────────────╖{RESET}"
    ));
    lines.push(format!(
        "{DIM}  budget {BUDGET:?} per query · {GREEN}●{DIM} answered · {RED}○{DIM} silent · sentinel = DA1{RESET}",
    ));
    lines.push(String::new());
    lines.push(format!(
        "  {BOLD}{:<22} {:<30} {:>8} {:>9}  result{RESET}",
        "capability", "request", "budget", "time"
    ));

    // The sentinel row first.
    lines.push(row_raw(
        "DA1 (sentinel)",
        query::PRIMARY_DEVICE_ATTRIBUTES.request(),
        da1_ok,
        da1_elapsed,
        Detail::text(match &da1 {
            Some(a) => format!("attrs {}", attrs(a)),
            None => "—".to_string(),
        }),
    ));

    lines.push(row("XTVERSION", &q_ver, ver, Detail::text));
    lines.push(row("DA2 (firmware)", &q_da2, da2, |a| {
        Detail::text(format!("attrs {}", attrs(&a)))
    }));
    lines.push(row("DA3 (unit id)", &q_da3, da3, Detail::text));
    lines.push(row("foreground color", &q_fg, fg, Detail::color));
    lines.push(row("background color", &q_bg, bg, Detail::color));
    lines.push(row("cursor color", &q_cur, cur, Detail::color));
    lines.push(row("color scheme", &q_scheme, scheme, |s| {
        Detail::text(match s {
            ColorScheme::Dark => "dark",
            ColorScheme::Light => "light",
        })
    }));
    lines.push(row(
        "kitty keyboard",
        &q_kbd,
        kbd,
        |f: KittyKeyboardFlags| Detail::text(format!("flags 0x{:02x} {f:?}", f.bits())),
    ));
    lines.push(row("kitty graphics", &q_gfx, gfx, |(_opts, payload)| {
        Detail::text(format!(
            "status {}",
            String::from_utf8_lossy(&payload).trim()
        ))
    }));
    lines.push(row("truecolor (XTGETTCAP)", &q_rgb, rgb, |s| {
        Detail::text(format!("RGB={s}"))
    }));
    lines.push(row("synchronized output", &q_sync, sync, mode_detail));
    lines.push(row("bracketed paste", &q_paste, paste, mode_detail));
    lines.push(row("cell pixel size", &q_cell, cell, |(w, h)| {
        Detail::text(format!("{w}×{h} px"))
    }));
    lines.push(row("window pixel size", &q_win, win, |(w, h)| {
        Detail::text(format!("{w}×{h} px"))
    }));
    lines.push(row("read clipboard (OSC 52)", &q_clip, clip, |s| {
        Detail::text(if s.is_empty() {
            "<empty>".to_string()
        } else {
            format!("{} bytes", s.len())
        })
    }));

    lines.push(String::new());
    lines.push(format!("  {BOLD}derived{RESET}"));
    lines.push(format!(
        "    sixel graphics : {}",
        yes_no(sixel, "advertised in DA1", "not in DA1")
    ));
    lines.push(format!(
        "    truecolor      : {}",
        yes_no(truecolor, "XTGETTCAP RGB", "no XTGETTCAP reply"),
    ));
    lines.push(format!(
        "{BOLD}{CYAN}╙──────────────────────────────────────────────────────────────────╜{RESET}"
    ));

    Ok(lines)
}

/// Issue a single query on the stream, returning its in-flight handle and
/// a copy of the request bytes (for display). The request is flushed
/// immediately, so handles accumulate in flight as we issue more.
fn issue<I, O, T>(
    stream: &EventStream<I>,
    out: &mut O,
    q: query::Single<T>,
) -> io::Result<(QueryReply<T>, Vec<u8>)>
where
    I: Input + 'static,
    O: Write,
    T: 'static,
{
    let request = q.request().to_vec();
    let handle = stream.query(out, q, BUDGET)?;
    Ok((handle, request))
}

/// Await a reply handle, recording how long it took from `t0`.
async fn timed<T: Unpin>(handle: QueryReply<T>, t0: Instant) -> (Option<T>, Duration) {
    let value = handle.await;
    (value, t0.elapsed())
}

/// Render one capability row from a typed result and its formatter.
fn row<T>(
    name: &str,
    request: &[u8],
    (value, elapsed): (Option<T>, Duration),
    fmt: impl FnOnce(T) -> Detail,
) -> String {
    match value {
        Some(v) => row_raw(name, request, true, elapsed, fmt(v)),
        None => row_raw(name, request, false, elapsed, Detail::text("no reply")),
    }
}

/// Render one row from already-resolved parts.
fn row_raw(name: &str, request: &[u8], ok: bool, elapsed: Duration, detail: Detail) -> String {
    let marker = if ok {
        format!("{GREEN}●{RESET}")
    } else {
        format!("{RED}○{RESET}")
    };
    let req = truncate(&escape(request), 30);
    let result = if ok {
        match detail.swatch {
            Some((r, g, b)) => {
                format!(
                    "\x1b[48;2;{r};{g};{b}m  {RESET} {GREEN}{}{RESET}",
                    detail.text
                )
            }
            None => format!("{GREEN}{}{RESET}", detail.text),
        }
    } else {
        format!("{DIM}{}{RESET}", detail.text)
    };
    format!(
        "{marker} {BOLD}{name:<22}{RESET} {DIM}{req:<30}{RESET} {YELLOW}{:>6}ms{RESET} {:>8.1}ms  {result}",
        BUDGET.as_millis(),
        elapsed.as_secs_f64() * 1000.0,
    )
}

fn mode_detail(setting: ModeSetting) -> Detail {
    Detail::text(match setting {
        ModeSetting::Set => "set",
        ModeSetting::Reset => "reset",
        ModeSetting::PermanentlySet => "permanently set",
        ModeSetting::PermanentlyReset => "permanently reset",
        ModeSetting::NotRecognized => "not recognized",
    })
}

fn attrs(a: &[Option<u32>]) -> String {
    let parts: Vec<String> = a
        .iter()
        .map(|x| x.map(|n| n.to_string()).unwrap_or_else(|| "·".to_string()))
        .collect();
    parts.join(";")
}

fn yes_no(cond: bool, yes: &str, no: &str) -> String {
    if cond {
        format!("{GREEN}yes{RESET} ({yes})")
    } else {
        format!("{DIM}no ({no}){RESET}")
    }
}

/// Make request bytes printable: ESC as `\e`, BEL as `\a`, other control
/// bytes as `\xNN`, printable ASCII as-is.
fn escape(bytes: &[u8]) -> String {
    let mut s = String::new();
    for &b in bytes {
        match b {
            0x1b => s.push_str("\\e"),
            0x07 => s.push_str("\\a"),
            0x20..=0x7e => s.push(b as char),
            _ => s.push_str(&format!("\\x{b:02x}")),
        }
    }
    s
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(max.saturating_sub(1)).collect();
        t.push('…');
        t
    }
}
