//! External handler registration for unrecognised sequences.
//!
//! Each escape-sequence class (CSI, SS3, OSC, DCS, APC, PM, SOS, plus a
//! catch-all for raw bytes that don't begin any known sequence) exposes a
//! chain of caller-registered hooks. A hook receives a borrowed view of the
//! parsed sequence and returns `Some(Event)` to claim it or `None` to pass.
//!
//! Hooks run **only on the fallback path** — sequences the builtin decoder
//! already recognises (e.g. cursor reports, kitty graphics, OSC 52) never
//! reach a hook, so registering one adds no overhead to the hot path.
//!
//! Multiple hooks per category are stored in registration order. The first
//! one to return `Some` wins; if none claims the sequence, the original
//! `Event::Unknown*` variant is emitted, preserving today's behaviour.
//!
//! Register through [`super::Decoder::on_csi`] (and siblings), remove with
//! [`super::Decoder::remove_handler`].

use super::Decoder;
use super::util::Params;
use crate::event::Event;

/// Opaque handle returned by `Decoder::on_*` so a hook can be removed later.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct HandlerId(u64);

/// CSI sequence view passed to hooks registered with
/// [`Decoder::on_csi`](super::Decoder::on_csi).
///
/// `raw` is the body between the introducer (`ESC [` or `0x9B`) and the
/// final byte (inclusive), exactly as it appeared on the wire.
///
/// The parameter body is exposed as a lazy [`Params`] walker through
/// [`Csi::params`]; no parameter `Vec` is ever materialised.
#[derive(Debug, Clone, Copy)]
pub struct Csi<'a> {
    pub raw: &'a [u8],
    pub private: Option<u8>,
    pub params_raw: &'a [u8],
    pub intermediates: &'a [u8],
    pub final_byte: u8,
}

impl<'a> Csi<'a> {
    /// Lazy walker over the `;`-separated parameter list. Each group
    /// may carry colon-separated sub-parameters; see
    /// [`crate::ansi::params::Params`].
    #[inline]
    pub fn params(&self) -> Params<'a> {
        Params::from_raw(self.params_raw)
    }
}

/// SS3 sequence view: a single final byte after `ESC O` (or `0x8F`).
#[derive(Debug, Clone, Copy)]
pub struct Ss3 {
    pub final_byte: u8,
}

/// OSC payload view (no `ESC ]` prefix, no terminator).
#[derive(Debug, Clone, Copy)]
pub struct Osc<'a> {
    pub payload: &'a [u8],
}

/// DCS sequence view with the parameter / intermediate / final / data
/// regions already split out. The parameter body is exposed lazily via
/// [`Dcs::params`].
#[derive(Debug, Clone, Copy)]
pub struct Dcs<'a> {
    /// Entire DCS payload between the introducer and the string terminator.
    pub raw: &'a [u8],
    pub private: Option<u8>,
    pub params_raw: &'a [u8],
    pub intermediates: &'a [u8],
    pub final_byte: u8,
    /// Bytes following the final byte, up to (but not including) the
    /// string terminator.
    pub data: &'a [u8],
}

impl<'a> Dcs<'a> {
    #[inline]
    pub fn params(&self) -> Params<'a> {
        Params::from_raw(self.params_raw)
    }
}

/// APC payload view (no `ESC _` prefix, no terminator).
#[derive(Debug, Clone, Copy)]
pub struct Apc<'a> {
    pub payload: &'a [u8],
}

/// PM payload view (no `ESC ^` prefix, no terminator).
#[derive(Debug, Clone, Copy)]
pub struct Pm<'a> {
    pub payload: &'a [u8],
}

/// SOS payload view (no `ESC X` prefix, no terminator).
#[derive(Debug, Clone, Copy)]
pub struct Sos<'a> {
    pub payload: &'a [u8],
}

type CsiHandler = Box<dyn for<'a, 'b> Fn(&'b Csi<'a>) -> Option<Event> + Send + Sync>;
type Ss3Handler = Box<dyn Fn(Ss3) -> Option<Event> + Send + Sync>;
type OscHandler = Box<dyn for<'a> Fn(Osc<'a>) -> Option<Event> + Send + Sync>;
type DcsHandler = Box<dyn for<'a, 'b> Fn(&'b Dcs<'a>) -> Option<Event> + Send + Sync>;
type ApcHandler = Box<dyn for<'a> Fn(Apc<'a>) -> Option<Event> + Send + Sync>;
type PmHandler = Box<dyn for<'a> Fn(Pm<'a>) -> Option<Event> + Send + Sync>;
type SosHandler = Box<dyn for<'a> Fn(Sos<'a>) -> Option<Event> + Send + Sync>;
type UnknownHandler = Box<dyn Fn(&[u8]) -> Option<Event> + Send + Sync>;

#[derive(Default)]
pub(super) struct Handlers {
    next_id: u64,
    csi: Vec<(HandlerId, CsiHandler)>,
    ss3: Vec<(HandlerId, Ss3Handler)>,
    osc: Vec<(HandlerId, OscHandler)>,
    dcs: Vec<(HandlerId, DcsHandler)>,
    apc: Vec<(HandlerId, ApcHandler)>,
    pm: Vec<(HandlerId, PmHandler)>,
    sos: Vec<(HandlerId, SosHandler)>,
    unknown: Vec<(HandlerId, UnknownHandler)>,
}

impl Handlers {
    fn alloc_id(&mut self) -> HandlerId {
        let id = HandlerId(self.next_id);
        self.next_id = self.next_id.wrapping_add(1);
        id
    }

    pub(super) fn dispatch_csi(&self, view: &Csi<'_>) -> Option<Event> {
        self.csi.iter().find_map(|(_, f)| f(view))
    }
    pub(super) fn dispatch_ss3(&self, view: Ss3) -> Option<Event> {
        self.ss3.iter().find_map(|(_, f)| f(view))
    }
    pub(super) fn dispatch_osc(&self, view: Osc<'_>) -> Option<Event> {
        self.osc.iter().find_map(|(_, f)| f(view))
    }
    pub(super) fn dispatch_dcs(&self, view: &Dcs<'_>) -> Option<Event> {
        self.dcs.iter().find_map(|(_, f)| f(view))
    }
    pub(super) fn dispatch_apc(&self, view: Apc<'_>) -> Option<Event> {
        self.apc.iter().find_map(|(_, f)| f(view))
    }
    pub(super) fn dispatch_pm(&self, view: Pm<'_>) -> Option<Event> {
        self.pm.iter().find_map(|(_, f)| f(view))
    }
    pub(super) fn dispatch_sos(&self, view: Sos<'_>) -> Option<Event> {
        self.sos.iter().find_map(|(_, f)| f(view))
    }
    pub(super) fn dispatch_unknown(&self, bytes: &[u8]) -> Option<Event> {
        self.unknown.iter().find_map(|(_, f)| f(bytes))
    }

    pub(super) fn remove(&mut self, id: HandlerId) -> bool {
        let mut found = false;
        macro_rules! retain_id {
            ($v:expr) => {{
                let before = $v.len();
                $v.retain(|(i, _)| *i != id);
                if $v.len() != before {
                    found = true;
                }
            }};
        }
        retain_id!(self.csi);
        retain_id!(self.ss3);
        retain_id!(self.osc);
        retain_id!(self.dcs);
        retain_id!(self.apc);
        retain_id!(self.pm);
        retain_id!(self.sos);
        retain_id!(self.unknown);
        found
    }

    pub(super) fn clear(&mut self) {
        self.csi.clear();
        self.ss3.clear();
        self.osc.clear();
        self.dcs.clear();
        self.apc.clear();
        self.pm.clear();
        self.sos.clear();
        self.unknown.clear();
    }
}

/// Split a CSI body (private prefix + params + intermediates + final byte)
/// into a structured view. `seq` must end with the final byte.
pub(super) fn split_csi(seq: &[u8]) -> Csi<'_> {
    let final_byte = *seq.last().unwrap_or(&0);
    let body = &seq[..seq.len().saturating_sub(1)];
    let (private, rest) = match body.first() {
        Some(&b) if matches!(b, b'?' | b'<' | b'>' | b'=') => (Some(b), &body[1..]),
        _ => (None, body),
    };
    let mid = rest
        .iter()
        .position(|&b| (0x20..=0x2f).contains(&b))
        .unwrap_or(rest.len());
    let (params_raw, intermediates) = rest.split_at(mid);
    Csi {
        raw: seq,
        private,
        params_raw,
        intermediates,
        final_byte,
    }
}

/// Try to split a DCS payload (between introducer and string terminator)
/// into a structured view. Returns `None` if no valid final byte is found.
pub(super) fn split_dcs(payload: &[u8]) -> Option<Dcs<'_>> {
    let (private, head_start) = match payload.first() {
        Some(&b) if matches!(b, b'?' | b'<' | b'>' | b'=') => (Some(b), 1),
        _ => (None, 0),
    };
    let mut i = head_start;
    while i < payload.len() {
        let b = payload[i];
        if (0x40..=0x7e).contains(&b) {
            let head = &payload[head_start..i];
            let mid = head
                .iter()
                .position(|&x| (0x20..=0x2f).contains(&x))
                .unwrap_or(head.len());
            let (params_raw, intermediates) = head.split_at(mid);
            return Some(Dcs {
                raw: payload,
                private,
                params_raw,
                intermediates,
                final_byte: b,
                data: &payload[i + 1..],
            });
        }
        if !((0x30..=0x3f).contains(&b) || (0x20..=0x2f).contains(&b) || b == b';' || b == b':') {
            return None;
        }
        i += 1;
    }
    None
}

impl Decoder {
    /// Register a hook for CSI sequences the builtin decoder doesn't
    /// recognise. The hook may inspect the structured view and return
    /// `Some(Event)` to claim the sequence, or `None` to let later hooks
    /// (or the default `Event::UnknownCsi` fallback) handle it.
    ///
    /// Returns a [`HandlerId`] that can be passed to
    /// [`Decoder::remove_handler`] to deregister.
    pub fn on_csi<F>(&mut self, f: F) -> HandlerId
    where
        F: for<'a, 'b> Fn(&'b Csi<'a>) -> Option<Event> + Send + Sync + 'static,
    {
        let id = self.handlers.alloc_id();
        self.handlers.csi.push((id, Box::new(f)));
        id
    }

    /// Register a hook for unrecognised SS3 sequences.
    pub fn on_ss3<F>(&mut self, f: F) -> HandlerId
    where
        F: Fn(Ss3) -> Option<Event> + Send + Sync + 'static,
    {
        let id = self.handlers.alloc_id();
        self.handlers.ss3.push((id, Box::new(f)));
        id
    }

    /// Register a hook for unrecognised OSC payloads.
    pub fn on_osc<F>(&mut self, f: F) -> HandlerId
    where
        F: for<'a> Fn(Osc<'a>) -> Option<Event> + Send + Sync + 'static,
    {
        let id = self.handlers.alloc_id();
        self.handlers.osc.push((id, Box::new(f)));
        id
    }

    /// Register a hook for unrecognised DCS payloads.
    pub fn on_dcs<F>(&mut self, f: F) -> HandlerId
    where
        F: for<'a, 'b> Fn(&'b Dcs<'a>) -> Option<Event> + Send + Sync + 'static,
    {
        let id = self.handlers.alloc_id();
        self.handlers.dcs.push((id, Box::new(f)));
        id
    }

    /// Register a hook for unrecognised APC payloads.
    pub fn on_apc<F>(&mut self, f: F) -> HandlerId
    where
        F: for<'a> Fn(Apc<'a>) -> Option<Event> + Send + Sync + 'static,
    {
        let id = self.handlers.alloc_id();
        self.handlers.apc.push((id, Box::new(f)));
        id
    }

    /// Register a hook for PM (Privacy Message) payloads.
    pub fn on_pm<F>(&mut self, f: F) -> HandlerId
    where
        F: for<'a> Fn(Pm<'a>) -> Option<Event> + Send + Sync + 'static,
    {
        let id = self.handlers.alloc_id();
        self.handlers.pm.push((id, Box::new(f)));
        id
    }

    /// Register a hook for SOS (Start Of String) payloads.
    pub fn on_sos<F>(&mut self, f: F) -> HandlerId
    where
        F: for<'a> Fn(Sos<'a>) -> Option<Event> + Send + Sync + 'static,
    {
        let id = self.handlers.alloc_id();
        self.handlers.sos.push((id, Box::new(f)));
        id
    }

    /// Register a hook for raw bytes that don't begin any recognised
    /// sequence. The slice contains the bytes that would otherwise be
    /// wrapped in `Event::Unknown`.
    pub fn on_unknown<F>(&mut self, f: F) -> HandlerId
    where
        F: Fn(&[u8]) -> Option<Event> + Send + Sync + 'static,
    {
        let id = self.handlers.alloc_id();
        self.handlers.unknown.push((id, Box::new(f)));
        id
    }

    /// Deregister a previously-registered hook by id. Returns `true` if a
    /// handler with that id was found and removed.
    pub fn remove_handler(&mut self, id: HandlerId) -> bool {
        self.handlers.remove(id)
    }

    /// Remove every registered hook across all categories.
    pub fn clear_handlers(&mut self) {
        self.handlers.clear();
    }
}
