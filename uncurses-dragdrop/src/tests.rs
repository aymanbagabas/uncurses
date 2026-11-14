use super::*;
use crate::controller::DragPhase;
use crate::protocol::{BASE64, value};
use base64::Engine;
use std::{
    ffi::OsStr,
    fs,
    io::{self, Read, Write},
    path::Path,
};
use uncurses::event::Event as RawEvent;

#[path = "file_tests.rs"]
mod files;

const MIME: &str = "text/plain";

#[test]
fn control_limit_can_be_changed_or_disabled() {
    for limit in [Some(8), None] {
        let mut controller = DragDrop::new(DragDropOptions {
            max_control_bytes: limit,
            ..DragDropOptions::default()
        });
        assert!(handle(&mut controller, "t=unknown:m=1", b"12345678").is_empty());
        let result = controller.handle_event(&raw("m=0", b"9"));
        assert_eq!(
            matches!(result, Err(Error::LimitExceeded(_))),
            limit.is_some()
        );
    }
}

#[test]
fn oversized_control_continuations_are_drained_after_registration_changes() {
    let mut controller = dropping();
    assert!(
        controller
            .handle_event(&raw("t=R:m=1", &vec![b'a'; 65537]))
            .is_err()
    );
    assert!(controller.request.is_none());
    controller.close(&mut Vec::new()).unwrap();
    controller.enable_drops(&mut Vec::new(), &[MIME]).unwrap();
    assert!(handle(&mut controller, "t=M:o=1:m=1", MIME.as_bytes()).is_empty());
    assert!(handle(&mut controller, "t=M:o=1", b"").is_empty());
    assert!(controller.dropped.is_none());
    assert!(matches!(
        handle(&mut controller, "t=M:o=1", MIME.as_bytes()).as_slice(),
        [Event::Drop(_)]
    ));
}

#[test]
fn malformed_replacement_preserves_the_active_drop_for_cleanup() {
    for (metadata, payload) in [
        ("t=m:x=invalid:o=1", MIME.as_bytes()),
        ("t=m:x=-1:y=invalid", b""),
        ("t=m:x=-1:y=-1", MIME.as_bytes()),
        ("t=M:o=4", MIME.as_bytes()),
        ("t=M:o=1", b"text/plain\0"),
    ] {
        let mut controller = dropping();
        handle(&mut controller, "t=r:x=1", b"YQ");
        assert!(controller.handle_event(&raw(metadata, payload)).is_err());
        assert!(controller.dropped.is_some());
        assert!(controller.request.is_some());
        let mut out = Vec::new();
        controller.cancel_drop(&mut out).unwrap();
        assert_eq!(out, b"\x1b]72;t=r:o=0\x1b\\");
    }
}

#[test]
fn assembled_control_messages_are_bounded_but_data_streams_are_not() {
    let chunk = vec![b'a'; 4096];
    for kind in ["m", "unknown"] {
        let mut controller = DragDrop::default();
        controller.enable_drops(&mut Vec::new(), &[MIME]).unwrap();
        for _ in 0..16 {
            assert!(handle(&mut controller, &format!("t={kind}:o=1:m=1"), &chunk).is_empty());
        }
        assert!(matches!(
            controller.handle_event(&raw("m=0", b"a")),
            Err(Error::LimitExceeded(_))
        ));
    }

    let mut controller = dropping();
    let chunk = b"YWFh".repeat(1024);
    for _ in 0..20 {
        let events = handle(&mut controller, "t=r:x=1:m=1", &chunk);
        assert!(matches!(events.as_slice(), [Event::Data(_)]));
    }
    let events = handle(&mut controller, "m=0", b"");
    assert!(matches!(events.last(), Some(Event::DataEnd)));
}

#[test]
fn incomplete_osc72_is_a_protocol_error() {
    for prefix in [b"\x1b]72;".as_slice(), b"\x9d72;", b"\xc2\x9d72;"] {
        let mut controller = dropping();
        let bytes = [prefix, b"t=r:x=1;YQ"].concat();
        assert!(matches!(
            controller.handle_event(&RawEvent::Unknown(bytes)),
            Err(Error::Protocol(_))
        ));
        controller.cancel_drop(&mut Vec::new()).unwrap();
        assert!(handle(&mut controller, "t=r:x=1", b"").is_empty());
        assert!(
            controller
                .handle_event(&RawEvent::Unknown(b"\x1b]720;other".to_vec()))
                .unwrap()
                .is_empty()
        );
    }
}

#[test]
fn malformed_routed_messages_preserve_other_endpoints() {
    let mut controller = DragDrop::new(DragDropOptions {
        routing_id: Some(17),
        ..DragDropOptions::default()
    });
    controller.enable_drops(&mut Vec::new(), &[MIME]).unwrap();
    handle(&mut controller, "t=M:i=17:o=1", MIME.as_bytes());
    controller.request(&mut Vec::new(), MIME).unwrap();
    for event in [
        RawEvent::UnknownOsc(b"72;t=r:i=99;invalid".to_vec()),
        RawEvent::UnknownOsc(b"72;i=99".to_vec()),
        RawEvent::Unknown(b"\x1b]72;t=r:i=99;partial".to_vec()),
        RawEvent::Unknown(b"\x9d72;i=99:t=r".to_vec()),
        RawEvent::Unknown(b"\xc2\x9d72;i=99;partial".to_vec()),
        RawEvent::UnknownOsc(b"72;t=r;payload:i=99;".to_vec()),
    ] {
        assert!(controller.handle_event(&event).unwrap().is_empty());
        assert!(controller.request.is_some());
    }
    for event in [
        RawEvent::UnknownOsc(b"72;t=r:i=17;invalid".to_vec()),
        RawEvent::Unknown(b"\x1b]72;i=17;partial".to_vec()),
        RawEvent::Unknown(b"\x1b]72;i=99".to_vec()),
        RawEvent::UnknownOsc(b"72;t=r:i=17;payload:i=99;".to_vec()),
    ] {
        assert!(matches!(
            controller.handle_event(&event),
            Err(Error::Protocol(_))
        ));
    }
}

#[test]
fn bare_malformed_drag_commands_are_not_ignored() {
    for event in [
        RawEvent::UnknownOsc(b"72".to_vec()),
        RawEvent::Unknown(b"\x1b]72".to_vec()),
        RawEvent::Unknown(b"\x9d72".to_vec()),
        RawEvent::Unknown(b"\xc2\x9d72".to_vec()),
    ] {
        assert!(matches!(
            dropping().handle_event(&event),
            Err(Error::Protocol(_))
        ));
    }
    for event in [
        RawEvent::UnknownOsc(b"720".to_vec()),
        RawEvent::Unknown(b"\x1b]720".to_vec()),
        RawEvent::Unknown(b"\x9d720;other".to_vec()),
        RawEvent::Unknown(b"\xc2\x9d720;other".to_vec()),
    ] {
        assert!(dropping().handle_event(&event).unwrap().is_empty());
    }
}

#[test]
fn numeric_osc72_spellings_report_lost_data() {
    let (reader, mut writer) = std::io::pipe().unwrap();
    let mut source = uncurses::event::EventSource::new(reader).unwrap();
    for command in [b"072".as_slice(), b"00072", b"+72"] {
        let valid = [b"\x1b]", command, b";t=r:x=1;YQ\x1b\\"].concat();
        writer.write_all(&valid).unwrap();
        assert!(matches!(source.read().unwrap(), RawEvent::DragDrop { .. }));
        let oversized = [
            b"\x1b]",
            command,
            b";t=r:x=1;",
            &b"YQ".repeat(2050),
            b"\x1b\\",
        ]
        .concat();
        let event = std::thread::scope(|scope| {
            let sending = scope.spawn(|| writer.write_all(&oversized).unwrap());
            let event = source.read().unwrap();
            sending.join().unwrap();
            event
        });
        assert!(matches!(event, RawEvent::UnknownOsc(_)));
        let mut controller = dropping();
        assert!(matches!(
            controller.handle_event(&event),
            Err(Error::Protocol(_))
        ));
        controller.cancel_drop(&mut Vec::new()).unwrap();
        assert!(handle(&mut controller, "t=r:x=1", b"").is_empty());
        for prefix in [b"\x1b]".as_slice(), b"\x9d", b"\xc2\x9d"] {
            let event = RawEvent::Unknown([prefix, command, b";t=r:x=1;YQ"].concat());
            assert!(matches!(
                dropping().handle_event(&event),
                Err(Error::Protocol(_))
            ));
        }
    }
}

#[test]
fn complete_malformed_headers_use_default_zero_routing() {
    let mut controller = DragDrop::new(DragDropOptions {
        routing_id: Some(17),
        ..DragDropOptions::default()
    });
    controller.enable_drops(&mut Vec::new(), &[MIME]).unwrap();
    handle(&mut controller, "t=M:i=17:o=1", MIME.as_bytes());
    controller.request(&mut Vec::new(), MIME).unwrap();
    for event in [
        RawEvent::UnknownOsc([b"72;t=r:x=1;".as_slice(), &vec![b'a'; 4097]].concat()),
        RawEvent::UnknownOsc(b"72;t=r:x=1".to_vec()),
        RawEvent::Unknown(b"\x1b]72;t=r:x=1;partial".to_vec()),
    ] {
        assert!(controller.handle_event(&event).unwrap().is_empty());
        assert!(controller.request.is_some());
        assert!(matches!(
            dropping().handle_event(&event),
            Err(Error::Protocol(_))
        ));
    }
    for event in [
        RawEvent::Unknown(b"\x1b]72;t=r:x=1".to_vec()),
        RawEvent::UnknownOsc(b"72;broken;payload".to_vec()),
        RawEvent::UnknownOsc(b"72;t=r:i=bad;payload".to_vec()),
        RawEvent::UnknownOsc(b"72;t=r:\0bad=value;payload".to_vec()),
    ] {
        assert!(matches!(
            controller.handle_event(&event),
            Err(Error::Protocol(_))
        ));
    }
}

#[test]
fn orphaned_request_errors_preserve_replacement_drops() {
    for replacement in [false, true] {
        let mut controller = dropping();
        if replacement {
            handle(&mut controller, "t=M:o=1", MIME.as_bytes());
        } else {
            controller.cancel_drop(&mut Vec::new()).unwrap();
        }
        for metadata in ["t=R:x=1", "t=R:y=1", "t=R:Y=37:x=1"] {
            assert!(handle(&mut controller, metadata, b"EIO:late").is_empty());
            assert_eq!(controller.dropped.is_some(), replacement);
        }
        assert!(matches!(
            handle(&mut controller, "t=R:x=0:y=0:Y=0", b"EIO:global").as_slice(),
            [Event::DropFailed(_)]
        ));
        assert!(controller.dropped.is_none());
    }
    assert!(matches!(
        handle(&mut dropping(), "t=R", b"EIO:global").as_slice(),
        [Event::DropFailed(_)]
    ));
}

fn raw(metadata: &str, payload: &[u8]) -> RawEvent {
    RawEvent::DragDrop {
        options: metadata
            .split(':')
            .map(|pair| {
                let (key, value) = pair.split_once('=').unwrap();
                (key.into(), value.into())
            })
            .collect(),
        payload: payload.into(),
    }
}

fn handle(controller: &mut DragDrop, metadata: &str, payload: &[u8]) -> Vec<Event> {
    controller.handle_event(&raw(metadata, payload)).unwrap()
}

fn dropping() -> DragDrop {
    let mut controller = DragDrop::default();
    controller.enable_drops(&mut Vec::new(), &[MIME]).unwrap();
    handle(&mut controller, "t=M:o=1", MIME.as_bytes());
    controller.request(&mut Vec::new(), MIME).unwrap();
    controller
}

fn preparing() -> DragDrop {
    let mut controller = DragDrop::default();
    controller.enable_drags(&mut Vec::new()).unwrap();
    handle(&mut controller, "t=o", b"");
    controller
        .offer(&mut Vec::new(), &[MIME], Operations::COPY)
        .unwrap();
    controller
}

fn dragging() -> DragDrop {
    let mut controller = preparing();
    controller.start_drag(&mut Vec::new()).unwrap();
    assert_eq!(handle(&mut controller, "t=E", b"OK"), [Event::DragStarted]);
    controller
}

fn output_messages(output: &[u8]) -> Vec<RawEvent> {
    let text = std::str::from_utf8(output).unwrap();
    text.split("\x1b\\")
        .filter(|message| !message.is_empty())
        .map(|message| {
            let body = message.strip_prefix("\x1b]72;").unwrap();
            let (options, payload) = body.split_once(';').unwrap_or((body, ""));
            raw(options, payload.as_bytes())
        })
        .collect()
}

#[test]
fn drop_uses_actual_mimes_and_requires_explicit_completion() {
    let mut controller = DragDrop::default();
    let mut out = Vec::new();
    controller.enable_drops(&mut out, &[MIME]).unwrap();
    let events = handle(
        &mut controller,
        "t=m:x=3:y=4:X=5:Y=6:o=3",
        b"text/plain text/html",
    );
    let Event::Hover(offer) = &events[0] else {
        panic!("expected hover");
    };
    assert_eq!(offer.position.column, 3);
    assert_eq!(offer.position.pixel_y, 6);
    assert_eq!(offer.operations, Operations::COPY | Operations::MOVE);
    assert!(matches!(
        controller.request(&mut out, MIME),
        Err(Error::InvalidState(_))
    ));
    controller
        .accept(&mut out, &[MIME], Operation::Copy)
        .unwrap();
    let events = handle(&mut controller, "t=m:x=4:o=1", b"");
    assert!(matches!(&events[0], Event::Hover(offer) if offer.mime_types.len() == 2));
    handle(&mut controller, "t=M:o=1", b"text/html text/plain");
    out.clear();
    controller.request(&mut out, MIME).unwrap();
    assert_eq!(out, b"\x1b]72;t=r:x=2\x1b\\");
    assert!(controller.request(&mut out, MIME).is_err());
    assert!(controller.complete_drop(&mut out, Operation::Copy).is_err());
    assert_eq!(
        handle(&mut controller, "t=r:x=2", b"YWJj"),
        [Event::Data(b"abc".to_vec())]
    );
    assert!(controller.dropped.is_some());
    assert_eq!(handle(&mut controller, "t=r:x=2", b""), [Event::DataEnd]);
    assert!(controller.dropped.is_some());
    out.clear();
    controller.complete_drop(&mut out, Operation::Copy).unwrap();
    assert_eq!(out, b"\x1b]72;t=r:o=1\x1b\\");
    assert!(controller.dropped.is_none());
}

#[test]
fn new_drag_cancels_the_unfinished_drop_before_its_offer() {
    for actual in [false, true] {
        for data_ended in [false, true] {
            let mut controller = dropping();
            handle(&mut controller, "t=r:x=1", b"b2xk");
            if data_ended {
                handle(&mut controller, "t=r:x=1", b"");
            }
            let metadata = if actual { "t=M:o=1" } else { "t=m:o=1" };
            let events = handle(&mut controller, metadata, b"text/html text/plain");
            assert_eq!(events.len(), 2);
            assert_eq!(events[0], Event::DropCanceled);
            assert!(
                matches!(
                    &events[1],
                    Event::Drop(_) if actual
                ) || matches!(&events[1], Event::Hover(_) if !actual)
            );

            assert!(handle(&mut controller, "t=r:x=1", b"b2xk").is_empty());
            assert!(handle(&mut controller, "t=r:x=1", b"").is_empty());
            let mut out = Vec::new();
            if !actual {
                assert!(controller.complete_drop(&mut out, Operation::Copy).is_err());
                controller.cancel_drop(&mut out).unwrap();
                assert!(out.is_empty());
                controller
                    .accept(&mut out, &[MIME], Operation::Copy)
                    .unwrap();
                handle(&mut controller, "t=M:o=1", b"text/html text/plain");
            }
            out.clear();
            controller.request(&mut out, MIME).unwrap();
            assert_eq!(out, b"\x1b]72;t=r:x=2\x1b\\");
            assert_eq!(
                handle(&mut controller, "t=r:x=2", b"bmV3"),
                [Event::Data(b"new".to_vec())]
            );
            assert_eq!(handle(&mut controller, "t=r:x=2", b""), [Event::DataEnd]);
            controller.complete_drop(&mut out, Operation::Copy).unwrap();
            assert!(matches!(
                handle(&mut controller, "t=m:o=1", MIME.as_bytes()).as_slice(),
                [Event::Hover(_)]
            ));
        }
    }
}

#[test]
fn leave_after_an_unfinished_drop_clears_the_exchange() {
    let mut controller = dropping();
    assert_eq!(
        handle(&mut controller, "t=m:x=-1:y=-1", b""),
        [Event::DropCanceled, Event::Leave]
    );
    let mut out = Vec::new();
    assert!(controller.request(&mut out, MIME).is_err());
    assert!(
        controller
            .accept(&mut out, &[MIME], Operation::Copy)
            .is_err()
    );
    controller.cancel_drop(&mut out).unwrap();
    assert!(out.is_empty());
}

#[test]
fn final_chunk_can_return_data_and_end_together() {
    let mut controller = dropping();
    assert!(handle(&mut controller, "t=r:x=1:m=1", b"YQ").is_empty());
    assert_eq!(
        handle(&mut controller, "m=0", b""),
        [Event::Data(b"a".to_vec()), Event::DataEnd]
    );
}

#[test]
fn all_base64_splits_queries_and_padding() {
    for encoded in [b"YWJjZGU=".as_slice(), b"YWJjZGU"] {
        for split in 0..=encoded.len() {
            let mut controller = dropping();
            let mut events = handle(&mut controller, "t=r:x=1:m=1", &encoded[..split]);
            assert!(handle(&mut controller, "t=q", b"").is_empty());
            assert!(handle(&mut controller, "t=Q", b"").is_empty());
            // Metadata in continuation chunks belongs to the first chunk.
            events.extend(handle(&mut controller, "t=E:x=9:m=0", &encoded[split..]));
            if split != encoded.len() {
                events.extend(handle(&mut controller, "t=r:x=1", b""));
            }
            let mut bytes = Vec::new();
            let mut ends = 0;
            for event in events {
                match event {
                    Event::Data(data) => bytes.extend(data),
                    Event::DataEnd => ends += 1,
                    other => panic!("unexpected event {other:?}"),
                }
            }
            assert_eq!(bytes, b"abcde");
            assert_eq!(ends, 1);
        }
    }
}

#[test]
fn routing_filter_precedes_continuation_state() {
    let options = DragDropOptions {
        routing_id: Some(17),
        machine_id: Some(format!("1:{}", "a".repeat(64)).parse().unwrap()),
        ..DragDropOptions::default()
    };
    let mut controller = DragDrop::new(options.clone());
    let mut out = Vec::new();
    controller.enable_drops(&mut out, &[MIME]).unwrap();
    controller.enable_drags(&mut out).unwrap();
    for message in output_messages(&out) {
        let RawEvent::DragDrop { options, .. } = message else {
            unreachable!()
        };
        assert_eq!(value(&options, "i"), Some("17"));
    }
    assert_eq!(controller.options, options);
    handle(&mut controller, "t=M:i=17:o=1", MIME.as_bytes());
    controller.request(&mut Vec::new(), MIME).unwrap();
    assert!(handle(&mut controller, "t=r:x=1:i=17:m=1", b"Y").is_empty());
    assert!(handle(&mut controller, "t=R:i=99", b"EIO").is_empty());
    assert!(handle(&mut controller, "t=r:x=1", b"garbage").is_empty());
    assert_eq!(
        handle(&mut controller, "m=0:i=17", b"Q"),
        [Event::Data(b"a".to_vec())]
    );
    assert_eq!(
        handle(&mut controller, "t=r:x=1:i=17", b""),
        [Event::DataEnd]
    );
}

#[test]
fn control_messages_are_assembled_and_leave_resets_hover() {
    let mut controller = DragDrop::default();
    controller.enable_drops(&mut Vec::new(), &[]).unwrap();
    assert!(handle(&mut controller, "t=m:o=1:m=1", b"text/").is_empty());
    assert!(matches!(
        &handle(&mut controller, "m=0", b"plain")[0],
        Event::Hover(offer) if offer.mime_types == [MIME]
    ));
    assert_eq!(
        handle(&mut controller, "t=m:x=-1:y=-1", b""),
        [Event::Leave]
    );
    assert!(
        controller
            .accept(&mut Vec::new(), &[MIME], Operation::Copy)
            .is_err()
    );
    assert!(controller.handle_event(&raw("t=m:o=1", b"")).is_err());
}

#[test]
fn outgoing_initial_data_requests_and_repeated_drags() {
    let mut controller = preparing();
    let mut out = Vec::new();
    controller.send(&mut out, MIME, b"a").unwrap();
    assert_eq!(out, b"\x1b]72;t=p:x=0;YQ\x1b\\\x1b]72;t=p:x=0\x1b\\");
    controller.start_drag(&mut out).unwrap();
    let before = out.len();
    assert!(controller.send(&mut out, MIME, b"early").is_err());
    assert_eq!(out.len(), before);
    handle(&mut controller, "t=E", b"OK");
    assert_eq!(
        handle(&mut controller, "t=e:x=5:y=0", b""),
        [Event::DataRequested {
            mime_type: MIME.into()
        }]
    );
    out.clear();
    controller.send(&mut out, MIME, b"abc").unwrap();
    assert_eq!(out, b"\x1b]72;t=e:y=0;YWJj\x1b\\\x1b]72;t=e:y=0\x1b\\");
    assert!(controller.send(&mut out, MIME, b"unrequested").is_err());
    assert_eq!(
        handle(&mut controller, "t=e:x=4:y=1", b""),
        [Event::DragFinished { canceled: true }]
    );
    handle(&mut controller, "t=o", b"");
    controller
        .offer(&mut out, &[MIME], Operations::COPY)
        .unwrap();
}

struct ShortReader<'a> {
    bytes: &'a [u8],
    maximum: usize,
    interrupted: bool,
}

impl Read for ShortReader<'_> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if !self.interrupted {
            self.interrupted = true;
            return Err(io::ErrorKind::Interrupted.into());
        }
        let count = out.len().min(self.maximum).min(self.bytes.len());
        out[..count].copy_from_slice(&self.bytes[..count]);
        self.bytes = &self.bytes[count..];
        Ok(count)
    }
}

#[test]
fn send_from_streams_short_reads_and_obeys_encoded_limit() {
    for length in [0, 1, 2, 3, 3071, 3072, 3073, 10_000] {
        let data: Vec<_> = (0..length).map(|i| (i % 256) as u8).collect();
        let mut reader = ShortReader {
            bytes: &data,
            maximum: 7,
            interrupted: false,
        };
        let mut controller = preparing();
        let mut output = Vec::new();
        controller
            .send_from(&mut output, MIME, &mut reader)
            .unwrap();
        let messages = output_messages(&output);
        let mut decoded = Vec::new();
        for (index, message) in messages.iter().enumerate() {
            let RawEvent::DragDrop { options, payload } = message else {
                unreachable!()
            };
            assert_eq!(value(options, "t"), Some("p"));
            assert!(payload.len() <= 4096);
            assert_eq!(payload.is_empty(), index == messages.len() - 1);
            decoded.extend(BASE64.decode(payload).unwrap());
        }
        assert_eq!(decoded, data);
    }
}

#[test]
fn incoming_stream_retains_only_base64_remainder() {
    let mut controller = dropping();
    let payload = vec![b'Y'; 4096];
    let mut count = 0;
    for _ in 0..128 {
        for event in handle(&mut controller, "t=r:x=1:m=1", &payload) {
            if let Event::Data(bytes) = event {
                count += bytes.len();
            }
        }
        assert!(controller.continuation.as_ref().unwrap().payload.len() <= 4);
    }
    let last = handle(&mut controller, "m=0", b"");
    assert!(matches!(last.last(), Some(Event::DataEnd)));
    if let Event::Data(bytes) = &last[0] {
        count += bytes.len();
    }
    assert_eq!(count, 128 * 3072);
}

#[test]
fn both_roles_operate_without_overwriting_each_other() {
    let mut controller = dropping();
    controller.enable_drags(&mut Vec::new()).unwrap();
    handle(&mut controller, "t=o", b"");
    controller
        .offer(&mut Vec::new(), &[MIME], Operations::COPY)
        .unwrap();
    controller.send(&mut Vec::new(), MIME, b"outgoing").unwrap();
    assert_eq!(
        handle(&mut controller, "t=r:x=1", b"aW5jb21pbmc"),
        [Event::Data(b"incoming".to_vec())]
    );
    controller.start_drag(&mut Vec::new()).unwrap();
    handle(&mut controller, "t=E", b"OK");
    handle(&mut controller, "t=e:x=5", b"");
    handle(&mut controller, "t=r:x=1", b"");
    controller
        .complete_drop(&mut Vec::new(), Operation::Copy)
        .unwrap();
    controller.send(&mut Vec::new(), MIME, b"outgoing").unwrap();
}

#[test]
fn invalid_operations_and_arguments_do_not_write() {
    let mut controller = DragDrop::default();
    let mut out = Vec::new();
    assert!(
        controller
            .enable_drops(&mut out, &["text/plain\x1b"])
            .is_err()
    );
    assert!(controller.enable_drops(&mut out, &[MIME, MIME]).is_err());
    assert!(out.is_empty());
    controller.enable_drops(&mut out, &[MIME]).unwrap();
    handle(&mut controller, "t=m:o=2", MIME.as_bytes());
    out.clear();
    assert!(
        controller
            .accept(&mut out, &[MIME], Operation::Copy)
            .is_err()
    );
    assert!(
        controller
            .accept(&mut out, &["image/png"], Operation::Move)
            .is_err()
    );
    assert!(out.is_empty());
    controller.reject(&mut out).unwrap();
    assert_eq!(out, b"\x1b]72;t=m:o=0\x1b\\");
}

#[test]
fn cancellation_drains_continuations_and_preserves_registration() {
    let mut controller = dropping();
    handle(&mut controller, "t=r:x=1:m=1", b"Y");
    let mut out = Vec::new();
    controller.cancel_drop(&mut out).unwrap();
    assert!(handle(&mut controller, "m=0", b"Q").is_empty());
    handle(&mut controller, "t=M:o=1", MIME.as_bytes());
    controller.request(&mut out, MIME).unwrap();
    assert_eq!(handle(&mut controller, "t=r:x=1", b""), [Event::DataEnd]);
    controller.close(&mut out).unwrap();
    assert!(out.ends_with(b"\x1b]72;t=A\x1b\\"));
    let length = out.len();
    controller.close(&mut out).unwrap();
    assert_eq!(out.len(), length);
}

#[test]
fn cancel_preparation_and_pending_start_use_distinct_cleanup() {
    for starting in [false, true] {
        let mut controller = preparing();
        if starting {
            controller.start_drag(&mut Vec::new()).unwrap();
        }
        let mut out = Vec::new();
        controller.cancel_drag(&mut out).unwrap();
        if starting {
            assert_eq!(out, b"\x1b]72;t=E:y=-1\x1b\\");
        } else {
            assert_eq!(out, b"\x1b]72;t=o:x=2\x1b\\\x1b]72;t=o:x=1\x1b\\");
        }
        assert!(handle(&mut controller, "t=E", b"OK").is_empty());
        assert!(matches!(
            handle(&mut controller, "t=o", b"")[0],
            Event::DragRequested(_)
        ));
    }
    let mut controller = dragging();
    let mut out = Vec::new();
    controller.cancel_drag(&mut out).unwrap();
    assert_eq!(out, b"\x1b]72;t=E:y=-1\x1b\\");
}

#[test]
fn remote_errors_and_invalid_file_metadata_are_explicit() {
    let mut controller = dropping();
    assert_eq!(
        handle(&mut controller, "t=R:x=1", b"EIO:failed"),
        [Event::DropFailed(RemoteError {
            code: "EIO".into(),
            description: Some("failed".into())
        })]
    );
    assert!(controller.dropped.is_none());
    let mut controller = dropping();
    assert!(matches!(
        controller.handle_event(&raw("t=r:x=1:X=1", b"")),
        Err(Error::Protocol(_))
    ));
    let mut controller = dragging();
    assert!(matches!(
        controller.handle_event(&raw("t=k:x=1", b"")),
        Err(Error::Protocol(_))
    ));
    controller.cancel_drag(&mut Vec::new()).unwrap();
}

#[test]
fn malformed_data_and_out_of_range_indices_fail() {
    for (metadata, payload) in [
        ("t=r:x=2", b"YWJj".as_slice()),
        ("t=r:x=1", b"!"),
        ("t=r:x=1", b"Y"),
        ("t=r:x=1:m=2", b""),
        ("t=r:x=4294967296", b""),
    ] {
        let mut controller = dropping();
        assert!(matches!(
            controller.handle_event(&raw(metadata, payload)),
            Err(Error::Protocol(_))
        ));
    }
    let mut controller = dragging();
    assert!(controller.handle_event(&raw("t=e:x=5:y=99", b"")).is_err());
    assert!(
        controller
            .handle_event(&RawEvent::UnknownOsc(b"72;t=r".to_vec()))
            .is_err()
    );
}

#[test]
fn source_failure_does_not_finish_or_allow_a_retry() {
    struct FailsAfterChunk(bool);
    impl Read for FailsAfterChunk {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            if self.0 {
                Err(io::Error::other("source failed"))
            } else {
                self.0 = true;
                buffer.fill(b'x');
                Ok(buffer.len())
            }
        }
    }
    let mut controller = dragging();
    handle(&mut controller, "t=e:x=5", b"");
    let mut out = Vec::new();
    assert!(matches!(
        controller.send_from(&mut out, MIME, &mut FailsAfterChunk(false)),
        Err(Error::Io(_))
    ));
    assert!(!out.ends_with(b"\x1b]72;t=e:y=0\x1b\\"));
    let length = out.len();
    assert!(controller.send(&mut out, MIME, b"retry").is_err());
    assert_eq!(out.len(), length);
    controller
        .fail_data(
            &mut out,
            MIME,
            RemoteError {
                code: "EIO".into(),
                description: None,
            },
        )
        .unwrap();
    assert!(out.ends_with(b"\x1b]72;t=E:y=0;EIO\x1b\\"));
}

#[test]
fn partial_output_failure_poisoning_and_no_implicit_flush() {
    struct FailWriter(usize);
    impl Write for FailWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if self.0 == 0 {
                return Err(io::Error::other("broken output"));
            }
            let count = self.0.min(bytes.len());
            self.0 -= count;
            Ok(count)
        }
        fn flush(&mut self) -> io::Result<()> {
            panic!("controller must not flush");
        }
    }
    let mut controller = preparing();
    assert!(matches!(
        controller.send(&mut FailWriter(5), MIME, b"abc"),
        Err(Error::Io(_))
    ));
    let mut out = Vec::new();
    assert!(matches!(
        controller.cancel_drag(&mut out),
        Err(Error::OutputFailed)
    ));
    assert!(matches!(controller.next_send(), Err(Error::OutputFailed)));
    assert!(out.is_empty());
    let mut controller = preparing();
    controller.send(&mut FailWriter(100), MIME, b"abc").unwrap();
}

#[test]
fn long_mime_lists_are_split_and_machine_ids_are_validated() {
    let mime = format!("application/{}", "a".repeat(9000));
    let mut controller = DragDrop::default();
    let mut out = Vec::new();
    controller.enable_drops(&mut out, &[&mime]).unwrap();
    let messages = output_messages(&out);
    assert_eq!(messages.len(), 3);
    for (index, message) in messages.iter().enumerate() {
        let RawEvent::DragDrop { options, payload } = message else {
            unreachable!()
        };
        assert!(payload.len() <= 4096);
        assert_eq!(
            value(options, "m"),
            if index < 2 { Some("1") } else { None }
        );
    }
    assert!("a raw UUID".parse::<MachineId>().is_err());
    assert!(
        format!("1:{}", "g".repeat(64))
            .parse::<MachineId>()
            .is_err()
    );
}

#[test]
fn native_mime_lists_allow_trailing_whitespace() {
    let mut controller = DragDrop::default();
    controller.enable_drops(&mut Vec::new(), &[MIME]).unwrap();
    for kind in ["t=m:o=1", "t=M:o=1"] {
        let events = handle(&mut controller, kind, b"text/html text/plain ");
        match &events[0] {
            Event::Hover(offer) | Event::Drop(offer) => {
                assert_eq!(offer.mime_types, ["text/html", MIME]);
            }
            other => panic!("unexpected event {other:?}"),
        }
    }
    let mut out = Vec::new();
    controller.request(&mut out, MIME).unwrap();
    assert_eq!(out, b"\x1b]72;t=r:x=2\x1b\\");
}

#[test]
fn per_mime_failure_preserves_other_requests_and_cancellation() {
    let mut controller = DragDrop::default();
    let mut out = Vec::new();
    controller.enable_drags(&mut out).unwrap();
    handle(&mut controller, "t=o", b"");
    controller
        .offer(&mut out, &[MIME, "text/html"], Operations::COPY)
        .unwrap();
    controller.start_drag(&mut out).unwrap();
    handle(&mut controller, "t=E", b"OK");
    handle(&mut controller, "t=e:x=5:y=0", b"");
    handle(&mut controller, "t=e:x=5:y=1", b"");
    out.clear();
    controller
        .fail_data(
            &mut out,
            MIME,
            RemoteError {
                code: "EIO".into(),
                description: None,
            },
        )
        .unwrap();
    controller
        .send(&mut out, "text/html", b"<p>text</p>")
        .unwrap();
    controller.cancel_drag(&mut out).unwrap();
    assert!(out.starts_with(b"\x1b]72;t=E:y=0;EIO\x1b\\"));
    assert!(out.ends_with(b"\x1b]72;t=E:y=-1\x1b\\"));
}

#[test]
fn close_after_start_request_cancels_before_unregistering() {
    let mut controller = preparing();
    controller.start_drag(&mut Vec::new()).unwrap();
    let mut out = Vec::new();
    controller.close(&mut out).unwrap();
    assert_eq!(out, b"\x1b]72;t=E:y=-1\x1b\\\x1b]72;t=o:x=2\x1b\\");
    assert!(handle(&mut controller, "t=E", b"OK").is_empty());
}

#[test]
fn positions_preserve_negative_pixel_offsets() {
    for (kind, pixel_x, pixel_y) in [("m", 0, -1), ("M", -2, -1), ("o", 0, -1)] {
        let mut controller = DragDrop::default();
        let mut out = Vec::new();
        controller.enable_drops(&mut out, &[MIME]).unwrap();
        controller.enable_drags(&mut out).unwrap();
        let events = handle(
            &mut controller,
            &format!("t={kind}:x=3:y=4:X={pixel_x}:Y={pixel_y}:o=1"),
            if kind == "o" { b"" } else { MIME.as_bytes() },
        );
        let position = match &events[0] {
            Event::Hover(offer) | Event::Drop(offer) => offer.position,
            Event::DragRequested(position) => *position,
            other => panic!("unexpected event {other:?}"),
        };
        assert_eq!(position.column, 3);
        assert_eq!(position.row, 4);
        assert_eq!(i64::from(position.pixel_x), i64::from(pixel_x));
        assert_eq!(i64::from(position.pixel_y), i64::from(pixel_y));
    }
}

#[test]
fn native_format_aliases_preserve_request_indices() {
    let mut controller = DragDrop::default();
    let mut out = Vec::new();
    controller.enable_drops(&mut out, &[MIME]).unwrap();
    let events = handle(&mut controller, "t=m:o=1", b"UTF8_STRING text/plain");
    assert!(matches!(
        &events[0], Event::Hover(offer) if offer.mime_types == ["UTF8_STRING", MIME]
    ));
    controller
        .accept(&mut out, &["UTF8_STRING"], Operation::Copy)
        .unwrap();
    handle(&mut controller, "t=M:o=1", b"UTF8_STRING text/plain");
    out.clear();
    controller.request(&mut out, MIME).unwrap();
    assert_eq!(out, b"\x1b]72;t=r:x=2\x1b\\");
}

#[test]
fn reregistration_does_not_revive_a_disabled_drop_chunk_chain() {
    for initially_enabled in [false, true] {
        let mut controller = DragDrop::default();
        let mut out = Vec::new();
        if initially_enabled {
            controller.enable_drops(&mut out, &[MIME]).unwrap();
        }
        assert!(handle(&mut controller, "t=M:o=1:m=1", b"text/").is_empty());
        controller.disable_drops(&mut out).unwrap();
        controller.enable_drops(&mut out, &[MIME]).unwrap();
        assert!(handle(&mut controller, "m=0", b"plain").is_empty());
        assert!(controller.dropped.is_none());
        assert!(matches!(
            handle(&mut controller, "t=m:o=1", MIME.as_bytes())[0],
            Event::Hover(_)
        ));
    }
}

#[test]
fn disabling_drops_preserves_the_outgoing_message_chain() {
    let mut controller = preparing();
    let mut out = Vec::new();
    controller.enable_drops(&mut out, &[MIME]).unwrap();
    controller.start_drag(&mut out).unwrap();
    assert!(handle(&mut controller, "t=E:m=1", b"O").is_empty());
    controller.disable_drops(&mut out).unwrap();
    assert_eq!(handle(&mut controller, "m=0", b"K"), [Event::DragStarted]);
}
