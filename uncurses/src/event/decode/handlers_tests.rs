//! Tests for the external handler-registration API.
//!
//! Each test exercises one category's hook chain: empty (pass-through),
//! single hook claiming, single hook passing, multi-hook ordering,
//! removal, and the invariant that recognised sequences never hit a hook.

use super::*;
use crate::event::Event;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

// ---------- CSI ----------

#[test]
fn csi_hook_claims_unknown() {
    let mut d = Decoder::new(DecoderFlags::empty());
    d.on_csi(|view| {
        if view.final_byte == b'q' {
            Some(Event::Termcap(format!("csi/q params={:?}", view.params())))
        } else {
            None
        }
    });
    let events = d.parse(b"\x1b[1;2q");
    assert_eq!(events.len(), 1);
    match &events[0] {
        Event::Termcap(s) => assert_eq!(s, "csi/q params=[Some(1), Some(2)]"),
        other => panic!("expected Capability, got {:?}", other),
    }
}

#[test]
fn csi_hook_falls_through_when_returning_none() {
    let mut d = Decoder::new(DecoderFlags::empty());
    d.on_csi(|_| None);
    let events = d.parse(b"\x1b[1;2q");
    assert_eq!(events.len(), 1);
    assert!(matches!(events[0], Event::UnknownCsi(_)));
}

#[test]
fn csi_hook_overrides_recognised() {
    let mut d = Decoder::new(DecoderFlags::empty());
    let hits = std::sync::Arc::new(AtomicUsize::new(0));
    let h = hits.clone();
    d.on_csi(move |_| {
        h.fetch_add(1, Ordering::SeqCst);
        Some(Event::Unknown(b"hijacked".to_vec()))
    });
    // CSI A would normally decode as Up; the user hook overrides it.
    let events = d.parse(b"\x1b[A");
    assert_eq!(events.len(), 1);
    match &events[0] {
        Event::Unknown(b) => assert_eq!(b, b"hijacked"),
        other => panic!("expected hook override, got {:?}", other),
    }
    assert_eq!(hits.load(Ordering::SeqCst), 1);
}

#[test]
fn csi_hook_chain_first_some_wins() {
    let mut d = Decoder::new(DecoderFlags::empty());
    d.on_csi(|_| None);
    d.on_csi(|_| Some(Event::Termcap("second".into())));
    d.on_csi(|_| Some(Event::Termcap("third".into())));
    let events = d.parse(b"\x1b[99q");
    assert_eq!(events.len(), 1);
    match &events[0] {
        Event::Termcap(s) => assert_eq!(s, "second"),
        other => panic!("got {:?}", other),
    }
}

#[test]
fn csi_view_exposes_private_intermediates_and_subparams() {
    type Captured = (Option<u8>, Vec<u8>, u8, Vec<Vec<Option<u32>>>);
    let mut d = Decoder::new(DecoderFlags::empty());
    let seen: std::sync::Arc<Mutex<Option<Captured>>> = std::sync::Arc::new(Mutex::new(None));
    let s = seen.clone();
    d.on_csi(move |view| {
        let subparams: Vec<Vec<Option<u32>>> =
            view.params().iter().map(|g| g.iter().collect()).collect();
        *s.lock().unwrap() = Some((
            view.private,
            view.intermediates.to_vec(),
            view.final_byte,
            subparams,
        ));
        Some(Event::Unknown(b"ok".to_vec()))
    });
    // Private '?', params "1;3:5", intermediate '$', final 'z' (unrecognised)
    let _ = d.parse(b"\x1b[?1;3:5$z");
    let captured = seen.lock().unwrap().clone().expect("hook ran");
    assert_eq!(captured.0, Some(b'?'));
    assert_eq!(captured.1, b"$");
    assert_eq!(captured.2, b'z');
    assert_eq!(captured.3, vec![vec![Some(1)], vec![Some(3), Some(5)]]);
}

#[test]
fn csi_view_distinguishes_missing_param_from_zero() {
    let mut d = Decoder::new(DecoderFlags::empty());
    let captured: std::sync::Arc<Mutex<Option<Vec<Option<u32>>>>> =
        std::sync::Arc::new(Mutex::new(None));
    let c = captured.clone();
    d.on_csi(move |view| {
        let flat: Vec<Option<u32>> = view.params().iter().map(|g| g.first()).collect();
        *c.lock().unwrap() = Some(flat);
        Some(Event::Unknown(b"ok".to_vec()))
    });
    // First param omitted, second is explicit 0, third is 5.
    let _ = d.parse(b"\x1b[;0;5q");
    assert_eq!(
        captured.lock().unwrap().clone().unwrap(),
        vec![None, Some(0), Some(5)]
    );
}

// ---------- SS3 ----------

#[test]
fn ss3_hook_claims_unknown_final() {
    let mut d = Decoder::new(DecoderFlags::empty());
    d.on_ss3(|view| {
        if view.final_byte == b'Z' {
            Some(Event::Termcap("ss3/Z".into()))
        } else {
            None
        }
    });
    let events = d.parse(b"\x1bOZ");
    assert_eq!(events.len(), 1);
    match &events[0] {
        Event::Termcap(s) => assert_eq!(s, "ss3/Z"),
        other => panic!("got {:?}", other),
    }
}

#[test]
fn ss3_hook_overrides_recognised() {
    let mut d = Decoder::new(DecoderFlags::empty());
    d.on_ss3(|_| Some(Event::Unknown(b"hijack".to_vec())));
    let events = d.parse(b"\x1bOA"); // Up — recognised default, but hook overrides.
    match &events[0] {
        Event::Unknown(b) => assert_eq!(b, b"hijack"),
        other => panic!("got {:?}", other),
    }
}

// ---------- OSC ----------

#[test]
fn osc_hook_claims_unknown_command() {
    let mut d = Decoder::new(DecoderFlags::empty());
    d.on_osc(|view| {
        let s = std::str::from_utf8(view.payload).ok()?;
        s.strip_prefix("9001;")
            .map(|rest| Event::Termcap(format!("osc9001:{}", rest)))
    });
    let events = d.parse(b"\x1b]9001;hello\x07");
    assert_eq!(events.len(), 1);
    match &events[0] {
        Event::Termcap(s) => assert_eq!(s, "osc9001:hello"),
        other => panic!("got {:?}", other),
    }
}

#[test]
fn osc_hook_overrides_recognised() {
    let mut d = Decoder::new(DecoderFlags::empty());
    d.on_osc(|_| Some(Event::Unknown(b"hijack".to_vec())));
    // OSC 10 would normally decode to ForegroundColor; hook overrides it.
    let events = d.parse(b"\x1b]10;rgb:1111/2222/3333\x07");
    assert!(matches!(events[0], Event::Unknown(_)));
}

// ---------- DCS ----------

#[test]
fn dcs_hook_claims_with_structured_view() {
    let mut d = Decoder::new(DecoderFlags::empty());
    d.on_dcs(|view| {
        if view.final_byte == b'q' {
            Some(Event::Termcap(format!(
                "dcs/q private={:?} data={:?}",
                view.private,
                std::str::from_utf8(view.data).unwrap_or("")
            )))
        } else {
            None
        }
    });
    // Sixel-shaped: DCS 0;1 q <data> ST  (no private, q final, data after final)
    let events = d.parse(b"\x1bP0;1q#0~~\x1b\\");
    assert_eq!(events.len(), 1);
    match &events[0] {
        Event::Termcap(s) => {
            assert!(s.contains("private=None"));
            assert!(s.contains("data=\"#0~~\""));
        }
        other => panic!("got {:?}", other),
    }
}

#[test]
fn dcs_hook_overrides_xtgettcap() {
    let mut d = Decoder::new(DecoderFlags::empty());
    d.on_dcs(|_| Some(Event::Unknown(b"hijack".to_vec())));
    let events = d.parse(b"\x1bP1+r626F=31\x1b\\");
    assert!(matches!(events[0], Event::Unknown(_)));
}

// ---------- APC / PM / SOS ----------

#[test]
fn apc_hook_claims_unknown() {
    let mut d = Decoder::new(DecoderFlags::empty());
    d.on_apc(|view| Some(Event::Termcap(format!("apc:{}", view.payload.len()))));
    let events = d.parse(b"\x1b_hello\x1b\\");
    match &events[0] {
        Event::Termcap(s) => assert_eq!(s, "apc:5"),
        other => panic!("got {:?}", other),
    }
}

#[test]
fn apc_hook_overrides_kitty_graphics() {
    let mut d = Decoder::new(DecoderFlags::empty());
    d.on_apc(|_| Some(Event::Unknown(b"hijack".to_vec())));
    let events = d.parse(b"\x1b_Ga=T,f=32;DATA\x1b\\");
    assert!(matches!(events[0], Event::Unknown(_)));
}

#[test]
fn pm_and_sos_hooks_dispatch_separately() {
    let mut d = Decoder::new(DecoderFlags::empty());
    d.on_pm(|v| Some(Event::Termcap(format!("pm:{}", v.payload.len()))));
    d.on_sos(|v| Some(Event::Termcap(format!("sos:{}", v.payload.len()))));
    let events = d.parse(b"\x1b^pm-payload\x1b\\");
    match &events[0] {
        Event::Termcap(s) => assert_eq!(s, "pm:10"),
        other => panic!("got {:?}", other),
    }
    let events = d.parse(b"\x1bXsos!\x1b\\");
    match &events[0] {
        Event::Termcap(s) => assert_eq!(s, "sos:4"),
        other => panic!("got {:?}", other),
    }
}

// ---------- Raw Unknown ----------

#[test]
fn unknown_hook_registers_and_removes_cleanly() {
    // The raw Unknown path is reachable only through `parse`/`parse_one`'s
    // `consumed == 0` branch and the drain-on-expire branch — neither is
    // exercised by any byte sequence today (every leading byte in
    // `try_parse` produces a typed event). The API is exposed for
    // future-proofing; this test guards the registration / removal
    // contract so the surface stays usable.
    let mut d = Decoder::new(DecoderFlags::empty());
    let id = d.on_unknown(|_| Some(Event::Unknown(b"claimed".to_vec())));
    assert!(d.remove_handler(id));
    assert!(!d.remove_handler(id));
}

// ---------- Removal & clear ----------

#[test]
fn remove_handler_returns_true_then_false() {
    let mut d = Decoder::new(DecoderFlags::empty());
    let id = d.on_csi(|_| Some(Event::Unknown(b"x".to_vec())));
    assert!(d.remove_handler(id));
    assert!(!d.remove_handler(id));
    let events = d.parse(b"\x1b[99q");
    assert!(matches!(events[0], Event::UnknownCsi(_)));
}

#[test]
fn clear_handlers_drops_every_chain() {
    let mut d = Decoder::new(DecoderFlags::empty());
    d.on_csi(|_| Some(Event::Unknown(b"a".to_vec())));
    d.on_osc(|_| Some(Event::Unknown(b"b".to_vec())));
    d.on_unknown(|_| Some(Event::Unknown(b"c".to_vec())));
    d.clear_handlers();
    let events = d.parse(b"\x1b[99q");
    assert!(matches!(events[0], Event::UnknownCsi(_)));
}
