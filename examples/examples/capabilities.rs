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
//! It runs the *same* battery four ways, across two axes selectable on the
//! command line:
//!
//!   * **execution** — who drives the source:
//!     * `--async` — a thread-backed [`EventStream`]; replies land on its
//!       reader thread.
//!     * `--sync` — a threadless [`EventSource`]; you pump it yourself.
//!   * **method** — how replies are collected:
//!     * `--nonblocking` — issue *every* query up front (all in flight at
//!       once), then collect them; the budget is shared, so the whole
//!       sweep costs about one round-trip.
//!     * `--blocking` — issue and resolve each query in turn; every
//!       *silent* capability costs a full budget before the next begins.
//!
//! The four cells:
//!
//! | | `--nonblocking` | `--blocking` |
//! |---------|-----------------|--------------|
//! | `--async` | issue all, await each on the reader thread *(default)* | `query_blocking` on the stream, one at a time |
//! | `--sync`  | issue all, pump the source, then collect | `query_blocking` on the source, one at a time |
//!
//! Every reply also stays visible to ordinary
//! [`read`](uncurses::event::EventSource::read): a query never swallows
//! input. Run e.g. `cargo run --example capabilities -- --sync --blocking`.
//!
//! [`EventStream`]: uncurses::event::EventStream
//! [`EventSource`]: uncurses::event::EventSource

mod probe;

use std::cell::Cell;
use std::future::Future;
use std::io::{self, Write};
use std::pin::Pin;
use std::rc::Rc;
use std::time::{Duration, Instant};

use uncurses::ansi::KittyKeyboardFlags;
use uncurses::ansi::mode::Mode;
use uncurses::event::source::Input;
use uncurses::event::{ClipboardSelection, EventSource, EventStream, query};
use uncurses::terminal::Terminal;

use probe::{BUDGET, Detail, attrs, mode_detail};

const HELP: &str = "\
terminal capability probe — query the terminal across the execution × method matrix

USAGE:
    capabilities [--async|--sync] [--nonblocking|--blocking]

EXECUTION (who drives the source):
    --async    thread-backed EventStream (default)
    --sync     threadless EventSource

METHOD (how replies are collected):
    --nonblocking   issue every query up front, then collect (default)
    --blocking      issue and resolve each query in turn

    -h, --help  print this help
";

fn main() -> io::Result<()> {
    let cfg = Config::from_args();
    let app_start = Instant::now();

    let mut term = Terminal::open()?;
    term.make_raw()?;
    let report = run(&mut term, cfg);
    // Always leave raw mode before printing, whatever happened.
    term.restore()?;

    let (lines, wait) = report?;
    for line in lines {
        println!("{line}");
    }
    println!();
    println!("{}", probe::total_line(app_start.elapsed(), wait));
    Ok(())
}

// ---------------------------------------------------------------------------
// Command line
// ---------------------------------------------------------------------------

/// Which source backend drives the probe.
#[derive(Clone, Copy)]
enum Exec {
    /// Thread-backed [`EventStream`].
    Async,
    /// Threadless [`EventSource`].
    Sync,
}

/// How the probe collects each query's reply.
#[derive(Clone, Copy)]
enum Method {
    /// Issue every query up front, then collect them (concurrent).
    NonBlocking,
    /// Issue and resolve each query in turn (sequential).
    Blocking,
}

#[derive(Clone, Copy)]
struct Config {
    exec: Exec,
    method: Method,
}

impl Config {
    /// Parse the matrix cell from the command line, printing help and
    /// exiting on `--help` or an unknown flag.
    fn from_args() -> Self {
        let mut exec = Exec::Async;
        let mut method = Method::NonBlocking;
        for arg in std::env::args().skip(1) {
            match arg.as_str() {
                "--async" => exec = Exec::Async,
                "--sync" => exec = Exec::Sync,
                "--nonblocking" | "--concurrent" => method = Method::NonBlocking,
                "--blocking" | "--sequential" => method = Method::Blocking,
                "-h" | "--help" => {
                    print!("{HELP}");
                    std::process::exit(0);
                }
                other => {
                    eprintln!("unknown argument: {other}\n\n{HELP}");
                    std::process::exit(2);
                }
            }
        }
        Config { exec, method }
    }
}

impl Exec {
    fn label(self) -> &'static str {
        match self {
            Exec::Async => "async (EventStream)",
            Exec::Sync => "sync (EventSource)",
        }
    }
}

impl Method {
    fn label(self) -> &'static str {
        match self {
            Method::NonBlocking => "non-blocking (query)",
            Method::Blocking => "blocking (query_blocking)",
        }
    }
}

// ---------------------------------------------------------------------------
// Driving the matrix
// ---------------------------------------------------------------------------

fn run<I, O>(term: &mut Terminal<I, O>, cfg: Config) -> io::Result<(Vec<String>, Duration)>
where
    I: Input + Copy + 'static,
    O: Write + Copy,
{
    let (rows, sixel, truecolor, wait) = match (cfg.exec, cfg.method) {
        (Exec::Async, Method::NonBlocking) => run_async_concurrent(term)?,
        (Exec::Async, Method::Blocking) => run_sequential_stream(term)?,
        (Exec::Sync, Method::NonBlocking) => run_sync_concurrent(term)?,
        (Exec::Sync, Method::Blocking) => run_sequential_source(term)?,
    };
    Ok((assemble(cfg, rows, sixel, truecolor), wait))
}

/// `--async --nonblocking`: issue every query on the stream up front, then
/// await them all concurrently on the reader thread.
fn run_async_concurrent<I, O>(
    term: &mut Terminal<I, O>,
) -> io::Result<(Vec<String>, bool, bool, Duration)>
where
    I: Input + Copy + 'static,
    O: Write + Copy,
{
    let out = term.output();
    let stream = EventSource::new(term.input())?.into_stream();

    let t0 = Instant::now();
    let (pendings, derived) = {
        let mut driver = AsyncDriver {
            stream: &stream,
            out,
            t0,
        };
        sweep(&mut driver)?
    };

    // Drive every query future to completion concurrently: each records
    // its own reply latency the moment its slot resolves, regardless of
    // the order we collect the results in.
    let rt = tokio::runtime::Builder::new_current_thread().build()?;
    let local = tokio::task::LocalSet::new();
    let lines = local.block_on(&rt, async move {
        let tasks: Vec<_> = pendings.into_iter().map(tokio::task::spawn_local).collect();
        let mut lines = Vec::with_capacity(tasks.len());
        for task in tasks {
            lines.push(task.await.expect("probe task panicked"));
        }
        lines
    });

    let wait = t0.elapsed();
    Ok((lines, derived.sixel.get(), derived.truecolor.get(), wait))
}

/// `--sync --nonblocking`: issue every query on the threadless source up
/// front, then pump the source yourself until they all resolve.
fn run_sync_concurrent<I, O>(
    term: &mut Terminal<I, O>,
) -> io::Result<(Vec<String>, bool, bool, Duration)>
where
    I: Input + Copy,
    O: Write + Copy,
{
    let out = term.output();
    let mut src = EventSource::new(term.input())?;

    let t0 = Instant::now();
    let (pollers, derived) = {
        let mut driver = SyncConcurrentDriver {
            src: &mut src,
            out,
            t0,
        };
        sweep(&mut driver)?
    };

    let lines = pump_collect(&mut src, pollers)?;
    let wait = t0.elapsed();
    Ok((lines, derived.sixel.get(), derived.truecolor.get(), wait))
}

/// `--async --blocking`: resolve each query in turn with the stream's
/// blocking call (parking on the reader thread).
fn run_sequential_stream<I, O>(
    term: &mut Terminal<I, O>,
) -> io::Result<(Vec<String>, bool, bool, Duration)>
where
    I: Input + Copy + 'static,
    O: Write + Copy,
{
    let out = term.output();
    let stream = EventSource::new(term.input())?.into_stream();
    let mut driver = SequentialDriver {
        backend: StreamBackend(&stream),
        out,
        wait: Duration::ZERO,
    };
    let (lines, derived) = sweep(&mut driver)?;
    Ok((
        lines,
        derived.sixel.get(),
        derived.truecolor.get(),
        driver.wait,
    ))
}

/// `--sync --blocking`: resolve each query in turn, driving the threadless
/// source inline (the simplest, most sequential form).
fn run_sequential_source<I, O>(
    term: &mut Terminal<I, O>,
) -> io::Result<(Vec<String>, bool, bool, Duration)>
where
    I: Input + Copy,
    O: Write + Copy,
{
    let out = term.output();
    let mut src = EventSource::new(term.input())?;
    let mut driver = SequentialDriver {
        backend: SourceBackend(&mut src),
        out,
        wait: Duration::ZERO,
    };
    let (lines, derived) = sweep(&mut driver)?;
    Ok((
        lines,
        derived.sixel.get(),
        derived.truecolor.get(),
        driver.wait,
    ))
}

/// Pump the threadless source until every issued query resolves (matched
/// or expired), collecting each poller's rendered row.
fn pump_collect<I>(src: &mut EventSource<I>, mut pollers: Vec<Poller>) -> io::Result<Vec<String>>
where
    I: Input,
{
    // Every query was issued just before this call, so each slot expires
    // within one budget of *now*; by `now + BUDGET` they have all
    // resolved (the per-slot deadlines, issue + BUDGET, are all earlier).
    let deadline = Instant::now() + BUDGET;
    let mut done: Vec<Option<String>> = pollers.iter().map(|_| None).collect();
    loop {
        for (slot, poll) in done.iter_mut().zip(pollers.iter_mut()) {
            if slot.is_none() {
                *slot = poll();
            }
        }
        if done.iter().all(Option::is_some) {
            break;
        }
        let now = Instant::now();
        if now >= deadline {
            // Past the shared budget: every still-pending handle now
            // resolves to "silent" on its next poll.
            for (slot, poll) in done.iter_mut().zip(pollers.iter_mut()) {
                if slot.is_none() {
                    *slot = poll();
                }
            }
            break;
        }
        // Keep the queue empty so `poll` performs I/O (replies are also
        // queued; this one-shot probe discards stray input it isn't
        // matching).
        while src.try_read().is_some() {}
        src.poll(Some(deadline - now))?;
    }
    Ok(done.into_iter().map(Option::unwrap_or_default).collect())
}

/// Assemble the full report from the capability rows and derived facts.
fn assemble(cfg: Config, rows: Vec<String>, sixel: bool, truecolor: bool) -> Vec<String> {
    let mut lines = vec![
        probe::banner_top(),
        probe::mode_line(cfg.exec.label(), cfg.method.label()),
        probe::legend(),
        String::new(),
        probe::column_header(),
    ];
    lines.extend(rows);
    lines.push(String::new());
    lines.push(probe::section("derived"));
    lines.push(probe::derived(
        "sixel graphics",
        sixel,
        "advertised in DA1",
        "not in DA1",
    ));
    lines.push(probe::derived(
        "truecolor",
        truecolor,
        "XTGETTCAP RGB",
        "no XTGETTCAP reply",
    ));
    lines.push(probe::banner_bottom());
    lines
}

// ---------------------------------------------------------------------------
// The query catalog, written once
// ---------------------------------------------------------------------------

/// Issues each probe query and yields one *pending* result per query. The
/// catalog of queries (and how to decode each reply) lives here, once; the
/// four drivers differ only in what a "pending" result is and how it is
/// later collected.
trait Driver {
    /// A query in flight: a future, a poller, or — for the sequential
    /// drivers — an already-rendered row.
    type Pending;

    /// Issue `q`, returning its pending result. `f` decodes a matched
    /// reply into a presentable [`Detail`]; a timeout renders as silent.
    fn ask<T, F>(
        &mut self,
        name: &'static str,
        q: query::Single<T>,
        f: F,
    ) -> io::Result<Self::Pending>
    where
        T: 'static + Unpin,
        F: FnOnce(T) -> Detail + 'static;
}

/// Run the whole battery through `driver`, returning each query's pending
/// result plus the two derived flags (set when their reply is decoded).
fn sweep<D: Driver>(driver: &mut D) -> io::Result<(Vec<D::Pending>, Derived)> {
    let derived = Derived::default();
    let sixel = derived.sixel.clone();
    let truecolor = derived.truecolor.clone();
    let mut pending = Vec::new();

    // Sentinel first: every terminal answers DA1, so its latency is the
    // yardstick for telling unsupported from merely slow. Attribute 4 in
    // the reply advertises sixel graphics.
    pending.push({
        let sixel = sixel.clone();
        driver.ask(
            "DA1 (sentinel)",
            query::PRIMARY_DEVICE_ATTRIBUTES,
            move |a: Vec<Option<u32>>| {
                sixel.set(a.contains(&Some(4)));
                Detail::text(format!("attrs {}", attrs(&a)))
            },
        )?
    });

    pending.push(driver.ask("XTVERSION", query::TERMINAL_VERSION, Detail::text)?);
    pending.push(driver.ask(
        "DA2 (firmware)",
        query::SECONDARY_DEVICE_ATTRIBUTES,
        |a: Vec<Option<u32>>| Detail::text(format!("attrs {}", attrs(&a))),
    )?);
    pending.push(driver.ask(
        "DA3 (unit id)",
        query::TERTIARY_DEVICE_ATTRIBUTES,
        Detail::text,
    )?);
    pending.push(driver.ask("foreground color", query::FOREGROUND_COLOR, Detail::color)?);
    pending.push(driver.ask("background color", query::BACKGROUND_COLOR, Detail::color)?);
    pending.push(driver.ask("cursor color", query::CURSOR_COLOR, Detail::color)?);
    pending.push(driver.ask("color scheme", query::COLOR_SCHEME, |s| {
        Detail::text(s.to_string())
    })?);
    pending.push(driver.ask(
        "kitty keyboard",
        query::KITTY_KEYBOARD_FLAGS,
        |f: KittyKeyboardFlags| Detail::text(format!("flags 0x{:02x} {f:?}", f.bits())),
    )?);
    pending.push(driver.ask(
        "kitty graphics",
        query::kitty_graphics(&["i=1", "s=1", "v=1"]),
        |(_opts, payload): (Vec<(String, String)>, Vec<u8>)| {
            Detail::text(format!(
                "status {}",
                String::from_utf8_lossy(&payload).trim()
            ))
        },
    )?);
    pending.push({
        let truecolor = truecolor.clone();
        driver.ask(
            "truecolor (XTGETTCAP)",
            query::termcap(&["RGB"]),
            move |s: String| {
                truecolor.set(true);
                Detail::text(format!("RGB={s}"))
            },
        )?
    });
    pending.push(driver.ask(
        "synchronized output",
        query::mode(Mode::SYNCHRONIZED_OUTPUT),
        mode_detail,
    )?);
    pending.push(driver.ask(
        "bracketed paste",
        query::mode(Mode::BRACKETED_PASTE),
        mode_detail,
    )?);
    pending.push(driver.ask(
        "cell pixel size",
        query::CELL_PIXEL_SIZE,
        |(w, h): (u16, u16)| Detail::text(format!("{w}×{h} px")),
    )?);
    pending.push(driver.ask(
        "window pixel size",
        query::WINDOW_PIXEL_SIZE,
        |(w, h): (u16, u16)| Detail::text(format!("{w}×{h} px")),
    )?);
    pending.push(driver.ask(
        "read clipboard (OSC 52)",
        query::read_clipboard(ClipboardSelection::System),
        |s: String| {
            Detail::text(if s.is_empty() {
                "<empty>".to_string()
            } else {
                format!("{} bytes", s.len())
            })
        },
    )?);

    Ok((pending, derived))
}

/// Capabilities inferred from another query's reply rather than asked for
/// directly. The flags are shared into the decode closures and set when
/// the relevant reply lands.
#[derive(Default)]
struct Derived {
    /// Sixel graphics, advertised as attribute 4 in the DA1 reply.
    sixel: Rc<Cell<bool>>,
    /// Truecolor, inferred from an XTGETTCAP `RGB` reply.
    truecolor: Rc<Cell<bool>>,
}

/// Render one capability row from a typed reply and its decoder. A missing
/// value (timeout) renders as a silent row.
fn render<T, F>(
    name: &'static str,
    request: &[u8],
    elapsed: Duration,
    value: Option<T>,
    f: F,
) -> String
where
    F: FnOnce(T) -> Detail,
{
    match value {
        Some(v) => probe::row_raw(name, request, true, elapsed, f(v)),
        None => probe::row_raw(name, request, false, elapsed, Detail::text("no reply")),
    }
}

// ---------------------------------------------------------------------------
// Driver: async, concurrent (issue on the stream, await on its thread)
// ---------------------------------------------------------------------------

struct AsyncDriver<'a, I: Input, O> {
    stream: &'a EventStream<I>,
    out: O,
    t0: Instant,
}

impl<I, O> Driver for AsyncDriver<'_, I, O>
where
    I: Input + 'static,
    O: Write,
{
    type Pending = Pin<Box<dyn Future<Output = String>>>;

    fn ask<T, F>(
        &mut self,
        name: &'static str,
        q: query::Single<T>,
        f: F,
    ) -> io::Result<Self::Pending>
    where
        T: 'static + Unpin,
        F: FnOnce(T) -> Detail + 'static,
    {
        let request = q.request().to_vec();
        let t0 = self.t0;
        let handle = self.stream.query(&mut self.out, q, BUDGET)?;
        Ok(Box::pin(async move {
            let value = handle.await;
            render(name, &request, t0.elapsed(), value, f)
        }))
    }
}

// ---------------------------------------------------------------------------
// Driver: sync, concurrent (issue on the source, pump it yourself)
// ---------------------------------------------------------------------------

/// A query in flight on the threadless source: returns its rendered row
/// once its handle resolves, `None` while still pending.
type Poller = Box<dyn FnMut() -> Option<String>>;

struct SyncConcurrentDriver<'a, I: Input, O> {
    src: &'a mut EventSource<I>,
    out: O,
    t0: Instant,
}

impl<I, O> Driver for SyncConcurrentDriver<'_, I, O>
where
    I: Input,
    O: Write,
{
    type Pending = Poller;

    fn ask<T, F>(
        &mut self,
        name: &'static str,
        q: query::Single<T>,
        f: F,
    ) -> io::Result<Self::Pending>
    where
        T: 'static + Unpin,
        F: FnOnce(T) -> Detail + 'static,
    {
        let request = q.request().to_vec();
        let t0 = self.t0;
        let mut handle = self.src.query(&mut self.out, q, BUDGET)?;
        let mut f = Some(f);
        Ok(Box::new(move || {
            if let Some(value) = handle.try_take() {
                Some(render(
                    name,
                    &request,
                    t0.elapsed(),
                    Some(value),
                    f.take().unwrap(),
                ))
            } else if handle.is_ready() {
                Some(render::<T, _>(
                    name,
                    &request,
                    t0.elapsed(),
                    None,
                    f.take().unwrap(),
                ))
            } else {
                None
            }
        }))
    }
}

// ---------------------------------------------------------------------------
// Driver: sequential (resolve each query in turn, blocking)
// ---------------------------------------------------------------------------

/// The blocking, resolve-in-one-call form of a source or a stream.
trait BlockingBackend {
    fn resolve<T: 'static>(
        &mut self,
        out: &mut dyn Write,
        q: query::Single<T>,
        timeout: Duration,
    ) -> io::Result<Option<T>>;
}

struct SourceBackend<'a, I: Input>(&'a mut EventSource<I>);

impl<I: Input> BlockingBackend for SourceBackend<'_, I> {
    fn resolve<T: 'static>(
        &mut self,
        out: &mut dyn Write,
        q: query::Single<T>,
        timeout: Duration,
    ) -> io::Result<Option<T>> {
        let mut w: &mut dyn Write = out;
        self.0.query_blocking(&mut w, q, timeout)
    }
}

struct StreamBackend<'a, I: Input>(&'a EventStream<I>);

impl<I: Input + 'static> BlockingBackend for StreamBackend<'_, I> {
    fn resolve<T: 'static>(
        &mut self,
        out: &mut dyn Write,
        q: query::Single<T>,
        timeout: Duration,
    ) -> io::Result<Option<T>> {
        let mut w: &mut dyn Write = out;
        self.0.query_blocking(&mut w, q, timeout)
    }
}

struct SequentialDriver<B, O> {
    backend: B,
    out: O,
    wait: Duration,
}

impl<B, O> Driver for SequentialDriver<B, O>
where
    B: BlockingBackend,
    O: Write,
{
    type Pending = String;

    fn ask<T, F>(
        &mut self,
        name: &'static str,
        q: query::Single<T>,
        f: F,
    ) -> io::Result<Self::Pending>
    where
        T: 'static + Unpin,
        F: FnOnce(T) -> Detail + 'static,
    {
        let request = q.request().to_vec();
        let t = Instant::now();
        let value = self.backend.resolve(&mut self.out, q, BUDGET)?;
        let elapsed = t.elapsed();
        self.wait += elapsed;
        Ok(render(name, &request, elapsed, value, f))
    }
}
