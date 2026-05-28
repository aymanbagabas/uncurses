//! Refactor safety net for `decode.rs`.
//!
//! Every parser path in `Decoder` is fed through three feeding
//! strategies and the resulting event streams are asserted to be
//! byte-for-byte identical:
//!
//! 1. **whole**       — one `parse()` call with the entire input.
//! 2. **byte_by_byte** — one `parse()` call per input byte.
//! 3. **every_split** — for every offset `i` in `0..=len`, feed the
//!    head `0..i` then the tail `i..`.
//!
//! Any decomposition that holds a byte differently or changes the
//! observable event boundary will trip at least one strategy.

#![cfg(test)]

use super::Event;
use super::decode::Decoder;

/// Feed all bytes at once.
fn parse_whole(data: &[u8]) -> Vec<Event> {
    let mut p = Decoder::new();
    p.parse(data)
}

/// Feed one byte at a time.
fn parse_byte_by_byte(data: &[u8]) -> Vec<Event> {
    let mut p = Decoder::new();
    let mut events = Vec::new();
    for b in data {
        events.extend(p.parse(std::slice::from_ref(b)));
    }
    events
}

/// Feed `data[..i]` then `data[i..]`.
fn parse_split_at(data: &[u8], i: usize) -> Vec<Event> {
    let mut p = Decoder::new();
    let mut events = p.parse(&data[..i]);
    events.extend(p.parse(&data[i..]));
    events
}

/// Merge adjacent `PasteChunk` events so that chunk granularity does
/// not artificially differ under different feeding strategies. The
/// parser legitimately emits one `PasteChunk` per `parse()` call that
/// sees paste-mode bytes, so the *content* (not the boundary) is the
/// invariant we care about across feed strategies.
fn normalize(events: Vec<Event>) -> Vec<Event> {
    let mut out: Vec<Event> = Vec::with_capacity(events.len());
    for ev in events {
        if let Event::PasteChunk(bytes) = ev {
            if let Some(Event::PasteChunk(prev)) = out.last_mut() {
                prev.extend(bytes);
                continue;
            }
            out.push(Event::PasteChunk(bytes));
        } else {
            out.push(ev);
        }
    }
    out
}

/// Single canonical input — given a label and its bytes, the three
/// feeding strategies must all agree.
fn assert_split_invariant(label: &str, data: &[u8]) {
    let whole = normalize(parse_whole(data));
    let bbb = normalize(parse_byte_by_byte(data));
    assert_eq!(
        whole, bbb,
        "{label}: byte-by-byte feed disagrees with whole-buffer feed\n  whole = {whole:?}\n  b-by-b = {bbb:?}",
    );
    for i in 0..=data.len() {
        let split = normalize(parse_split_at(data, i));
        assert_eq!(
            whole, split,
            "{label}: split at offset {i} disagrees with whole-buffer feed\n  whole = {whole:?}\n  split = {split:?}",
        );
    }
}

/// Cases that exercise every reachable parser branch. Each entry is
/// fed through `assert_split_invariant`.
fn corpus() -> Vec<(&'static str, &'static [u8])> {
    vec![
        // ----- ASCII / control / UTF-8 ---------------------------------
        ("plain ascii", b"hello"),
        ("ctrl-c", b"\x03"),
        ("ctrl-a", b"\x01"),
        ("tab", b"\t"),
        ("enter cr", b"\r"),
        ("enter lf", b"\n"),
        ("backspace 0x7f", b"\x7f"),
        ("bare esc (alt none after timeout)", b"\x1b"),
        ("utf-8 two-byte (£)", "£".as_bytes()),
        ("utf-8 three-byte (€)", "€".as_bytes()),
        ("utf-8 four-byte (𝄞)", "𝄞".as_bytes()),
        ("utf-8 mixed run", "aÅ€𝄞z".as_bytes()),
        ("alt-letter", b"\x1ba"),
        ("alt-shift-letter", b"\x1bA"),
        // ----- SS3 -----------------------------------------------------
        ("ss3 F1", b"\x1bOP"),
        ("ss3 F4", b"\x1bOS"),
        // ----- CSI keys ------------------------------------------------
        ("csi arrow up", b"\x1b[A"),
        ("csi arrow up ctrl", b"\x1b[1;5A"),
        ("csi arrow up shift", b"\x1b[1;2A"),
        ("csi backtab", b"\x1b[Z"),
        ("csi home (CSI H)", b"\x1b[H"),
        ("csi end  (CSI F)", b"\x1b[F"),
        ("csi delete tilde", b"\x1b[3~"),
        ("csi pgup tilde", b"\x1b[5~"),
        ("csi f5 tilde", b"\x1b[15~"),
        ("csi f12 tilde with mods", b"\x1b[24;5~"),
        // ----- CSI focus / cursor / DECRPM / pixel / cell --------------
        ("csi focus in", b"\x1b[I"),
        ("csi focus out", b"\x1b[O"),
        ("csi cursor pos report", b"\x1b[10;42R"),
        ("csi decrpm 1004", b"\x1b[?1004;1$y"),
        ("csi pixel size", b"\x1b[4;480;640t"),
        ("csi cell size", b"\x1b[6;16;8t"),
        // ----- CSI mouse ----------------------------------------------
        ("csi sgr mouse press", b"\x1b[<0;10;5M"),
        ("csi sgr mouse release", b"\x1b[<0;10;5m"),
        ("csi sgr pixel mouse", b"\x1b[<35;200;100M"),
        ("csi urxvt mouse", b"\x1b[32;10;5M"),
        // X10 mouse is 6 raw bytes (CSI M + 3 bytes)
        ("csi x10 mouse", b"\x1b[M !!"),
        // ----- Multiple events in one stream --------------------------
        (
            "burst: text + arrow + mouse + focus",
            b"ab\x1b[Ac\x1b[<0;3;3M\x1b[I",
        ),
        // ----- OSC ----------------------------------------------------
        (
            "osc fg color ST-terminated",
            b"\x1b]10;rgb:abcd/ef01/2345\x1b\\",
        ),
        (
            "osc fg color BEL-terminated",
            b"\x1b]10;rgb:abcd/ef01/2345\x07",
        ),
        ("osc clipboard query", b"\x1b]52;c;?\x07"),
        // ----- DCS ----------------------------------------------------
        ("dcs xtversion", b"\x1bP>|uncurses 0.1.0\x1b\\"),
        ("dcs tertiary DA", b"\x1bP!|00000000\x1b\\"),
        ("dcs xtgettcap", b"\x1bP1+r626365=31\x1b\\"),
        ("dcs bel does NOT terminate", b"\x1bP1+r\x07more\x1b\\"),
        // ----- APC (kitty graphics) -----------------------------------
        ("apc kitty graphics", b"\x1b_Gf=24,s=4,v=4;AAAA\x1b\\"),
        // ----- SOS / PM ----------------------------------------------
        ("sos payload", b"\x1bXpayload\x1b\\"),
        ("pm payload", b"\x1b^payload\x1b\\"),
        // ----- Bracketed paste ----------------------------------------
        ("paste empty", b"\x1b[200~\x1b[201~"),
        ("paste hello", b"\x1b[200~hello\x1b[201~"),
        ("paste with newline", b"\x1b[200~ab\ncd\x1b[201~"),
        ("paste with escape inside", b"\x1b[200~a\x1bb\x1b[201~"),
        (
            "paste preserves control codes",
            b"\x1b[200~a\x01b\x07c\x1b[201~",
        ),
        // ----- Kitty keyboard protocol --------------------------------
        ("kitty key press u-event", b"\x1b[97;1u"),
        ("kitty key release", b"\x1b[97;1:3u"),
        ("kitty key with alternates", b"\x1b[97:65;1u"),
        ("kitty key astral codepoint", b"\x1b[119070;1u"),
        // ----- Recovery / edge ----------------------------------------
        ("two bare ESCs in a row", b"\x1b\x1b"),
        ("esc then printable", b"\x1bx"),
        ("incomplete csi then ascii", b"\x1b[ab"),
    ]
}

#[test]
fn corpus_round_trips_under_every_split() {
    for (label, data) in corpus() {
        assert_split_invariant(label, data);
    }
}

#[test]
fn corpus_concatenated_round_trips_byte_by_byte() {
    // Concatenate the whole corpus and verify that feeding the giant
    // buffer at once produces the same event stream as feeding it
    // one byte at a time. This catches cross-sequence boundary bugs
    // that per-entry tests cannot.
    let mut joined: Vec<u8> = Vec::new();
    for (_, data) in corpus() {
        joined.extend_from_slice(data);
    }
    let whole = normalize(parse_whole(&joined));
    let bbb = normalize(parse_byte_by_byte(&joined));
    assert_eq!(
        whole.len(),
        bbb.len(),
        "concatenated corpus produced different event counts: whole={} b-by-b={}",
        whole.len(),
        bbb.len(),
    );
    assert_eq!(whole, bbb, "concatenated corpus byte-by-byte mismatch");
}

#[test]
fn corpus_concatenated_round_trips_under_random_chunking() {
    // Deterministic pseudo-random chunking: feed the corpus in chunks
    // of sizes 1,3,7,2,11,5,13,4,9,6 cycling, and verify the result
    // matches a whole-buffer feed.
    let mut joined: Vec<u8> = Vec::new();
    for (_, data) in corpus() {
        joined.extend_from_slice(data);
    }
    let chunk_sizes = [1usize, 3, 7, 2, 11, 5, 13, 4, 9, 6];
    let whole = normalize(parse_whole(&joined));

    let mut p = Decoder::new();
    let mut chunked = Vec::new();
    let mut i = 0;
    let mut ci = 0;
    while i < joined.len() {
        let n = chunk_sizes[ci % chunk_sizes.len()].min(joined.len() - i);
        chunked.extend(p.parse(&joined[i..i + n]));
        i += n;
        ci += 1;
    }
    let chunked = normalize(chunked);
    assert_eq!(whole, chunked, "chunked feed disagrees with whole feed");
}

#[test]
fn empty_input_yields_no_events() {
    assert!(parse_whole(b"").is_empty());
    assert!(parse_byte_by_byte(b"").is_empty());
}
