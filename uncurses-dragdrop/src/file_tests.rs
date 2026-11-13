use super::*;

fn file_drop() -> DragDrop {
    let mut controller = DragDrop::default();
    controller.enable_drops(&mut Vec::new(), &[]).unwrap();
    handle(&mut controller, "t=M:o=1", b"text/plain text/uri-list");
    controller
}

fn listed_drop() -> DragDrop {
    let mut controller = file_drop();
    controller.list(&mut Vec::new()).unwrap();
    handle(&mut controller, "t=r:x=2:X=1", b"ZmlsZTovLy9h");
    handle(&mut controller, "t=r:x=2:X=1", b"");
    controller
}

fn file_drag() -> DragDrop {
    let mut controller = DragDrop::default();
    controller.enable_drags(&mut Vec::new()).unwrap();
    handle(&mut controller, "t=o", b"");
    controller
        .offer_files(
            &mut Vec::new(),
            &["file:///root/", "file:///last"],
            Operations::COPY,
        )
        .unwrap();
    controller.start_drag(&mut Vec::new()).unwrap();
    handle(&mut controller, "t=E", b"OK");
    controller
}

fn decoded(output: &[u8]) -> Vec<u8> {
    output_messages(output)
        .iter()
        .flat_map(|message| {
            let RawEvent::DragDrop { payload, .. } = message else {
                unreachable!()
            };
            BASE64.decode(payload).unwrap()
        })
        .collect()
}

#[test]
fn replacement_drop_releases_directory_state_and_preserves_outgoing_drag() {
    let mut controller = listed_drop();
    let mut out = Vec::new();
    controller.request(&mut out, Entry::UriEntry(1)).unwrap();
    handle(&mut controller, "t=r:x=2:y=1:X=42", b"Y2hpbGQ=");
    handle(&mut controller, "t=r:x=2:y=1:X=42", b"");
    controller
        .request(
            &mut out,
            Entry::DirectoryEntry {
                handle: 42,
                index: 1,
            },
        )
        .unwrap();
    controller.enable_drags(&mut out).unwrap();
    handle(&mut controller, "t=o", b"");
    controller
        .offer(&mut out, &[MIME], Operations::COPY)
        .unwrap();
    controller.start_drag(&mut out).unwrap();
    handle(&mut controller, "t=E", b"OK");

    assert!(matches!(
        handle(&mut controller, "t=M:o=1", b"text/uri-list").as_slice(),
        [Event::DropCanceled, Event::Drop(_)]
    ));
    out.clear();
    assert!(controller.close_directory(&mut out, 42).is_err());
    assert!(controller.request(&mut out, Entry::UriEntry(1)).is_err());
    assert!(
        controller
            .request(
                &mut out,
                Entry::DirectoryEntry {
                    handle: 42,
                    index: 1
                }
            )
            .is_err()
    );
    assert!(out.is_empty());
    controller.list(&mut out).unwrap();
    assert_eq!(out, b"\x1b]72;t=r:x=1\x1b\\");
    assert_eq!(
        handle(&mut controller, "t=e:x=5:y=0", b""),
        [Event::DataRequested {
            mime_type: MIME.into()
        }]
    );
    controller.send(&mut out, MIME, b"still active").unwrap();
}

#[test]
fn entry_conversions_preserve_mime_requests_and_empty_acceptance() {
    let mut controller = DragDrop::default();
    let mut out = Vec::new();
    controller.enable_drops(&mut out, &[]).unwrap();
    handle(&mut controller, "t=m:o=1", b"text/plain UTF8_STRING");
    out.clear();
    controller.accept(&mut out, &[], Operation::Copy).unwrap();
    assert_eq!(out, b"\x1b]72;t=m:o=1\x1b\\");
    handle(&mut controller, "t=M:o=1", b"text/plain UTF8_STRING");
    for entry in [
        Entry::Text,
        Entry::from("text/plain"),
        Entry::from(String::from("text/plain")),
        Entry::from(&String::from("text/plain")),
        Entry::from(&Entry::Text),
    ] {
        out.clear();
        controller.request(&mut out, entry).unwrap();
        assert_eq!(out, b"\x1b]72;t=r:x=1\x1b\\");
        handle(&mut controller, "t=r:x=1", b"");
    }
    out.clear();
    controller
        .request(&mut out, Entry::Custom("UTF8_STRING".into()))
        .unwrap();
    assert_eq!(out, b"\x1b]72;t=r:x=2\x1b\\");
}

#[test]
fn uri_lists_stream_original_names_and_native_indices_at_every_split() {
    let data = b"#comment\r\n\r\n \t\rfile:///one \t\r\nfile:///two\n#last\nfile:///three";
    for split in 1..=data.len() {
        let mut controller = file_drop();
        let mut out = Vec::new();
        controller.list(&mut out).unwrap();
        assert_eq!(out, b"\x1b]72;t=r:x=2\x1b\\");
        let mut events = handle(
            &mut controller,
            "t=r:x=2:X=1",
            BASE64.encode(&data[..split]).as_bytes(),
        );
        if split != data.len() {
            events.extend(handle(
                &mut controller,
                "t=r:x=2:X=1",
                BASE64.encode(&data[split..]).as_bytes(),
            ));
        }
        events.extend(handle(&mut controller, "t=r:x=2:X=1", b""));
        assert_eq!(
            events,
            vec![
                Event::UriList { remote: true },
                Event::Listed {
                    entry: Entry::UriEntry(1),
                    name: b"file:///one \t".to_vec()
                },
                Event::Listed {
                    entry: Entry::UriEntry(2),
                    name: b"file:///two".to_vec()
                },
                Event::Listed {
                    entry: Entry::UriEntry(3),
                    name: b"file:///three".to_vec()
                },
                Event::DataEnd,
            ]
        );
        assert!(controller.request(&mut out, Entry::UriEntry(0)).is_err());
        assert!(controller.request(&mut out, Entry::UriEntry(4)).is_err());
        out.clear();
        controller.request(&mut out, Entry::UriEntry(3)).unwrap();
        assert_eq!(out, b"\x1b]72;t=r:x=2:y=3\x1b\\");
    }
}

#[test]
fn raw_uri_request_retains_bytes_and_announces_metadata_before_chunked_data() {
    let mut controller = file_drop();
    let data = b"#comment\r\nfile:///one\n";
    controller
        .request(&mut Vec::new(), &Entry::UriList)
        .unwrap();
    let encoded = BASE64.encode(data);
    assert_eq!(
        handle(&mut controller, "t=r:x=2:X=1:m=1", &encoded.as_bytes()[..1]),
        [Event::UriList { remote: true }]
    );
    let events = handle(
        &mut controller,
        "t=r:x=99:X=0:m=0",
        &encoded.as_bytes()[1..],
    );
    assert_eq!(events, [Event::Data(data.to_vec())]);
    assert_eq!(
        handle(&mut controller, "t=r:x=2:X=1", b""),
        [Event::DataEnd]
    );
    controller
        .request(&mut Vec::new(), Entry::UriEntry(1))
        .unwrap();
}

#[test]
fn list_flushes_the_last_uri_with_base64_remainder_and_reports_local_empty_lists() {
    let mut controller = file_drop();
    controller.list(&mut Vec::new()).unwrap();
    assert_eq!(
        handle(&mut controller, "t=r:x=2:m=1", b"YQ"),
        [Event::UriList { remote: false }]
    );
    assert_eq!(
        handle(&mut controller, "m=0", b""),
        [
            Event::Listed {
                entry: Entry::UriEntry(1),
                name: b"a".to_vec()
            },
            Event::DataEnd
        ]
    );
    controller
        .request(&mut Vec::new(), Entry::UriEntry(1))
        .unwrap();
    assert_eq!(
        handle(&mut controller, "t=r:x=2:y=1", b""),
        [Event::Entry(EntryKind::File), Event::DataEnd]
    );
    let mut controller = file_drop();
    controller.list(&mut Vec::new()).unwrap();
    assert_eq!(
        handle(&mut controller, "t=r:x=2", b""),
        [Event::UriList { remote: false }, Event::DataEnd]
    );
    assert!(
        controller
            .request(&mut Vec::new(), Entry::UriEntry(1))
            .is_err()
    );
}

#[test]
fn invalid_chunk_data_drains_before_a_new_drop_can_register() {
    let mut controller = file_drop();
    controller.list(&mut Vec::new()).unwrap();
    assert!(
        controller
            .handle_event(&raw("t=r:x=2:m=1", b"!!!!!"))
            .is_err()
    );
    controller.disable_drops(&mut Vec::new()).unwrap();
    controller.enable_drops(&mut Vec::new(), &[]).unwrap();
    assert!(handle(&mut controller, "t=M:o=1:m=0", b"text/uri-list").is_empty());
    assert!(controller.dropped.is_none());
    handle(&mut controller, "t=M:o=1", b"text/uri-list");
    controller.list(&mut Vec::new()).unwrap();
    assert_eq!(
        handle(&mut controller, "t=r:x=1", b""),
        [Event::UriList { remote: false }, Event::DataEnd]
    );
}

#[test]
fn incoming_directories_preserve_binary_names_signed_handles_and_nested_targets() {
    for handle_id in [i64::from(i32::MIN), -1, 2, i64::from(u32::MAX)] {
        let mut controller = listed_drop();
        let mut out = Vec::new();
        controller.request(&mut out, Entry::UriEntry(1)).unwrap();
        let header = format!("t=r:x=2:y=1:X={handle_id}");
        let mut events = Vec::new();
        for byte in b"a\0\xffb\0child" {
            events.extend(handle(
                &mut controller,
                &header,
                BASE64.encode([*byte]).as_bytes(),
            ));
        }
        events.extend(handle(&mut controller, &header, b""));
        assert_eq!(
            events,
            [
                Event::Entry(EntryKind::Directory { handle: handle_id }),
                Event::Listed {
                    entry: Entry::DirectoryEntry {
                        handle: handle_id,
                        index: 1
                    },
                    name: b"a".to_vec()
                },
                Event::Listed {
                    entry: Entry::DirectoryEntry {
                        handle: handle_id,
                        index: 2
                    },
                    name: b"\xffb".to_vec()
                },
                Event::Listed {
                    entry: Entry::DirectoryEntry {
                        handle: handle_id,
                        index: 3
                    },
                    name: b"child".to_vec()
                },
                Event::DataEnd,
            ]
        );
        assert!(
            controller
                .request(
                    &mut out,
                    Entry::DirectoryEntry {
                        handle: handle_id,
                        index: 4
                    }
                )
                .is_err()
        );
        out.clear();
        controller
            .request(
                &mut out,
                Entry::DirectoryEntry {
                    handle: handle_id,
                    index: 2,
                },
            )
            .unwrap();
        assert_eq!(
            out,
            format!("\x1b]72;t=r:x=2:Y={handle_id}\x1b\\").as_bytes()
        );
        assert!(controller.close_directory(&mut out, handle_id).is_err());
        let header = format!("t=r:x=2:Y={handle_id}:X=1");
        assert_eq!(
            handle(&mut controller, &header, b"Li4vYQ"),
            [
                Event::Entry(EntryKind::Symlink),
                Event::Data(b"../a".to_vec())
            ]
        );
        assert_eq!(handle(&mut controller, &header, b""), [Event::DataEnd]);
        controller
            .request(
                &mut out,
                Entry::DirectoryEntry {
                    handle: handle_id,
                    index: 3,
                },
            )
            .unwrap();
        let child_header = format!("t=r:x=3:Y={handle_id}:X=42");
        assert_eq!(
            handle(&mut controller, &child_header, b""),
            [
                Event::Entry(EntryKind::Directory { handle: 42 }),
                Event::DataEnd
            ]
        );
        out.clear();
        controller.close_directory(&mut out, handle_id).unwrap();
        assert_eq!(out, format!("\x1b]72;t=r:Y={handle_id}\x1b\\").as_bytes());
        assert!(controller.close_directory(&mut out, handle_id).is_err());
        assert!(
            controller
                .request(
                    &mut out,
                    Entry::DirectoryEntry {
                        handle: handle_id,
                        index: 1
                    }
                )
                .is_err()
        );
        controller.close_directory(&mut out, 42).unwrap();
    }
}

#[test]
fn file_response_matching_kind_and_error_validation_clear_traversal() {
    for header in [
        "t=r:x=1:y=1",
        "t=r:x=2:y=2",
        "t=r:x=2:y=1:Y=3",
        "t=r:x=2:y=1:X=4294967296",
        "t=R:x=2:y=2",
    ] {
        let mut controller = listed_drop();
        controller
            .request(&mut Vec::new(), Entry::UriEntry(1))
            .unwrap();
        let payload = if header.starts_with("t=R") {
            b"EIO".as_slice()
        } else {
            b""
        };
        assert!(matches!(
            controller.handle_event(&raw(header, payload)),
            Err(Error::Protocol(_))
        ));
        assert!(controller.request.is_none());
        assert!(
            controller
                .request(&mut Vec::new(), Entry::UriEntry(1))
                .is_err()
        );
    }
    let mut controller = listed_drop();
    controller
        .request(&mut Vec::new(), Entry::UriEntry(1))
        .unwrap();
    assert_eq!(
        handle(&mut controller, "t=r:x=2:y=1", b"YQ"),
        [Event::Entry(EntryKind::File), Event::Data(b"a".to_vec())]
    );
    assert!(
        controller
            .handle_event(&raw("t=r:x=2:y=1:X=1", b""))
            .is_err()
    );
    controller.cancel_drop(&mut Vec::new()).unwrap();
    assert!(controller.dropped.is_none());
}

#[test]
fn incoming_file_error_and_cancellation_discard_pending_lists() {
    let mut controller = listed_drop();
    controller
        .request(&mut Vec::new(), Entry::UriEntry(1))
        .unwrap();
    assert_eq!(
        handle(&mut controller, "t=R:x=2:y=1", b"ENOENT"),
        [Event::DropFailed(RemoteError {
            code: "ENOENT".into(),
            description: None
        })]
    );
    assert!(controller.dropped.is_none());
    let mut controller = file_drop();
    controller.list(&mut Vec::new()).unwrap();
    handle(&mut controller, "t=r:x=2:X=1:m=1", b"Z");
    controller.disable_drops(&mut Vec::new()).unwrap();
    controller.enable_drops(&mut Vec::new(), &[]).unwrap();
    assert!(handle(&mut controller, "t=M:o=1:m=0", b"mlsZTovLy9h").is_empty());
    assert!(
        controller
            .request(&mut Vec::new(), Entry::UriEntry(1))
            .is_err()
    );
}

#[test]
fn outgoing_uri_list_must_be_initial_and_keeps_native_entry_indices() {
    let mut controller = DragDrop::default();
    let mut out = Vec::new();
    controller.enable_drags(&mut out).unwrap();
    handle(&mut controller, "t=o", b"");
    controller
        .offer(&mut out, &["text/plain", "text/uri-list"], Operations::COPY)
        .unwrap();
    out.clear();
    assert!(controller.start_drag(&mut out).is_err());
    assert!(out.is_empty());
    let mime = String::from("text/uri-list");
    controller
        .send(&mut out, &mime, b"#ignore\r\nfile:///one\r \t\nfile:///two")
        .unwrap();
    assert!(
        controller
            .send(&mut out, &mime, b"file:///replacement")
            .is_err()
    );
    controller.start_drag(&mut out).unwrap();
    handle(&mut controller, "t=E", b"OK");
    assert!(controller.handle_event(&raw("t=k:x=3", b"")).is_err());
    assert_eq!(
        handle(&mut controller, "t=k:x=2", b""),
        [Event::EntryRequested {
            entry: Entry::UriEntry(2)
        }]
    );
    out.clear();
    let target = Entry::UriEntry(2);
    controller.send(&mut out, &target, b"abc").unwrap();
    assert_eq!(out, b"\x1b]72;t=k:x=2;YWJj\x1b\\\x1b]72;t=k:x=2\x1b\\");
    assert!(controller.send(&mut out, &target, b"duplicate").is_err());
}

#[test]
fn offer_files_preserves_urls_and_rejects_ambiguous_entries_before_output() {
    let mut controller = DragDrop::default();
    let mut out = Vec::new();
    controller.enable_drags(&mut out).unwrap();
    handle(&mut controller, "t=o", b"");
    out.clear();
    for invalid in [
        vec![],
        vec![""],
        vec!["#comment"],
        vec!["file:///a\nfile:///b"],
        vec!["a\0b"],
    ] {
        assert!(
            controller
                .offer_files(&mut out, &invalid, Operations::COPY)
                .is_err()
        );
        assert!(out.is_empty());
    }
    controller
        .offer_files(
            &mut out,
            &["file:///dir/", "https://example.test/a%20b"],
            Operations::COPY,
        )
        .unwrap();
    let messages = output_messages(&out);
    assert_eq!(messages[0], raw("t=o:o=1", b"text/uri-list"));
    let RawEvent::DragDrop { payload, .. } = &messages[1] else {
        unreachable!()
    };
    assert_eq!(
        BASE64.decode(payload).unwrap(),
        b"file:///dir/\r\nhttps://example.test/a%20b"
    );
}

#[test]
fn outgoing_trees_keep_fifo_roots_and_breadth_first_descendants() {
    let mut controller = file_drag();
    handle(&mut controller, "t=k:x=1", b"");
    handle(&mut controller, "t=k:x=2", b"");
    let mut out = Vec::new();
    assert!(
        controller
            .send(&mut out, Entry::UriEntry(2), b"early")
            .is_err()
    );
    assert!(out.is_empty());
    let root = controller
        .send_directory(&mut out, Entry::UriEntry(1), &[b"sub", b"\xfffile"])
        .unwrap();
    assert_eq!(decoded(&out), b"sub\0\xfffile");
    out.clear();
    let child = controller
        .send_directory(
            &mut out,
            Entry::DirectoryEntry {
                handle: root,
                index: 1,
            },
            &[b"link"],
        )
        .unwrap();
    let grandchild = Entry::DirectoryEntry {
        handle: child,
        index: 1,
    };
    assert!(controller.send(&mut out, &grandchild, b"too soon").is_err());
    assert!(
        controller
            .send(&mut out, Entry::UriEntry(2), b"too soon")
            .is_err()
    );
    let sibling = Entry::DirectoryEntry {
        handle: root,
        index: 2,
    };
    controller.send(&mut out, sibling, b"file").unwrap();
    controller
        .send_symlink(&mut out, grandchild, b"../\xfftarget")
        .unwrap();
    controller
        .send(&mut out, Entry::UriEntry(2), b"last")
        .unwrap();
    let frames = output_messages(&out);
    assert_eq!(
        frames[0],
        raw(&format!("t=k:x=1:Y={root}:y=1:X={child}"), b"bGluaw")
    );
    assert_eq!(frames[2], raw(&format!("t=k:x=1:Y={root}:y=2"), b"ZmlsZQ"));
    assert_eq!(
        frames[4],
        raw(
            &format!("t=k:x=1:Y={child}:y=1:X=1"),
            BASE64.encode(b"../\xfftarget").as_bytes()
        )
    );
    assert_eq!(frames[6], raw("t=k:x=2", b"bGFzdA"));
    assert!(
        controller
            .send(&mut out, Entry::UriEntry(2), b"duplicate")
            .is_err()
    );
    assert!(controller.drag.file_requests.is_empty());
    assert!(controller.drag.directories.is_empty());
}

#[test]
fn empty_outgoing_directory_and_repeated_roots_finish_independently() {
    let mut controller = file_drag();
    handle(&mut controller, "t=k:x=1", b"");
    handle(&mut controller, "t=k:x=1", b"");
    let mut out = Vec::new();
    for bad in [vec![b"".as_slice()], vec![b"a\0b".as_slice()]] {
        assert!(
            controller
                .send_directory(&mut out, Entry::UriEntry(1), &bad)
                .is_err()
        );
        assert!(out.is_empty());
    }
    let handle = controller
        .send_directory(&mut out, Entry::UriEntry(1), &[])
        .unwrap();
    assert_eq!(
        output_messages(&out),
        [raw(&format!("t=k:x=1:X={handle}"), b"")]
    );
    controller
        .send_symlink(&mut out, Entry::UriEntry(1), b"target")
        .unwrap();
    assert!(controller.drag.file_requests.is_empty());
}

struct FailAfterBlock(bool);

impl Read for FailAfterBlock {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if self.0 {
            return Err(io::Error::other("source failed"));
        }
        self.0 = true;
        out.fill(b'x');
        Ok(out.len())
    }
}

#[test]
fn file_source_failure_requires_global_error_and_discards_the_queue() {
    let mut controller = file_drag();
    handle(&mut controller, "t=k:x=1", b"");
    handle(&mut controller, "t=k:x=2", b"");
    let mut out = Vec::new();
    assert!(matches!(
        controller.send_from(&mut out, Entry::UriEntry(1), &mut FailAfterBlock(false)),
        Err(Error::Io(_))
    ));
    assert!(!out.ends_with(b"\x1b]72;t=k:x=1\x1b\\"));
    assert!(
        controller
            .send(&mut out, Entry::UriEntry(1), b"retry")
            .is_err()
    );
    controller
        .fail_drag(
            &mut out,
            RemoteError {
                code: "EIO".into(),
                description: Some("source failed".into()),
            },
        )
        .unwrap();
    assert!(out.ends_with(b"\x1b]72;t=E;EIO:source failed\x1b\\"));
    assert!(controller.drag.file_requests.is_empty());
    assert!(handle(&mut controller, "t=k:x=2", b"").is_empty());
}

#[test]
fn thumbnails_use_one_chunk_chain_each_and_support_selection_and_removal() {
    let mut controller = preparing();
    let mut out = Vec::new();
    let pixels = vec![b'x'; 8000];
    assert_eq!(
        controller
            .add_thumbnail(
                &mut out,
                ThumbnailFormat::Rgba {
                    width: 100,
                    height: 20
                },
                &pixels
            )
            .unwrap(),
        0
    );
    assert_eq!(decoded(&out), pixels);
    let frames = output_messages(&out);
    for (index, frame) in frames.iter().enumerate() {
        let RawEvent::DragDrop { options, payload } = frame else {
            unreachable!()
        };
        assert_eq!(value(options, "x"), Some("-1"));
        assert_eq!(value(options, "y"), Some("32"));
        assert_eq!(value(options, "X"), Some("100"));
        assert_eq!(value(options, "Y"), Some("20"));
        assert_eq!(
            value(options, "m"),
            if index + 1 == frames.len() {
                None
            } else {
                Some("1")
            }
        );
        assert!(payload.len() <= 4096);
        assert_eq!(payload.is_empty(), index + 1 == frames.len());
    }
    out.clear();
    assert_eq!(
        controller
            .add_thumbnail(
                &mut out,
                ThumbnailFormat::Text {
                    numerator: 3,
                    denominator: 2,
                    opacity: 512
                },
                "📄".as_bytes()
            )
            .unwrap(),
        1
    );
    assert_eq!(
        output_messages(&out)[0],
        raw(
            "t=p:x=-2:y=0:X=3:Y=2:o=512:m=1",
            BASE64.encode("📄").as_bytes()
        )
    );
    assert!(controller.select_thumbnail(&mut out, Some(0)).is_err());
    controller.start_drag(&mut out).unwrap();
    handle(&mut controller, "t=E", b"OK");
    out.clear();
    controller.select_thumbnail(&mut out, Some(1)).unwrap();
    controller.select_thumbnail(&mut out, None).unwrap();
    assert_eq!(out, b"\x1b]72;t=P:x=1\x1b\\\x1b]72;t=P:x=2\x1b\\");
    assert!(controller.select_thumbnail(&mut out, Some(2)).is_err());
    assert!(
        controller
            .add_thumbnail(
                &mut out,
                ThumbnailFormat::Png {
                    width: 1,
                    height: 1
                },
                b"png"
            )
            .is_err()
    );
}

#[test]
fn invalid_thumbnail_arguments_emit_nothing_and_allow_correction() {
    let mut controller = preparing();
    let mut out = Vec::new();
    for (format, bytes) in [
        (
            ThumbnailFormat::Rgb {
                width: 0,
                height: 1,
            },
            b"".as_slice(),
        ),
        (
            ThumbnailFormat::Rgb {
                width: 1,
                height: 1,
            },
            b"ab",
        ),
        (
            ThumbnailFormat::Rgba {
                width: 1,
                height: 1,
            },
            b"abcde",
        ),
        (
            ThumbnailFormat::Png {
                width: u32::MAX,
                height: u32::MAX,
            },
            b"",
        ),
        (
            ThumbnailFormat::Text {
                numerator: 1,
                denominator: 0,
                opacity: 0,
            },
            b"a",
        ),
        (
            ThumbnailFormat::Text {
                numerator: 0,
                denominator: 1,
                opacity: 0,
            },
            b"a",
        ),
        (
            ThumbnailFormat::Text {
                numerator: 1,
                denominator: 1,
                opacity: 1025,
            },
            b"a",
        ),
        (
            ThumbnailFormat::Text {
                numerator: 1,
                denominator: 1,
                opacity: 0,
            },
            b"\xff",
        ),
    ] {
        assert!(controller.add_thumbnail(&mut out, format, bytes).is_err());
        assert!(out.is_empty());
    }
    assert_eq!(
        controller
            .add_thumbnail(
                &mut out,
                ThumbnailFormat::Rgb {
                    width: 1,
                    height: 1
                },
                b"rgb"
            )
            .unwrap(),
        0
    );
    assert_eq!(
        controller
            .add_thumbnail(
                &mut out,
                ThumbnailFormat::Png {
                    width: 1,
                    height: 1
                },
                b"opaque PNG bytes"
            )
            .unwrap(),
        1
    );
}

#[test]
fn thumbnail_source_failures_never_emit_success_and_poison_only_started_chains() {
    let format = ThumbnailFormat::Png {
        width: 1,
        height: 1,
    };
    for before_output in [true, false] {
        let mut controller = preparing();
        let mut out = Vec::new();
        assert!(matches!(
            controller.add_thumbnail_from(&mut out, format, &mut FailAfterBlock(before_output)),
            Err(Error::Io(_))
        ));
        assert!(controller.start_drag(&mut Vec::new()).is_err());
        assert!(
            controller
                .add_thumbnail(&mut Vec::new(), format, b"retry")
                .is_err()
        );
        if before_output {
            assert!(out.is_empty());
            controller.cancel_drag(&mut out).unwrap();
        } else {
            let frames = output_messages(&out);
            assert_eq!(frames.len(), 1);
            let RawEvent::DragDrop { options, payload } = &frames[0] else {
                unreachable!()
            };
            assert_eq!(value(options, "m"), Some("1"));
            assert!(!payload.is_empty());
            assert!(matches!(
                controller.cancel_drag(&mut Vec::new()),
                Err(Error::OutputFailed)
            ));
        }
    }
}

#[test]
fn thumbnail_reader_validates_final_length_and_split_utf8_before_completion() {
    for length in [3999, 4001] {
        let mut controller = preparing();
        let mut out = Vec::new();
        let data = vec![0; length];
        assert!(
            controller
                .add_thumbnail_from(
                    &mut out,
                    ThumbnailFormat::Rgba {
                        width: 100,
                        height: 10
                    },
                    &mut data.as_slice()
                )
                .is_err()
        );
        assert!(controller.output_failed);
        assert!(!output_messages(&out).iter().any(
            |frame| matches!(frame, RawEvent::DragDrop { payload, .. } if payload.is_empty())
        ));
    }
    let text = format!("{}💻", "a".repeat(3071));
    let mut controller = preparing();
    let mut out = Vec::new();
    let format = ThumbnailFormat::Text {
        numerator: 1,
        denominator: 1,
        opacity: 1024,
    };
    controller
        .add_thumbnail_from(
            &mut out,
            format,
            &mut ShortReader {
                bytes: text.as_bytes(),
                maximum: 7,
                interrupted: false,
            },
        )
        .unwrap();
    assert_eq!(decoded(&out), text.as_bytes());
    let mut controller = preparing();
    assert!(
        controller
            .add_thumbnail_from(&mut Vec::new(), format, &mut b"\xf0\x9f".as_slice())
            .is_err()
    );
}
