//! Program status reports and feature detection through OSC 7501.
//!
//! Each report replaces one terminal record. An absent ID selects the root.
//! A clear report removes its record and descendants; clearing the root
//! removes all records. Screen switches leave records intact. Done and error
//! records survive process exit and the next shell prompt.
//!
//! Writers use `ESC ] 7501 ; pairs ESC \`. Messages and titles are standard
//! base64 of UTF-8. Unsupported terminals ignore these sequences.
//!
//! See the [protocol specification](https://www.superlogical.com/rex/docs/build/program-status).

use std::io::{self, Write};

use super::clipboard::base64_encode;

/// Query OSC 7501 support. The reply starts with `?` and may list accepted
/// states and kinds.
pub const REQUEST_PROGRAM_STATUS: &[u8] = b"\x1b]7501;?\x1b\\";

/// State of a program or one of its tasks.
///
/// Standard names are associated constants. Use [`Self::new`] for a name
/// advertised by a protocol extension. The writer validates names before
/// output; construction does not establish terminal support.
///
/// State and kind names are distinct types:
///
/// ```compile_fail
/// use uncurses::ansi::program_status::{ProgramStatus, ProgramStatusKind};
///
/// let report = ProgramStatus::new(ProgramStatusKind::AUTH);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ProgramStatusState<'a>(&'a str);

impl<'a> ProgramStatusState<'a> {
    /// At rest, ready for the user's next instruction.
    pub const IDLE: Self = Self("idle");
    /// Work is in progress.
    pub const WORKING: Self = Self("working");
    /// Work finished and its result is ready.
    pub const DONE: Self = Self("done");
    /// Work requires user input before it can continue.
    pub const BLOCKED: Self = Self("blocked");
    /// Work failed and stopped.
    pub const ERROR: Self = Self("error");
    /// Remove the addressed record and all descendants.
    pub const CLEAR: Self = Self("clear");

    /// Borrow a state name without validation.
    ///
    /// Check the terminal's accepted states before use. The writer accepts
    /// nonempty values from `[A-Za-z0-9_.,+/=-]` within the sequence limit.
    pub const fn new(name: &'a str) -> Self {
        Self(name)
    }

    /// Return the state name used on the wire.
    pub const fn as_str(self) -> &'a str {
        self.0
    }
}

/// What a blocked program needs from the user.
///
/// Standard names are associated constants. Use [`Self::new`] for a name
/// advertised by a protocol extension. The writer validates names before
/// output; construction does not establish terminal support.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ProgramStatusKind<'a>(&'a str);

impl<'a> ProgramStatusKind<'a> {
    /// Approval for an action.
    pub const PERMISSION: Self = Self("permission");
    /// An answer to a question.
    pub const QUESTION: Self = Self("question");
    /// Authentication, such as a login or credential.
    pub const AUTH: Self = Self("auth");

    /// Borrow a kind name without validation.
    ///
    /// Check the terminal's accepted kinds before use. The writer accepts
    /// nonempty values from `[A-Za-z0-9_.,+/=-]` within the sequence limit.
    pub const fn new(name: &'a str) -> Self {
        Self(name)
    }

    /// Return the kind name used on the wire.
    pub const fn as_str(self) -> &'a str {
        self.0
    }
}

/// A complete OSC 7501 report, with borrowed text.
///
/// Each report replaces its record, so include every field that should
/// remain present. An absent `app` inherits from the nearest ancestor with
/// one. Text and identifiers are validated by [`write_program_status`]
/// before any output. Callers supply plain UTF-8, not base64.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgramStatus<'a> {
    /// Required record state.
    pub state: ProgramStatusState<'a>,
    /// Optional record path. At most 128 bytes and eight slash-separated
    /// segments, each 1-32 bytes from `[A-Za-z0-9_.+-]`. `None` selects root.
    pub id: Option<&'a str>,
    /// Reason for blocking. Terminals use this only with state `blocked`.
    pub kind: Option<ProgramStatusKind<'a>>,
    /// Percentage, from 0 to 100. Terminals use this only with state `working`
    /// or `blocked`. `None` means indeterminate progress.
    pub progress: Option<u8>,
    /// Program name: 1-32 bytes from `[A-Za-z0-9_.+-]`.
    pub app: Option<&'a str>,
    /// Human-readable record label, at most 192 UTF-8 bytes.
    /// C0, DEL, and C1 control characters are rejected.
    pub title: Option<&'a str>,
    /// Human-readable status message, at most 2048 UTF-8 bytes.
    /// C0, DEL, and C1 control characters are rejected.
    pub message: Option<&'a str>,
}

impl<'a> ProgramStatus<'a> {
    /// Create a root report with the specified state and no optional fields.
    pub fn new(state: ProgramStatusState<'a>) -> Self {
        Self {
            state,
            id: None,
            kind: None,
            progress: None,
            app: None,
            title: None,
            message: None,
        }
    }
}

pub(crate) fn valid_value(value: &str) -> bool {
    value
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b"_.,+/=-".contains(&b))
}

fn valid_segment(value: &str) -> bool {
    (1..=32).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.+-".contains(&b))
}

fn validate_text(value: Option<&str>, limit: usize, field: &str) -> io::Result<()> {
    if value.is_some_and(|s| s.len() > limit || s.chars().any(char::is_control)) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "program status {field} exceeds {limit} UTF-8 bytes or contains a control character"
            ),
        ));
    }
    Ok(())
}

/// Write one complete program status report without flushing.
///
/// All fields are validated before any bytes are written. Invalid IDs,
/// state names, kind names, app names, text, or percentages return
/// [`io::ErrorKind::InvalidInput`]. State and kind names must be nonempty
/// values from `[A-Za-z0-9_.,+/=-]`.
/// Text limits count UTF-8 bytes before base64 encoding. The complete
/// encoded sequence, including its framing, must fit within 4096 bytes.
///
/// A terminal may apply lower limits or ignore OSC 7501 entirely. This
/// function reports write errors, not terminal acceptance.
///
/// ```
/// use uncurses::ansi::program_status::{
///     ProgramStatus, ProgramStatusState, write_program_status,
/// };
///
/// let mut report = ProgramStatus::new(ProgramStatusState::WORKING);
/// report.app = Some("cargo");
/// report.message = Some("Build");
/// let mut bytes = Vec::new();
/// write_program_status(&mut bytes, &report).unwrap();
/// assert_eq!(bytes, b"\x1b]7501;state=working:app=cargo:msg=QnVpbGQ=\x1b\\");
/// ```
pub fn write_program_status<W: Write>(w: &mut W, report: &ProgramStatus<'_>) -> io::Result<()> {
    for (field, value) in [
        ("state", Some(report.state.as_str())),
        ("kind", report.kind.map(ProgramStatusKind::as_str)),
    ] {
        if value.is_some_and(|name| name.is_empty() || name.len() > 4096 || !valid_value(name)) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("invalid program status {field} name"),
            ));
        }
    }
    if report.id.is_some_and(|id| {
        id.len() > 128 || id.split('/').count() > 8 || !id.split('/').all(valid_segment)
    }) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid program status id",
        ));
    }
    if report.app.is_some_and(|app| !valid_segment(app)) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid program status app",
        ));
    }
    if report.progress.is_some_and(|progress| progress > 100) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "program status progress exceeds 100",
        ));
    }
    validate_text(report.title, 192, "title")?;
    validate_text(report.message, 2048, "message")?;

    let mut sequence = String::from("\x1b]7501;state=");
    sequence.push_str(report.state.as_str());
    if let Some(id) = report.id {
        sequence.push_str(":id=");
        sequence.push_str(id);
    }
    if let Some(kind) = report.kind {
        sequence.push_str(":kind=");
        sequence.push_str(kind.as_str());
    }
    if let Some(progress) = report.progress {
        sequence.push_str(":progress=");
        sequence.push_str(&progress.to_string());
    }
    if let Some(app) = report.app {
        sequence.push_str(":app=");
        sequence.push_str(app);
    }
    if let Some(title) = report.title {
        sequence.push_str(":title=");
        sequence.push_str(&base64_encode(title.as_bytes()));
    }
    if let Some(message) = report.message {
        sequence.push_str(":msg=");
        sequence.push_str(&base64_encode(message.as_bytes()));
    }
    sequence.push_str("\x1b\\");
    if sequence.len() > 4096 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "program status sequence exceeds 4096 bytes",
        ));
    }
    w.write_all(sequence.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn program_status_names_have_value_equality() {
        use std::collections::HashSet;

        for state in [
            ProgramStatusState::IDLE,
            ProgramStatusState::WORKING,
            ProgramStatusState::DONE,
            ProgramStatusState::BLOCKED,
            ProgramStatusState::ERROR,
            ProgramStatusState::CLEAR,
        ] {
            let name = state.as_str().to_owned();
            let constructed = ProgramStatusState::new(&name);
            assert_eq!(state, constructed);
            assert_eq!(HashSet::from([state, constructed]).len(), 1);
        }
        for kind in [
            ProgramStatusKind::PERMISSION,
            ProgramStatusKind::QUESTION,
            ProgramStatusKind::AUTH,
        ] {
            let name = kind.as_str().to_owned();
            let constructed = ProgramStatusKind::new(&name);
            assert_eq!(kind, constructed);
            assert_eq!(HashSet::from([kind, constructed]).len(), 1);
        }
        assert_ne!(
            ProgramStatusState::new("paused"),
            ProgramStatusState::WORKING
        );
        assert_ne!(ProgramStatusKind::new("confirm"), ProgramStatusKind::AUTH);
    }

    #[test]
    fn program_status_states_and_clear_records() {
        for (state, value) in [
            (ProgramStatusState::IDLE, "idle"),
            (ProgramStatusState::WORKING, "working"),
            (ProgramStatusState::DONE, "done"),
            (ProgramStatusState::BLOCKED, "blocked"),
            (ProgramStatusState::ERROR, "error"),
            (ProgramStatusState::CLEAR, "clear"),
        ] {
            let mut report = ProgramStatus::new(state);
            for id in [None, Some("build/test")] {
                report.id = id;
                let mut bytes = Vec::new();
                write_program_status(&mut bytes, &report).unwrap();
                let suffix = if id.is_some() { ":id=build/test" } else { "" };
                assert_eq!(
                    bytes,
                    format!("\x1b]7501;state={value}{suffix}\x1b\\").as_bytes()
                );
            }
        }
    }

    #[test]
    fn program_status_encodes_all_fields_and_blocked_kinds() {
        for (kind, value) in [
            (ProgramStatusKind::PERMISSION, "permission"),
            (ProgramStatusKind::QUESTION, "question"),
            (ProgramStatusKind::AUTH, "auth"),
        ] {
            let report = ProgramStatus {
                state: ProgramStatusState::BLOCKED,
                id: Some("build/test"),
                kind: Some(kind),
                progress: Some(42),
                app: Some("cargo"),
                title: Some("Test"),
                message: Some("Ready \u{2713}: yes?"),
            };
            let original = report.clone();
            let mut bytes = Vec::new();
            write_program_status(&mut bytes, &report).unwrap();
            assert_eq!(
                bytes,
                format!("\x1b]7501;state=blocked:id=build/test:kind={value}:progress=42:app=cargo:title=VGVzdA==:msg=UmVhZHkg4pyTOiB5ZXM/\x1b\\").as_bytes(),
            );
            assert_eq!(report, original);
        }
    }

    fn invalid(report: &ProgramStatus<'_>) {
        let mut bytes = Vec::new();
        let error = write_program_status(&mut bytes, report).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert!(bytes.is_empty(), "invalid reports must emit nothing");
    }

    #[test]
    fn program_status_accepts_borrowed_extension_names() {
        let state_name = String::from("paused");
        let kind_name = String::from("confirm");
        let mut report = ProgramStatus::new(ProgramStatusState::new(&state_name));
        let mut bytes = Vec::new();
        write_program_status(&mut bytes, &report).unwrap();
        assert_eq!(bytes, b"\x1b]7501;state=paused\x1b\\");
        assert_eq!(report.state.as_str(), state_name);

        report.state = ProgramStatusState::BLOCKED;
        report.kind = Some(ProgramStatusKind::new(&kind_name));
        let original = report.clone();
        bytes.clear();
        write_program_status(&mut bytes, &report).unwrap();
        assert_eq!(bytes, b"\x1b]7501;state=blocked:kind=confirm\x1b\\");
        assert_eq!(report.kind.unwrap().as_str(), kind_name);
        assert_eq!(report, original);

        report.state = ProgramStatusState::new("AZaz09_.,+/=-");
        report.kind = Some(ProgramStatusKind::new("AZaz09_.,+/=-"));
        bytes.clear();
        write_program_status(&mut bytes, &report).unwrap();
        assert_eq!(
            bytes,
            b"\x1b]7501;state=AZaz09_.,+/=-:kind=AZaz09_.,+/=-\x1b\\"
        );
    }

    #[test]
    fn program_status_preserves_commas_in_report_values() {
        for name in [",", ",paused", "paused,", "paused,done"] {
            let mut report = ProgramStatus::new(ProgramStatusState::new(name));
            let mut bytes = Vec::new();
            write_program_status(&mut bytes, &report).unwrap();
            assert_eq!(bytes, format!("\x1b]7501;state={name}\x1b\\").as_bytes());

            report.state = ProgramStatusState::BLOCKED;
            report.kind = Some(ProgramStatusKind::new(name));
            bytes.clear();
            write_program_status(&mut bytes, &report).unwrap();
            assert_eq!(
                bytes,
                format!("\x1b]7501;state=blocked:kind={name}\x1b\\").as_bytes()
            );
        }
    }

    #[test]
    fn program_status_rejects_invalid_extension_names_before_output() {
        for name in [
            "", "a:b", "a;b", "a b", "a\tb", "a\nb", "\x1b\\", "\u{e9}", "\u{9c}",
        ] {
            invalid(&ProgramStatus::new(ProgramStatusState::new(name)));
            let mut report = ProgramStatus::new(ProgramStatusState::BLOCKED);
            report.kind = Some(ProgramStatusKind::new(name));
            invalid(&report);
        }
        for code in (0..=0x1f).chain(0x7f..=0x9f) {
            let name = format!("a{}b", char::from_u32(code).unwrap());
            invalid(&ProgramStatus::new(ProgramStatusState::new(&name)));
            let mut report = ProgramStatus::new(ProgramStatusState::BLOCKED);
            report.kind = Some(ProgramStatusKind::new(&name));
            invalid(&report);
        }
    }

    #[test]
    fn program_status_extension_names_respect_complete_sequence_limit() {
        for (kind, overhead) in [
            (false, b"\x1b]7501;state=\x1b\\".len()),
            (true, b"\x1b]7501;state=blocked:kind=\x1b\\".len()),
        ] {
            for extra in [0, 1, overhead + 1] {
                let name = "a".repeat(4096 - overhead + extra);
                let mut report = ProgramStatus::new(if kind {
                    ProgramStatusState::BLOCKED
                } else {
                    ProgramStatusState::new(&name)
                });
                if kind {
                    report.kind = Some(ProgramStatusKind::new(&name));
                }
                if extra == 0 {
                    let mut bytes = Vec::new();
                    write_program_status(&mut bytes, &report).unwrap();
                    assert_eq!(bytes.len(), 4096);
                    report.title = Some("x");
                    invalid(&report);
                } else {
                    invalid(&report);
                }
            }
        }

        let name = "a".repeat(2100);
        let mut report = ProgramStatus::new(ProgramStatusState::new(&name));
        report.kind = Some(ProgramStatusKind::new(&name));
        invalid(&report);
    }

    #[test]
    fn program_status_rejects_invalid_ids_apps_and_progress() {
        let mut report = ProgramStatus::new(ProgramStatusState::WORKING);
        for id in ["", "/a", "a/", "a//b", "a:b", "a;b", "a=b", "a b", "\u{e9}"] {
            report.id = Some(id);
            invalid(&report);
        }
        let segment = "a".repeat(33);
        let levels = ["a"; 9].join("/");
        let long_id = vec!["a".repeat(25); 5].join("/");
        for id in [&segment, &levels, &long_id] {
            report.id = Some(id);
            invalid(&report);
        }
        report.id = None;
        for app in ["", "a/b", "a b", "a:b", "a=b", "\u{e9}", &segment] {
            report.app = Some(app);
            invalid(&report);
        }
        report.app = None;
        for progress in [101, 255] {
            report.progress = Some(progress);
            invalid(&report);
        }
    }

    #[test]
    fn program_status_rejects_controls_and_text_over_limits() {
        for code in (0..=0x1f).chain(0x7f..=0x9f) {
            let text = format!("before{}after", char::from_u32(code).unwrap());
            let mut report = ProgramStatus::new(ProgramStatusState::DONE);
            report.title = Some(&text);
            invalid(&report);
            report.title = None;
            report.message = Some(&text);
            invalid(&report);
            report.message = None;
        }
        for title in ["a".repeat(193), "\u{e9}".repeat(97)] {
            let mut report = ProgramStatus::new(ProgramStatusState::DONE);
            report.title = Some(&title);
            invalid(&report);
        }
        for message in ["a".repeat(2049), "\u{e9}".repeat(1025)] {
            let mut report = ProgramStatus::new(ProgramStatusState::DONE);
            report.message = Some(&message);
            invalid(&report);
        }
    }

    #[test]
    fn program_status_accepts_limits_and_empty_text() {
        let id = format!(
            "{}/{}/{}/{}",
            "a".repeat(32),
            "b".repeat(32),
            "c".repeat(32),
            "d".repeat(29)
        );
        assert_eq!(id.len(), 128);
        let app = "a".repeat(32);
        let title = "\u{e9}".repeat(96);
        let message = "\u{e9}".repeat(1024);
        let mut report = ProgramStatus {
            state: ProgramStatusState::BLOCKED,
            id: Some(&id),
            kind: Some(ProgramStatusKind::PERMISSION),
            progress: Some(100),
            app: Some(&app),
            title: Some(&title),
            message: Some(&message),
        };
        let mut bytes = Vec::new();
        write_program_status(&mut bytes, &report).unwrap();
        assert!(bytes.len() < 4096);
        let sequence = String::from_utf8(bytes).unwrap();
        assert_eq!(
            sequence
                .split(":title=")
                .nth(1)
                .unwrap()
                .split(':')
                .next()
                .unwrap()
                .len(),
            256
        );
        assert_eq!(
            sequence
                .split(":msg=")
                .nth(1)
                .unwrap()
                .trim_end_matches("\x1b\\")
                .len(),
            2732
        );

        report.id = Some("a/b/c/d/e/f/g/h");
        report.app = Some("AZaz09_.+-");
        report.title = Some("");
        report.message = Some("");
        report.progress = Some(0);
        let mut bytes = Vec::new();
        write_program_status(&mut bytes, &report).unwrap();
        assert!(bytes.ends_with(b":title=:msg=\x1b\\"));
    }

    #[test]
    fn program_status_propagates_write_errors() {
        let mut empty = [];
        let error = write_program_status(
            &mut empty.as_mut_slice(),
            &ProgramStatus::new(ProgramStatusState::IDLE),
        )
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::WriteZero);
    }
}
