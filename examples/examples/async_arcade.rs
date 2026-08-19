//! NEON ARCADE, tokio and uncurses sharing one task through an async event
//! stream.
//!
//! [`Program::event_stream`] hands you a real `futures_core::Stream` over the
//! screen's own decoder, so terminal input, a frame timer, and the game's
//! async tasks all merge in a single `tokio::select!`, and that same task
//! renders. No UI thread, no event channel: the `Screen` lives right here in
//! the async runtime.
//!
//! What the async side drives (each a `tokio::spawn` feeding one `UiMsg`
//! channel):
//! - a spawner task lobbing new bouncing orbs into the arena every so often,
//! - a sparkle task anointing a random menu item "special" for a while,
//! - a frenzy task that periodically speeds every orb up and rainbows them.
//!
//! The render task owns the orb physics (bouncing orbs that detonate when they
//! collide), the starfield, and the glow/pulse selection effects.
//!
//! The stream is pure: reading an event does not touch capability tracking.
//! Feed each event back through [`Program::observe_event`] so capability
//! tracking and resize handling still apply. That one line is the whole contract.
//!
//! Controls: up/down (or k/j) move the selector, Enter fires a burst on the
//! current item, Space drops an orb, q / Esc / Ctrl-C quit.
//!
//! Requires the `async` feature (on by default for the examples crate):
//! `cargo run --example async_arcade`.

use std::io;
use std::time::Duration;

use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};
use tokio_stream::StreamExt;

use uncurses::buffer::SurfaceMut;
use uncurses::cell::Cell;
use uncurses::color::Color;
use uncurses::event::{Event, Key, KeyCode};
use uncurses::layout::Position;
use uncurses::program::{Program, ProgramOptions};
use uncurses::screen::Screen;
use uncurses::style::Style;
use uncurses::terminal::{Stdin, Stdout};
use uncurses::text::TextSurface;

/// Async tasks -> the render task.
enum UiMsg {
    /// Auto-spawned orb from the spawner task; only lands below the auto
    /// threshold so the runtime never crowds out your own orbs.
    AutoOrb(u8),
    /// Anoint a menu item special until it decays (index, lifetime frames).
    Sparkle(usize, u16),
    /// Kick off a frenzy: orbs speed up and rainbow-cycle for N frames.
    Frenzy(u16),
}

/// The menu of "special items" the selection screen shows off.
const ITEMS: [&str; 5] = [
    "PLASMA SWORD",
    "PHASE BOOTS",
    "VOID CLOAK",
    " nova core ",
    "LUCKY COIN",
];

/// Fixed render cadence. ~60 fps feels smooth and idles cheaply.
// Fixed step, with no elapsed-time integration; add a dt term if you port this
// to a machine where 16ms frames visibly drift.
const FRAME: Duration = Duration::from_millis(16);

/// The spawner task stops auto-adding orbs at this count, leaving headroom for
/// player orbs up to `MAX_ORBS`.
const AUTO_ORBS: usize = 12;
const MAX_ORBS: usize = 24;

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> io::Result<()> {
    let (ui_tx, ui_rx) = unbounded_channel::<UiMsg>();

    // Async task 1: lob a fresh orb into the arena on a jittery cadence.
    let spawner = ui_tx.clone();
    tokio::spawn(async move {
        let mut rng = Rng::seed(0xA11CE);
        let mut hue = 0u8;
        loop {
            tokio::time::sleep(Duration::from_millis(900 + rng.below(700) as u64)).await;
            hue = hue.wrapping_add(1);
            if spawner.send(UiMsg::AutoOrb(hue)).is_err() {
                break;
            }
        }
    });

    // Async task 2: sprinkle "special" status on a random item.
    let sparkler = ui_tx.clone();
    tokio::spawn(async move {
        let mut rng = Rng::seed(0x5EED);
        loop {
            tokio::time::sleep(Duration::from_millis(1500)).await;
            let item = rng.below(ITEMS.len() as u32) as usize;
            if sparkler.send(UiMsg::Sparkle(item, 90)).is_err() {
                break;
            }
        }
    });

    // Async task 3: every few seconds the runtime declares a FRENZY, proof the
    // async side can drive gameplay, not just scenery.
    let frenzy = ui_tx.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(7)).await;
            if frenzy.send(UiMsg::Frenzy(180)).is_err() {
                break;
            }
        }
    });

    // Drop our spare handle so the render loop's channel closes if every task
    // dies.
    drop(ui_tx);

    let mut program = Program::stdio()?;
    program.init_with(ProgramOptions::default())?;
    program.enter_alt_screen()?;
    program.hide_cursor()?;

    let result = render_loop(&mut program, ui_rx).await;
    let finish = program.finish();
    result.and(finish)
}

/// Owns `Screen`, merges terminal input, async world messages, and the frame
/// timer in one `select!`, and renders every tick.
async fn render_loop(
    program: &mut Program<Stdin, Stdout>,
    mut ui_rx: UnboundedReceiver<UiMsg>,
) -> io::Result<()> {
    let mut world = World::new(
        program.screen().size().width,
        program.screen().size().height,
    );

    // The async input stream over the screen's own decoder. Owned, so it does
    // not borrow the screen: render and observe freely while it is live.
    let mut events = program.event_stream();
    let mut ticker = tokio::time::interval(FRAME);

    loop {
        tokio::select! {
            // Source A: terminal input, genuinely async. Reads are pure, so
            // `observe_event` runs on each so resize/capability side effects
            // still land on Screen.
            maybe = events.next() => {
                let Some(ev) = maybe else { break };
                let ev = ev?;
                program.observe_event(&ev)?;
                match ev {
                    Event::KeyPress(ref k) if world.quit_key(k) => break,
                    Event::KeyPress(key) => world.on_key(&key),
                    Event::Resize(ws) => {
                        program.screen_mut().resize((ws.col, ws.row));
                        world.resize(ws.col, ws.row);
                    }
                    _ => {}
                }
            }
            // Source B: async world messages.
            msg = ui_rx.recv() => {
                let Some(msg) = msg else { break };
                match msg {
                    UiMsg::AutoOrb(hue) => world.spawn_orb(hue, true),
                    UiMsg::Sparkle(i, life) => world.sparkle(i, life),
                    UiMsg::Frenzy(life) => world.start_frenzy(life),
                }
            }
            // Source C: the frame timer. Steps physics and renders on cadence.
            _ = ticker.tick() => {
                world.step();
                world.draw(program);
                program.screen_mut().render()?;
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// The arena: starfield, bouncing orbs with trails, and the selection menu.
// All pure state + rendering, no I/O, no async. Lives on the UI thread.
// ---------------------------------------------------------------------------

struct Orb {
    x: f32,
    y: f32,
    vx: f32,
    vy: f32,
    color: Color,
    trail: Vec<(u16, u16)>,
}

struct Star {
    x: u16,
    y: u16,
    phase: u8,
}

struct Spark {
    x: f32,
    y: f32,
    vx: f32,
    vy: f32,
    life: u16,
    color: Color,
}

struct World {
    w: u16,
    h: u16,
    frame: u64,
    rng: Rng,
    stars: Vec<Star>,
    orbs: Vec<Orb>,
    sparks: Vec<Spark>,
    selected: usize,
    /// Remaining "special" frames per menu item; 0 means ordinary.
    special: [u16; ITEMS.len()],
    /// Arcade score, climbs on bursts and orb collisions.
    score: u32,
    /// Consecutive-burst multiplier, decays if you stop firing.
    combo: u16,
    combo_timer: u16,
    /// Frames of FRENZY remaining; orbs speed up and rainbow-cycle.
    frenzy: u16,
}

impl World {
    fn new(w: u16, h: u16) -> Self {
        let mut rng = Rng::seed(0xC0FFEE);
        let stars = (0..Self::star_count(w, h))
            .map(|_| Star {
                x: rng.below(w.max(1) as u32) as u16,
                y: rng.below(h.max(1) as u32) as u16,
                phase: rng.below(255) as u8,
            })
            .collect();
        World {
            w: w.max(1),
            h: h.max(1),
            frame: 0,
            rng,
            stars,
            orbs: Vec::new(),
            sparks: Vec::new(),
            selected: 0,
            special: [0; ITEMS.len()],
            score: 0,
            combo: 0,
            combo_timer: 0,
            frenzy: 0,
        }
    }

    /// Star density: roughly one star per 60 cells, with a floor so a tiny
    /// window still twinkles.
    fn star_count(w: u16, h: u16) -> usize {
        ((w as u32 * h as u32 / 60).max(20)) as usize
    }

    /// Stretch the starfield and any in-flight sparks to the new size, then
    /// grow or shrink the star population so density stays constant. The field
    /// expands into new space and contracts when the terminal shrinks.
    fn resize(&mut self, w: u16, h: u16) {
        let (nw, nh) = (w.max(1), h.max(1));
        let (sx, sy) = (nw as f32 / self.w as f32, nh as f32 / self.h as f32);

        for s in &mut self.stars {
            s.x = ((s.x as f32 * sx) as u16).min(nw - 1);
            s.y = ((s.y as f32 * sy) as u16).min(nh - 1);
        }
        for sp in &mut self.sparks {
            sp.x *= sx;
            sp.y *= sy;
        }

        self.w = nw;
        self.h = nh;

        let target = Self::star_count(nw, nh);
        while self.stars.len() < target {
            self.stars.push(Star {
                x: self.rng.below(nw as u32) as u16,
                y: self.rng.below(nh as u32) as u16,
                phase: self.rng.below(255) as u8,
            });
        }
        self.stars.truncate(target);
    }

    fn quit_key(&self, k: &Key) -> bool {
        matches!(
            k.code,
            KeyCode::Char('q') | KeyCode::Char('Q') | KeyCode::Escape
        ) || (matches!(k.code, KeyCode::Char('c'))
            && k.modifiers.contains(uncurses::event::KeyModifiers::CTRL))
    }

    fn move_sel(&mut self, d: i16) {
        let n = ITEMS.len() as i16;
        self.selected = ((self.selected as i16 + d).rem_euclid(n)) as usize;
    }

    /// Map a keypress to a game action. Selector moves, Space drops a player
    /// orb, Enter fires a burst on the current row.
    fn on_key(&mut self, key: &Key) {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => self.move_sel(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_sel(1),
            KeyCode::Space => self.spawn_orb(self.selected as u8 * 40, false),
            KeyCode::Enter => self.burst(self.selected),
            _ => {}
        }
    }

    fn sparkle(&mut self, i: usize, life: u16) {
        if let Some(slot) = self.special.get_mut(i) {
            *slot = (*slot).max(life);
        }
    }

    fn spawn_orb(&mut self, hue: u8, auto: bool) {
        // Auto orbs plateau at AUTO_ORBS so the spawner task can't crowd the
        // arena; your Space-spawned orbs still fill up to the hard MAX_ORBS.
        if auto && self.orbs.len() >= AUTO_ORBS {
            return;
        }
        let color = wheel(hue);
        let vx = 0.4 + self.rng.frac() * 0.8;
        let vy = 0.3 + self.rng.frac() * 0.6;
        self.orbs.push(Orb {
            x: 2.0,
            y: 2.0 + self.rng.frac() * (self.h.saturating_sub(4)) as f32,
            vx: if self.rng.flip() { vx } else { -vx },
            vy: if self.rng.flip() { vy } else { -vy },
            color,
            trail: Vec::new(),
        });
        // Hard cap so a long run stays cheap; drop the oldest orb.
        if self.orbs.len() > MAX_ORBS {
            self.orbs.remove(0);
        }
    }

    fn burst(&mut self, item: usize) {
        // Fireworks over the menu, tinted by whether the item is special.
        let cx = (self.w / 2) as f32;
        let cy = (self.menu_top() + item as u16) as f32;
        let special = self.special.get(item).copied().unwrap_or(0) > 0;

        // Chained bursts build a combo; hitting a special item scores triple.
        self.combo = self.combo.saturating_add(1).min(99);
        self.combo_timer = 150;
        let base = if special { 30 } else { 10 };
        self.score += base * self.combo as u32;

        for a in 0..24 {
            let ang = a as f32 / 24.0 * std::f32::consts::TAU;
            let speed = 0.6 + self.rng.frac() * 0.9;
            self.sparks.push(Spark {
                x: cx,
                y: cy,
                vx: ang.cos() * speed * 1.8,
                vy: ang.sin() * speed,
                life: 26,
                color: if special {
                    wheel(a * 10)
                } else {
                    Color::BrightCyan
                },
            });
        }
    }

    fn menu_top(&self) -> u16 {
        self.h.saturating_sub(ITEMS.len() as u16 + 2).max(1)
    }

    fn start_frenzy(&mut self, life: u16) {
        self.frenzy = self.frenzy.max(life);
    }

    fn step(&mut self) {
        self.frame = self.frame.wrapping_add(1);
        self.frenzy = self.frenzy.saturating_sub(1);

        // Combo cools off if you stop firing.
        self.combo_timer = self.combo_timer.saturating_sub(1);
        if self.combo_timer == 0 {
            self.combo = 0;
        }

        for s in &mut self.stars {
            s.phase = s.phase.wrapping_add(3);
        }

        let (w, h) = (self.w as f32, self.h as f32);
        // Keep the play area at least one cell so clamp bounds never invert
        // on a tiny terminal.
        let (max_x, max_y) = ((w - 2.0).max(1.0), (h - 2.0).max(1.0));
        let boost = if self.frenzy > 0 { 1.7 } else { 1.0 };
        for orb in &mut self.orbs {
            orb.trail.push((orb.x as u16, orb.y as u16));
            if orb.trail.len() > 6 {
                orb.trail.remove(0);
            }
            orb.x += orb.vx * boost;
            orb.y += orb.vy * boost;
            if orb.x <= 1.0 || orb.x >= max_x {
                orb.vx = -orb.vx;
                orb.x = orb.x.clamp(1.0, max_x);
            }
            if orb.y <= 1.0 || orb.y >= max_y {
                orb.vy = -orb.vy;
                orb.y = orb.y.clamp(1.0, max_y);
            }
            if self.frenzy > 0 {
                orb.color = wheel((self.frame as u8).wrapping_add(orb.x as u8).wrapping_mul(3));
            }
        }

        // Orb-on-orb elastic bounce. O(n^2) over <=24 orbs is free, and it
        // turns a screensaver into a physics toy.
        // Swap velocities (equal mass) only when approaching, so
        // overlapping orbs don't jitter-lock; upgrade to a real 2D impulse if
        // you give orbs mass or size.
        // Orb-on-orb annihilation: when two orbs meet head-on they detonate,
        // both vanish (the orb counter drops), and a bright shockwave marks the
        // spot. O(n^2) over <=24 orbs is free.
        // Mark-then-remove so one orb can't be freed twice in a
        // multi-way pileup; upgrade to spatial hashing only past a few hundred
        // orbs, which this cap never reaches.
        let mut hits: Vec<(f32, f32, Color)> = Vec::new();
        let mut dead = vec![false; self.orbs.len()];
        for i in 0..self.orbs.len() {
            if dead[i] {
                continue;
            }
            for j in (i + 1)..self.orbs.len() {
                if dead[j] {
                    continue;
                }
                let (a, b) = (&self.orbs[i], &self.orbs[j]);
                let (dx, dy) = (b.x - a.x, b.y - a.y);
                let d2 = dx * dx + dy * dy;
                if d2 < 2.25 && (b.vx - a.vx) * dx + (b.vy - a.vy) * dy < 0.0 {
                    hits.push(((a.x + b.x) / 2.0, (a.y + b.y) / 2.0, a.color));
                    dead[i] = true;
                    dead[j] = true;
                    break;
                }
            }
        }
        if !hits.is_empty() {
            let mut k = 0;
            self.orbs.retain(|_| {
                let keep = !dead[k];
                k += 1;
                keep
            });
        }
        for (hx, hy, color) in hits {
            // Super detonations are a rare treat: only some collisions during a
            // frenzy roll the big one, so it stays special.
            let super_fx = self.frenzy > 0 && self.rng.below(4) == 0;
            self.detonate(hx, hy, color, super_fx);
        }

        self.sparks.retain_mut(|sp| {
            sp.x += sp.vx;
            sp.y += sp.vy;
            sp.vy += 0.06; // gravity
            sp.life = sp.life.saturating_sub(1);
            sp.life > 0 && sp.x >= 0.0 && sp.x < w && sp.y >= 0.0 && sp.y < h
        });

        for slot in &mut self.special {
            *slot = slot.saturating_sub(1);
        }
    }

    /// Spawn a collision detonation. A rare few during a frenzy go SUPER: a
    /// dense rainbow shockwave, a white core flash, a scatter of screen
    /// twinkles, and a fat score bonus.
    fn detonate(&mut self, x: f32, y: f32, color: Color, super_fx: bool) {
        if super_fx {
            self.score += 20;
            // Dense rainbow ring, fast and long-lived.
            for a in 0..28 {
                let ang = a as f32 / 28.0 * std::f32::consts::TAU;
                let speed = 1.4 + self.rng.frac() * 0.9;
                self.sparks.push(Spark {
                    x,
                    y,
                    vx: ang.cos() * speed,
                    vy: ang.sin() * speed * 0.7,
                    life: 26,
                    color: wheel((a as u8).wrapping_mul(9)),
                });
            }
            // White core flash punching outward.
            for a in 0..10 {
                let ang = a as f32 / 10.0 * std::f32::consts::TAU;
                self.sparks.push(Spark {
                    x,
                    y,
                    vx: ang.cos() * 0.6,
                    vy: ang.sin() * 0.5,
                    life: 20,
                    color: Color::BrightWhite,
                });
            }
            // Flashbang: a handful of twinkles scattered across the arena.
            for _ in 0..8 {
                let tx = self.rng.below(self.w as u32) as f32;
                let ty = self.rng.below(self.h as u32) as f32;
                self.sparks.push(Spark {
                    x: tx,
                    y: ty,
                    vx: 0.0,
                    vy: 0.0,
                    life: 8 + self.rng.below(8) as u16,
                    color: wheel(self.rng.below(255) as u8),
                });
            }
            return;
        }
        self.score += 5;
        // A bright white shockwave so the detonation reads instantly, with
        // a few color-tinted sparks trailing it.
        for a in 0..10 {
            let ang = a as f32 / 10.0 * std::f32::consts::TAU;
            let (speed, life, col) = if a % 2 == 0 {
                (1.1, 18, Color::BrightWhite)
            } else {
                (0.7, 10, color)
            };
            self.sparks.push(Spark {
                x,
                y,
                vx: ang.cos() * speed,
                vy: ang.sin() * speed * 0.7,
                life,
                color: col,
            });
        }
    }

    fn draw(&self, program: &mut Program<Stdin, Stdout>) {
        program.screen_mut().clear();
        self.draw_stars(program);
        self.draw_orbs(program);
        self.draw_banner(program);
        self.draw_menu(program);
        // Sparks/bursts render last, on top, but preserve whatever background
        // they land on (see `put`), so a firework over the menu bar doesn't
        // punch black holes in it.
        self.draw_sparks(program);
        self.draw_hud(program);
    }

    fn draw_stars(&self, program: &mut Program<Stdin, Stdout>) {
        for s in &self.stars {
            let (glyph, shade) = match s.phase {
                0..=90 => (".", Color::BrightBlack),
                91..=180 => ("+", Color::Blue),
                _ => ("*", Color::BrightBlue),
            };
            put(
                program.screen_mut(),
                s.x,
                s.y,
                glyph,
                Style::default().fg(shade),
            );
        }
    }

    fn draw_orbs(&self, program: &mut Program<Stdin, Stdout>) {
        for orb in &self.orbs {
            let len = orb.trail.len();
            for (i, &(tx, ty)) in orb.trail.iter().enumerate() {
                // Taper the trail from head to tail: newer cells keep the orb's
                // color and brighter glyph, older ones fade toward black.
                let faded = dim(orb.color, i as u16 + 1, len as u16 + 1);
                let glyph = if i + 2 >= len { "•" } else { "·" };
                put(
                    program.screen_mut(),
                    tx,
                    ty,
                    glyph,
                    Style::default().fg(faded),
                );
            }
            put(
                program.screen_mut(),
                orb.x as u16,
                orb.y as u16,
                "●",
                Style::default().fg(orb.color).bold(),
            );
        }
    }

    fn draw_sparks(&self, program: &mut Program<Stdin, Stdout>) {
        for sp in &self.sparks {
            let glyph = if sp.life > 16 {
                "✦"
            } else if sp.life > 8 {
                "✶"
            } else {
                "·"
            };
            put(
                program.screen_mut(),
                sp.x as u16,
                sp.y as u16,
                glyph,
                Style::default().fg(sp.color).bold(),
            );
        }
    }

    fn draw_banner(&self, program: &mut Program<Stdin, Stdout>) {
        const ART: [&str; 5] = [
            r"    _    ____   ____    _    ____  _____ ",
            r"   / \  |  _ \ / ___|  / \  |  _ \| ____|",
            r"  / _ \ | |_) | |     / _ \ | | | |  _|  ",
            r" / ___ \|  _ <| |___ / ___ \| |_| | |___ ",
            r"/_/   \_\_| \_\\____/_/   \_\____/|_____|",
        ];
        let cx = self.w.saturating_sub(ART[0].len() as u16) / 2;
        for (i, line) in ART.iter().enumerate() {
            // Cycle the banner hue over time for a marquee glow.
            let hue = ((self.frame / 2) as u8).wrapping_add(i as u8 * 24);
            program.screen_mut().set_str(
                (cx, i as u16),
                line,
                Style::default().fg(wheel(hue)).bold(),
            );
        }
    }

    fn draw_menu(&self, program: &mut Program<Stdin, Stdout>) {
        let top = self.menu_top();
        let box_w = 24u16;
        let cx = self.w.saturating_sub(box_w) / 2;

        for (i, label) in ITEMS.iter().enumerate() {
            let y = top + i as u16;
            let is_sel = i == self.selected;
            let is_special = self.special[i] > 0;

            // Selection screen effect: a pulsing reversed glow bar that
            // breathes via a sine-ish ramp off the frame counter.
            let mut style = Style::default().fg(Color::White);
            if is_sel {
                let pulse = pulse6(self.frame);
                style = Style::default()
                    .fg(Color::Black)
                    .bg(ramp_cyan(pulse))
                    .bold();
            }
            if is_special {
                style = if is_sel {
                    style.bg(ramp_gold(pulse6(self.frame)))
                } else {
                    Style::default().fg(Color::BrightYellow).bold()
                };
            }

            let marker = if is_sel { "▶" } else { " " };
            let star = if is_special { "★" } else { " " };
            let text = format!("{marker} {label:<14} {star}");
            program.screen_mut().set_str((cx, y), &text, style);

            // Twinkle a few sparkles around a special item.
            if is_special && self.frame % 6 < 3 {
                let sx = cx.saturating_sub(2);
                put(
                    program.screen_mut(),
                    sx,
                    y,
                    "✧",
                    Style::default().fg(Color::BrightYellow),
                );
                put(
                    program.screen_mut(),
                    cx + box_w,
                    y,
                    "✧",
                    Style::default().fg(Color::BrightYellow),
                );
            }
        }
    }

    fn draw_hud(&self, program: &mut Program<Stdin, Stdout>) {
        let combo = if self.combo > 1 {
            format!("  x{} combo", self.combo)
        } else {
            String::new()
        };
        let hud = format!(
            " score:{}{}  orbs:{}  ↑↓/kj move · Enter burst · Space orb · q quit ",
            self.score,
            combo,
            self.orbs.len(),
        );
        let y = self.h.saturating_sub(1);
        program
            .screen_mut()
            .set_str((0, y), &hud, Style::default().fg(Color::BrightBlack));

        // FRENZY banner flashes across the top while the runtime's frenzy runs.
        if self.frenzy > 0 && self.frame % 8 < 5 {
            let tag = "★ F R E N Z Y ★";
            let cx = self.w.saturating_sub(tag.len() as u16) / 2;
            let hue = wheel((self.frame as u8).wrapping_mul(9));
            program
                .screen_mut()
                .set_str((cx, 6), tag, Style::default().fg(hue).bold());
        }
    }
}

/// Write a single-cell glyph, ignoring out-of-bounds. Keeps the background of
/// the cell it lands on so sparks/orbs don't clobber the menu bar's fill.
fn put(screen: &mut Screen<Stdout>, x: u16, y: u16, glyph: &str, style: Style) {
    let pos = Position { x, y };
    let style = if style.bg.is_none() {
        match screen.cell_mut(pos).and_then(|c| c.style.bg) {
            Some(bg) => style.bg(bg),
            None => style,
        }
    } else {
        style
    };
    screen.set_cell(pos, &Cell::narrow(glyph).style(style));
}

/// A rainbow color wheel over a u8 so hue animations are one add away.
fn wheel(h: u8) -> Color {
    let h = h as u16 * 6;
    let seg = (h / 256) % 6;
    let t = (h % 256) as u8;
    let (r, g, b) = match seg {
        0 => (255, t, 0),
        1 => (255u8.wrapping_sub(t), 255, 0),
        2 => (0, 255, t),
        3 => (0, 255u8.wrapping_sub(t), 255),
        4 => (t, 0, 255),
        _ => (255, 0, 255u8.wrapping_sub(t)),
    };
    Color::Rgb(r, g, b)
}

/// Scale an RGB color's brightness by `num/den` for trail fades. Non-RGB
/// colors pass through unchanged (orbs are always RGB, so this is exact).
fn dim(c: Color, num: u16, den: u16) -> Color {
    if let Color::Rgb(r, g, b) = c {
        let s = |v: u8| (v as u16 * num / den) as u8;
        Color::Rgb(s(r), s(g), s(b))
    } else {
        c
    }
}

/// Triangle wave in 0..=5 for a slow breathing pulse.
fn pulse6(frame: u64) -> u8 {
    let p = (frame / 4) % 12;
    if p < 6 { p as u8 } else { (11 - p) as u8 }
}

fn ramp_cyan(p: u8) -> Color {
    let v = 90 + p * 26;
    Color::Rgb(0, v, v)
}

fn ramp_gold(p: u8) -> Color {
    let v = 120 + p * 22;
    Color::Rgb(v, (v as u16 * 3 / 4) as u8, 0)
}

/// Tiny xorshift PRNG so the demo needs no `rand` dependency.
struct Rng(u64);

impl Rng {
    fn seed(s: u64) -> Self {
        Rng(s | 1)
    }
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn below(&mut self, n: u32) -> u32 {
        if n == 0 {
            0
        } else {
            (self.next_u64() % n as u64) as u32
        }
    }
    fn frac(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }
    fn flip(&mut self) -> bool {
        self.next_u64() & 1 == 0
    }
}
