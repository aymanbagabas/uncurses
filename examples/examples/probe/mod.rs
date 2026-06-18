//! Shared presentation for the capability probe.
//!
//! The `capabilities` example drives the same battery of real terminal
//! queries four ways — across an execution axis (thread-backed
//! `EventStream` vs threadless `EventSource`) and a method axis
//! (`query` vs `query_blocking`) — and renders the same report whichever
//! way it ran. This module holds everything the four paths have in
//! common: the SGR tokens, the row/section formatting, and the small
//! decoders used to turn a typed reply into a presentable [`Detail`].

use std::sync::LazyLock;
use std::time::Duration;

use uncurses::ansi::mode::ModeSetting;
use uncurses::color::{BasicColor, Color};
use uncurses::style::Style;

/// Per-query budget. The DA1 sentinel makes this safe to keep short: we
/// rely on the sentinel — not a generous timeout — to tell unsupported
/// from slow.
pub const BUDGET: Duration = Duration::from_millis(200);

/// An SGR token derived from a [`Style`], computed once on first use.
/// Wrapping the lazy value lets these be module-level `static`s that
/// still drop straight into `format!`'s named arguments (e.g. `{BOLD}`).
struct Sgr(LazyLock<String>);

impl std::fmt::Display for Sgr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0.as_str())
    }
}

const fn sgr(f: fn() -> String) -> Sgr {
    Sgr(LazyLock::new(f))
}

fn fg(c: BasicColor) -> Style {
    Style::EMPTY.fg(Color::Basic(c))
}

// SGR tokens, each rendered from a `Style` rather than a hand-written
// escape. An empty style renders as the reset sequence.
static RESET: Sgr = sgr(|| Style::EMPTY.to_string());
static BOLD: Sgr = sgr(|| Style::EMPTY.bold().to_string());
static DIM: Sgr = sgr(|| Style::EMPTY.faint().to_string());
static RED: Sgr = sgr(|| fg(BasicColor::Red).to_string());
static GREEN: Sgr = sgr(|| fg(BasicColor::Green).to_string());
static YELLOW: Sgr = sgr(|| fg(BasicColor::Yellow).to_string());
static CYAN: Sgr = sgr(|| fg(BasicColor::Cyan).to_string());

/// A solid background swatch in true color, derived from a [`Style`].
fn swatch(r: u8, g: u8, b: u8) -> String {
    Style::EMPTY
        .bg(Color::Rgb(r, g, b))
        .styled("  ")
        .to_string()
}

/// The decoded, presentable form of one reply.
pub struct Detail {
    text: String,
    /// An RGB swatch to render alongside the text (color queries only).
    swatch: Option<(u8, u8, u8)>,
}

impl Detail {
    pub fn text(s: impl Into<String>) -> Self {
        Self {
            text: s.into(),
            swatch: None,
        }
    }

    pub fn color(c: Color) -> Self {
        let (r, g, b) = c.to_rgb();
        Self {
            text: format!("{c:?}"),
            swatch: Some((r, g, b)),
        }
    }
}

// ---- Report chrome --------------------------------------------------------

/// The report's top banner.
pub fn banner_top() -> String {
    format!(
        "{BOLD}{CYAN}╓─ terminal capability probe ──────────────────────────────────────╖{RESET}"
    )
}

/// The legend describing the markers and budget.
pub fn legend() -> String {
    format!(
        "{DIM}  budget {BUDGET:?} per query · {GREEN}●{DIM} answered · {RED}○{DIM} silent · sentinel = DA1{RESET}",
    )
}

/// A line naming which matrix cell produced this report.
pub fn mode_line(exec: &str, method: &str) -> String {
    format!("{DIM}  driving · {CYAN}{exec}{DIM} execution · {CYAN}{method}{DIM} method{RESET}")
}

/// The column header above the capability rows.
pub fn column_header() -> String {
    format!(
        "  {BOLD}{:<23} {:<30} {:>8} {:>9}  result{RESET}",
        "capability", "request", "budget", "time"
    )
}

/// A bold section heading.
pub fn section(title: &str) -> String {
    format!("  {BOLD}{title}{RESET}")
}

/// A derived-capability line: a padded label and a yes/no verdict.
pub fn derived(label: &str, cond: bool, yes: &str, no: &str) -> String {
    format!("    {label:<14} : {}", yes_no(cond, yes, no))
}

/// The report's bottom banner.
pub fn banner_bottom() -> String {
    format!(
        "{BOLD}{CYAN}╙──────────────────────────────────────────────────────────────────╜{RESET}"
    )
}

/// The closing summary: wall-clock time for the whole run and how much of
/// it was spent blocked on the terminal.
pub fn total_line(total: Duration, wait: Duration) -> String {
    let share = if total.as_secs_f64() > 0.0 {
        wait.as_secs_f64() / total.as_secs_f64() * 100.0
    } else {
        0.0
    };
    format!(
        "  {BOLD}total{RESET}  app {YELLOW}{:>8.1}ms{RESET} · waiting on terminal {YELLOW}{:>8.1}ms{RESET} {DIM}({share:.0}% of run){RESET}",
        total.as_secs_f64() * 1000.0,
        wait.as_secs_f64() * 1000.0,
    )
}

// ---- Rows -----------------------------------------------------------------

/// Render one row from already-resolved parts.
pub fn row_raw(name: &str, request: &[u8], ok: bool, elapsed: Duration, detail: Detail) -> String {
    let marker = if ok {
        format!("{GREEN}●{RESET}")
    } else {
        format!("{RED}○{RESET}")
    };
    let req = truncate(&escape(request), 30);
    let result = if ok {
        match detail.swatch {
            Some((r, g, b)) => format!("{} {GREEN}{}{RESET}", swatch(r, g, b), detail.text),
            None => format!("{GREEN}{}{RESET}", detail.text),
        }
    } else {
        format!("{DIM}{}{RESET}", detail.text)
    };
    format!(
        "{marker} {BOLD}{name:<23}{RESET} {DIM}{req:<30}{RESET} {YELLOW}{:>6}ms{RESET} {:>8.1}ms  {result}",
        BUDGET.as_millis(),
        elapsed.as_secs_f64() * 1000.0,
    )
}

// ---- Reply decoders -------------------------------------------------------

/// Format a mode-report setting.
pub fn mode_detail(setting: ModeSetting) -> Detail {
    Detail::text(setting.to_string())
}

/// Join device-attribute parameters (`Some(n)` numbers, `None` blanks).
pub fn attrs(a: &[Option<u32>]) -> String {
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
