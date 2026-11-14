use super::*;
use crate::types::Identity;
use url::Url;

#[test]
fn raw_root_entries_are_bounded_and_invalidated_by_a_new_list() {
    let mut controller = file_drop();
    assert!(controller.root_entry(0).is_none());
    controller.request(&mut Vec::new(), Entry::UriList).unwrap();
    handle(
        &mut controller,
        "t=r:x=2",
        BASE64.encode(b"# comment\nfile:///one\n").as_bytes(),
    );
    assert!(controller.root_entry(0).is_none());
    handle(&mut controller, "t=r:x=2", b"");
    let old = controller.root_entry(0).unwrap();
    assert!(controller.root_entry(1).is_none());
    assert!(controller.root_entry(usize::MAX).is_none());
    controller.list(&mut Vec::new()).unwrap();
    assert!(controller.root_entry(0).is_none());
    handle(
        &mut controller,
        "t=r:x=2",
        BASE64.encode(b"file:///two\n").as_bytes(),
    );
    handle(&mut controller, "t=r:x=2", b"");
    assert!(controller.request(&mut Vec::new(), old).is_err());
    controller
        .request(&mut Vec::new(), controller.root_entry(0).unwrap())
        .unwrap();
    controller.cancel_drop(&mut Vec::new()).unwrap();
    assert!(controller.root_entry(0).is_none());
}

#[test]
fn optional_entry_and_record_limits_apply_before_events_escape() {
    for (entries, record, expected_error) in [
        (Some(0), None, true),
        (Some(1), None, true),
        (None, Some(3), true),
        (Some(2), Some(9), false),
        (None, None, false),
    ] {
        let options = DragDropOptions {
            max_entries: entries,
            max_record_bytes: record,
            ..DragDropOptions::default()
        };
        let mut controller = DragDrop::new(options.clone());
        controller.enable_drops(&mut Vec::new(), &[]).unwrap();
        handle(&mut controller, "t=M:o=1", b"text/uri-list");
        controller.list(&mut Vec::new()).unwrap();
        let result = controller.handle_event(&raw(
            "t=r:x=1",
            BASE64.encode(b"file:///a\nfile:///b\n").as_bytes(),
        ));
        assert_eq!(
            matches!(result, Err(Error::LimitExceeded(_))),
            expected_error
        );
        assert_eq!(controller.options(), &options);
        if !expected_error {
            assert_eq!(
                result
                    .unwrap()
                    .iter()
                    .filter(|e| matches!(e, Event::Listed { .. }))
                    .count(),
                2
            );
            handle(&mut controller, "t=r:x=1", b"");
            assert!(controller.root_entry(1).is_some());
        }
    }
}

#[test]
fn directory_limit_is_optional_and_counts_open_handles() {
    for limit in [Some(1), None] {
        let mut controller = DragDrop::new(DragDropOptions {
            max_directories: limit,
            ..DragDropOptions::default()
        });
        controller.enable_drops(&mut Vec::new(), &[]).unwrap();
        handle(&mut controller, "t=M:o=1", b"text/uri-list");
        controller.list(&mut Vec::new()).unwrap();
        handle(
            &mut controller,
            "t=r:x=1",
            BASE64.encode(b"file:///root/").as_bytes(),
        );
        handle(&mut controller, "t=r:x=1", b"");
        controller
            .request(&mut Vec::new(), controller.root_entry(0).unwrap())
            .unwrap();
        let entry = listed_entry(handle(
            &mut controller,
            "t=r:x=1:y=1:X=37",
            BASE64.encode(b"child\0").as_bytes(),
        ));
        handle(&mut controller, "t=r:x=1:y=1:X=37", b"");
        controller.request(&mut Vec::new(), entry).unwrap();
        let result = controller.handle_event(&raw("t=r:Y=37:x=1:X=38", b""));
        assert_eq!(
            matches!(result, Err(Error::LimitExceeded(_))),
            limit.is_some()
        );
    }
}

#[test]
fn raw_uri_lists_stream_long_records_without_buffering_names() {
    let mut controller = file_drop();
    controller.request(&mut Vec::new(), Entry::UriList).unwrap();
    handle(&mut controller, "t=r:x=2", BASE64.encode(b"#").as_bytes());
    let bytes = vec![b'a'; 4096];
    for _ in 0..32 {
        assert_eq!(
            handle(&mut controller, "t=r:x=2", BASE64.encode(&bytes).as_bytes()),
            [Event::Data(bytes.clone())]
        );
    }
    let tail = b"\r\nfile:///one\r\nfile:///two";
    handle(&mut controller, "t=r:x=2", BASE64.encode(tail).as_bytes());
    assert_eq!(handle(&mut controller, "t=r:x=2", b""), [Event::DataEnd]);
    controller
        .request(&mut Vec::new(), controller.root_entry(1).unwrap())
        .unwrap();
}

#[test]
fn invalid_buffered_thumbnails_remain_cancellable() {
    for (format, bytes) in [
        (
            ThumbnailFormat::Rgba {
                width: 16,
                height: 16,
            },
            vec![0; 1000],
        ),
        (
            ThumbnailFormat::Text {
                numerator: 1,
                denominator: 1,
                opacity: 0,
            },
            vec![b'a', 0xe2],
        ),
    ] {
        let mut controller = preparing();
        let mut out = Vec::new();
        assert!(
            controller
                .add_thumbnail_from(&mut out, format, &mut bytes.as_slice())
                .is_err()
        );
        assert!(out.is_empty());
        controller.cancel_drag(&mut out).unwrap();
        controller.close(&mut out).unwrap();
    }
}

#[test]
fn incoming_records_are_bounded_across_messages() {
    for directory in [false, true] {
        let mut controller = if directory {
            listed_drop()
        } else {
            file_drop()
        };
        let metadata = if directory {
            controller
                .request(&mut Vec::new(), uri(&controller, 1))
                .unwrap();
            "t=r:x=2:y=1:X=37"
        } else {
            controller.list(&mut Vec::new()).unwrap();
            "t=r:x=2:X=1"
        };
        let chunk = BASE64.encode(vec![b'a'; 4096]);
        for _ in 0..16 {
            handle(&mut controller, metadata, chunk.as_bytes());
        }
        assert!(matches!(
            controller.handle_event(&raw(metadata, b"YQ")),
            Err(Error::LimitExceeded(_))
        ));
    }
}

fn uri(controller: &DragDrop, index: u32) -> Entry {
    let scope = if controller.dropped.is_some() {
        controller.received.scope
    } else {
        controller.drag.scope
    };
    Entry::file(scope, 0, index)
}

fn directory(controller: &DragDrop, handle: i64) -> Directory {
    controller
        .received
        .directories
        .get(&handle)
        .map(|(directory, _)| *directory)
        .unwrap_or(Directory {
            scope: controller.received.scope,
            handle,
        })
}

fn child(controller: &DragDrop, handle: i64, index: u32) -> Entry {
    let scope = if controller.dropped.is_some() {
        directory(controller, handle).scope
    } else {
        controller.drag.scope
    };
    Entry::file(scope, handle, index)
}

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

fn listed_entry(events: Vec<Event>) -> Entry {
    events
        .into_iter()
        .find_map(|event| match event {
            Event::Listed { entry, .. } => Some(entry),
            _ => None,
        })
        .unwrap()
}

#[test]
fn returned_entries_drive_nested_work_without_terminal_child_requests() {
    let mut controller = DragDrop::default();
    let mut out = Vec::new();
    controller.enable_drags(&mut out).unwrap();
    handle(&mut controller, "t=o", b"");
    let roots = controller
        .offer_files(
            &mut out,
            &["file:///tree/", "file:///last"],
            Operations::COPY,
        )
        .unwrap();
    assert!(controller.next_send().unwrap().is_none());
    controller.start_drag(&mut out).unwrap();
    handle(&mut controller, "t=E", b"OK");
    handle(&mut controller, "t=k:x=1", b"");
    handle(&mut controller, "t=k:x=2", b"");
    handle(&mut controller, "t=k:x=1", b"");
    assert_eq!(controller.next_send().unwrap(), Some(roots[0].clone()));
    assert_eq!(controller.next_send().unwrap(), Some(roots[0].clone()));
    let children = controller
        .send_directory(&mut out, &roots[0], ["nested", "sibling"])
        .unwrap();
    assert_eq!(controller.next_send().unwrap(), Some(children[0].clone()));
    let nested = controller
        .send_directory(&mut out, &children[0], ["link"])
        .unwrap();
    assert_eq!(controller.next_send().unwrap(), Some(children[1].clone()));
    controller.send(&mut out, &children[1], b"sibling").unwrap();
    assert_eq!(controller.next_send().unwrap(), Some(nested[0].clone()));
    controller
        .send_symlink(&mut out, &nested[0], Path::new("../sibling"))
        .unwrap();
    assert_eq!(controller.next_send().unwrap(), Some(roots[1].clone()));
    controller.send(&mut out, &roots[1], b"last").unwrap();
    assert_eq!(controller.next_send().unwrap(), Some(roots[0].clone()));
    assert!(
        controller
            .send_directory(&mut out, &roots[0], std::iter::empty::<&OsStr>())
            .unwrap()
            .is_empty()
    );
    assert!(controller.next_send().unwrap().is_none());
    out.clear();
    assert!(controller.send(&mut out, &nested[0], b"expired").is_err());
    assert!(out.is_empty());
}

#[test]
fn file_tokens_reject_other_controllers_directions_and_exchanges() {
    let mut sender = file_drag();
    handle(&mut sender, "t=k:x=1", b"");
    let target = sender.next_send().unwrap().unwrap();
    let mut other = file_drag();
    handle(&mut other, "t=k:x=1", b"");
    let mut out = Vec::new();
    assert!(other.send(&mut out, &target, b"wrong controller").is_err());

    sender.enable_drops(&mut out, &[]).unwrap();
    handle(&mut sender, "t=M:o=1", b"text/uri-list");
    sender.list(&mut out).unwrap();
    let same_controller_drop = listed_entry(handle(&mut sender, "t=r:x=1:X=1", b"ZmlsZTovLy9hCg"));
    handle(&mut sender, "t=r:x=1:X=1", b"");
    out.clear();
    assert!(sender.request(&mut out, &target).is_err());
    assert!(
        sender
            .send(&mut out, &same_controller_drop, b"wrong direction")
            .is_err()
    );
    assert!(out.is_empty());
    sender.cancel_drop(&mut out).unwrap();
    out.clear();
    assert_eq!(sender.next_send().unwrap(), Some(target.clone()));

    let mut receiver = file_drop();
    receiver.list(&mut Vec::new()).unwrap();
    let incoming = listed_entry(handle(&mut receiver, "t=r:x=2:X=1", b"ZmlsZTovLy9hCg"));
    handle(&mut receiver, "t=r:x=2:X=1", b"");
    assert!(receiver.request(&mut out, &target).is_err());
    assert!(
        sender
            .send(&mut out, &incoming, b"wrong direction")
            .is_err()
    );
    assert!(out.is_empty());

    receiver.list(&mut out).unwrap();
    let replacement = listed_entry(handle(&mut receiver, "t=r:x=2:X=1", b"ZmlsZTovLy9iCg"));
    handle(&mut receiver, "t=r:x=2:X=1", b"");
    out.clear();
    assert!(receiver.request(&mut out, &incoming).is_err());
    assert!(out.is_empty());
    receiver.request(&mut out, replacement).unwrap();

    sender.cancel_drag(&mut out).unwrap();
    handle(&mut sender, "t=o", b"");
    sender
        .offer_files(&mut out, &["file:///replacement"], Operations::COPY)
        .unwrap();
    sender.start_drag(&mut out).unwrap();
    handle(&mut sender, "t=E", b"OK");
    handle(&mut sender, "t=k:x=1", b"");
    out.clear();
    assert!(sender.send(&mut out, &target, b"old exchange").is_err());
    assert!(out.is_empty());
    sender
        .send(&mut out, sender.next_send().unwrap().unwrap(), b"new")
        .unwrap();
}

#[test]
fn reopened_directory_handles_do_not_revive_closed_tokens() {
    let mut controller = file_drop();
    let mut out = Vec::new();
    controller.list(&mut out).unwrap();
    let root = listed_entry(handle(&mut controller, "t=r:x=2:X=1", b"ZmlsZTovLy9hCg"));
    handle(&mut controller, "t=r:x=2:X=1", b"");
    controller.request(&mut out, &root).unwrap();
    let mut events = handle(&mut controller, "t=r:x=2:y=1:X=42", b"YQ");
    let Event::Entry(EntryKind::Directory(old_directory)) = events[0] else {
        panic!("expected directory")
    };
    events.extend(handle(&mut controller, "t=r:x=2:y=1:X=42", b""));
    let old_child = listed_entry(events);
    controller.close_directory(&mut out, old_directory).unwrap();
    controller.request(&mut out, &root).unwrap();
    let mut events = handle(&mut controller, "t=r:x=2:y=1:X=42", b"Yg");
    events.extend(handle(&mut controller, "t=r:x=2:y=1:X=42", b""));
    let new_child = listed_entry(events);
    out.clear();
    assert!(controller.close_directory(&mut out, old_directory).is_err());
    assert!(controller.request(&mut out, old_child).is_err());
    assert!(out.is_empty());
    controller.request(&mut out, new_child).unwrap();
}

struct TestDirectory(std::path::PathBuf);

impl TestDirectory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "uncurses-dragdrop-{}-{}",
            std::process::id(),
            Identity::default().0,
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn path_offers_validate_all_sources_before_output_and_preserve_order() {
    let temporary = TestDirectory::new();
    let folder = temporary.0.join("folder");
    let file = temporary.0.join("a #%.txt");
    fs::create_dir(&folder).unwrap();
    fs::write(&file, b"data").unwrap();
    let mut controller = DragDrop::default();
    let mut out = Vec::new();
    controller.enable_drags(&mut out).unwrap();
    handle(&mut controller, "t=o", b"");
    out.clear();
    assert!(
        controller
            .offer_paths(
                &mut out,
                [&file, &temporary.0.join("missing")],
                Operations::COPY
            )
            .is_err()
    );
    assert!(out.is_empty());
    assert_eq!(controller.drag.phase, DragPhase::Requested);
    let entries = controller
        .offer_paths(&mut out, [&file, &folder, &file], Operations::COPY)
        .unwrap();
    let expected = [
        path_to_uri(&file).unwrap(),
        path_to_uri(&folder).unwrap(),
        path_to_uri(&file).unwrap(),
    ];
    let messages = output_messages(&out);
    let RawEvent::DragDrop { payload, .. } = &messages[1] else {
        panic!("expected URI data")
    };
    assert_eq!(
        BASE64.decode(payload).unwrap(),
        expected.join("\r\n").as_bytes()
    );
    assert!(expected[0].contains("a%20%23%25.txt"));
    assert!(expected[1].ends_with('/'));
    assert_ne!(entries[0], entries[2]);
    controller.start_drag(&mut out).unwrap();
    handle(&mut controller, "t=E", b"OK");
    for (index, entry) in entries.iter().enumerate() {
        let events = handle(&mut controller, &format!("t=k:x={}", index + 1), b"");
        assert_eq!(
            events,
            [Event::EntryRequested {
                entry: entry.clone()
            }]
        );
        controller.send(&mut out, entry, b"data").unwrap();
    }
}

#[cfg(unix)]
#[test]
fn path_offers_preserve_trailing_directory_separators() {
    use std::os::unix::fs::symlink;
    let temporary = TestDirectory::new();
    fs::write(temporary.0.join("file"), b"data").unwrap();
    fs::create_dir(temporary.0.join("folder")).unwrap();
    symlink("folder", temporary.0.join("folder-link")).unwrap();
    symlink("file", temporary.0.join("file-link")).unwrap();
    symlink("missing", temporary.0.join("broken-link")).unwrap();

    let mut controller = DragDrop::default();
    let mut out = Vec::new();
    controller.enable_drags(&mut out).unwrap();
    handle(&mut controller, "t=o", b"");
    out.clear();
    for name in [
        "file/",
        "file-link/",
        "broken-link/",
        "file/.",
        "file-link/.",
        "broken-link/.",
    ] {
        let path = temporary.0.join(name);
        assert!(fs::symlink_metadata(&path).is_err());
        assert!(path_to_uri(&path).is_err(), "{name}");
        assert!(
            controller
                .offer_paths(&mut out, [&path], Operations::COPY)
                .is_err()
        );
        assert!(out.is_empty());
        assert_eq!(controller.drag.phase, DragPhase::Requested);
    }

    for name in ["folder/", "folder-link/", "folder/.", "folder-link/."] {
        let path = temporary.0.join(name);
        let uri = path_to_uri(&path).unwrap();
        assert!(uri.ends_with('/'), "{uri}");
        let advertised = Url::parse(&uri).unwrap().to_file_path().unwrap();
        assert!(fs::symlink_metadata(advertised).unwrap().is_dir());
    }
    let uri = path_to_uri(temporary.0.join("folder-link")).unwrap();
    assert!(!uri.ends_with('/'));
    assert!(
        fs::symlink_metadata(Url::parse(&uri).unwrap().to_file_path().unwrap())
            .unwrap()
            .is_symlink()
    );
}

#[cfg(unix)]
#[test]
fn native_paths_preserve_symlinks_and_non_utf8_names() {
    use std::os::unix::{ffi::OsStrExt, fs::symlink};
    let temporary = TestDirectory::new();
    let name = OsStr::from_bytes(b"file\xff");
    #[cfg(not(target_os = "macos"))]
    {
        let file = temporary.0.join(name);
        fs::write(&file, b"data").unwrap();
        let uri = path_to_uri(&file).unwrap();
        assert!(uri.ends_with("file%FF"));
        assert_eq!(
            Url::parse(&uri).unwrap().to_file_path().unwrap(),
            fs::canonicalize(&file).unwrap()
        );
    }
    let link = temporary.0.join("link");
    symlink("missing", &link).unwrap();
    assert!(path_to_uri(&link).unwrap().ends_with("/link"));
    fs::create_dir_all(temporary.0.join("real/nested")).unwrap();
    fs::write(temporary.0.join("real/file"), b"data").unwrap();
    symlink("real/nested", temporary.0.join("alias")).unwrap();
    assert_eq!(
        Url::parse(&path_to_uri(temporary.0.join("alias/../file")).unwrap())
            .unwrap()
            .to_file_path()
            .unwrap(),
        fs::canonicalize(temporary.0.join("real/file")).unwrap(),
    );
    assert!(
        Url::parse(&path_to_uri("Cargo.toml").unwrap())
            .unwrap()
            .to_file_path()
            .unwrap()
            .is_absolute()
    );

    let mut controller = file_drag();
    handle(&mut controller, "t=k:x=1", b"");
    let root = controller.next_send().unwrap().unwrap();
    let mut out = Vec::new();
    let entries = controller.send_directory(&mut out, &root, [name]).unwrap();
    assert_eq!(decoded(&out), b"file\xff");
    out.clear();
    controller
        .send_symlink(
            &mut out,
            &entries[0],
            Path::new(OsStr::from_bytes(b"target\xff")),
        )
        .unwrap();
    assert_eq!(decoded(&out), b"target\xff");
}

#[cfg(windows)]
#[test]
fn unpaired_surrogates_fail_before_protocol_output() {
    use std::os::windows::ffi::OsStringExt;
    let invalid = std::ffi::OsString::from_wide(&[0xd800]);
    let mut controller = file_drag();
    handle(&mut controller, "t=k:x=1", b"");
    let root = controller.next_send().unwrap().unwrap();
    let mut out = Vec::new();
    assert!(
        controller
            .send_directory(&mut out, &root, [&invalid])
            .is_err()
    );
    assert!(
        controller
            .send_symlink(&mut out, &root, Path::new(&invalid))
            .is_err()
    );
    assert!(out.is_empty());
    assert_eq!(controller.next_send().unwrap(), Some(root));
}

#[test]
fn replacement_drop_releases_directory_state_and_preserves_outgoing_drag() {
    let mut controller = listed_drop();
    let mut out = Vec::new();
    controller.request(&mut out, uri(&controller, 1)).unwrap();
    handle(&mut controller, "t=r:x=2:y=1:X=42", b"Y2hpbGQ=");
    handle(&mut controller, "t=r:x=2:y=1:X=42", b"");
    controller
        .request(&mut out, child(&controller, 42, 1))
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
    assert!(
        controller
            .close_directory(&mut out, directory(&controller, 42))
            .is_err()
    );
    assert!(controller.request(&mut out, uri(&controller, 1)).is_err());
    assert!(
        controller
            .request(&mut out, child(&controller, 42, 1))
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
                    entry: uri(&controller, 1),
                    name: b"file:///one \t".to_vec()
                },
                Event::Listed {
                    entry: uri(&controller, 2),
                    name: b"file:///two".to_vec()
                },
                Event::Listed {
                    entry: uri(&controller, 3),
                    name: b"file:///three".to_vec()
                },
                Event::DataEnd,
            ]
        );
        assert!(controller.request(&mut out, uri(&controller, 0)).is_err());
        assert!(controller.request(&mut out, uri(&controller, 4)).is_err());
        out.clear();
        controller.request(&mut out, uri(&controller, 3)).unwrap();
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
        .request(&mut Vec::new(), uri(&controller, 1))
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
                entry: uri(&controller, 1),
                name: b"a".to_vec()
            },
            Event::DataEnd
        ]
    );
    controller
        .request(&mut Vec::new(), uri(&controller, 1))
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
            .request(&mut Vec::new(), uri(&controller, 1))
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
        controller.request(&mut out, uri(&controller, 1)).unwrap();
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
                Event::Entry(EntryKind::Directory(directory(&controller, handle_id))),
                Event::Listed {
                    entry: child(&controller, handle_id, 1),
                    name: b"a".to_vec()
                },
                Event::Listed {
                    entry: child(&controller, handle_id, 2),
                    name: b"\xffb".to_vec()
                },
                Event::Listed {
                    entry: child(&controller, handle_id, 3),
                    name: b"child".to_vec()
                },
                Event::DataEnd,
            ]
        );
        assert!(
            controller
                .request(&mut out, child(&controller, handle_id, 4))
                .is_err()
        );
        out.clear();
        controller
            .request(&mut out, child(&controller, handle_id, 2))
            .unwrap();
        assert_eq!(
            out,
            format!("\x1b]72;t=r:x=2:Y={handle_id}\x1b\\").as_bytes()
        );
        assert!(
            controller
                .close_directory(&mut out, directory(&controller, handle_id))
                .is_err()
        );
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
            .request(&mut out, child(&controller, handle_id, 3))
            .unwrap();
        let child_header = format!("t=r:x=3:Y={handle_id}:X=42");
        assert_eq!(
            handle(&mut controller, &child_header, b""),
            [
                Event::Entry(EntryKind::Directory(directory(&controller, 42))),
                Event::DataEnd
            ]
        );
        out.clear();
        controller
            .close_directory(&mut out, directory(&controller, handle_id))
            .unwrap();
        assert_eq!(out, format!("\x1b]72;t=r:Y={handle_id}\x1b\\").as_bytes());
        assert!(
            controller
                .close_directory(&mut out, directory(&controller, handle_id))
                .is_err()
        );
        assert!(
            controller
                .request(&mut out, child(&controller, handle_id, 1))
                .is_err()
        );
        controller
            .close_directory(&mut out, directory(&controller, 42))
            .unwrap();
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
            .request(&mut Vec::new(), uri(&controller, 1))
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
                .request(&mut Vec::new(), uri(&controller, 1))
                .is_err()
        );
    }
    let mut controller = listed_drop();
    controller
        .request(&mut Vec::new(), uri(&controller, 1))
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
        .request(&mut Vec::new(), uri(&controller, 1))
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
            .request(&mut Vec::new(), uri(&controller, 1))
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
            entry: uri(&controller, 2)
        }]
    );
    out.clear();
    let target = uri(&controller, 2);
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
            .send(&mut out, uri(&controller, 2), b"early")
            .is_err()
    );
    assert!(out.is_empty());
    let children = controller
        .send_directory_bytes(&mut out, uri(&controller, 1), &[b"sub", b"\xfffile"])
        .unwrap();
    assert_eq!(decoded(&out), b"sub\0\xfffile");
    out.clear();
    let grandchildren = controller
        .send_directory_bytes(&mut out, &children[0], &[b"link"])
        .unwrap();
    let grandchild = &grandchildren[0];
    assert!(controller.send(&mut out, grandchild, b"too soon").is_err());
    assert!(
        controller
            .send(&mut out, uri(&controller, 2), b"too soon")
            .is_err()
    );
    let sibling = &children[1];
    controller.send(&mut out, sibling, b"file").unwrap();
    controller
        .send_symlink_bytes(&mut out, grandchild, b"../\xfftarget")
        .unwrap();
    controller
        .send(&mut out, uri(&controller, 2), b"last")
        .unwrap();
    let frames = output_messages(&out);
    assert_eq!(frames[0], raw("t=k:x=1:Y=2:y=1:X=3", b"bGluaw"));
    assert_eq!(frames[2], raw("t=k:x=1:Y=2:y=2", b"ZmlsZQ"));
    assert_eq!(
        frames[4],
        raw(
            "t=k:x=1:Y=3:y=1:X=1",
            BASE64.encode(b"../\xfftarget").as_bytes()
        )
    );
    assert_eq!(frames[6], raw("t=k:x=2", b"bGFzdA"));
    assert!(
        controller
            .send(&mut out, uri(&controller, 2), b"duplicate")
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
                .send_directory_bytes(&mut out, uri(&controller, 1), &bad)
                .is_err()
        );
        assert!(out.is_empty());
    }
    let children = controller
        .send_directory_bytes(&mut out, uri(&controller, 1), &[])
        .unwrap();
    assert!(children.is_empty());
    assert_eq!(output_messages(&out), [raw("t=k:x=1:X=2", b"")]);
    controller
        .send_symlink_bytes(&mut out, uri(&controller, 1), b"target")
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
        controller.send_from(&mut out, uri(&controller, 1), &mut FailAfterBlock(false)),
        Err(Error::Io(_))
    ));
    assert!(!out.ends_with(b"\x1b]72;t=k:x=1\x1b\\"));
    assert!(matches!(
        controller.next_send(),
        Err(Error::InvalidState(_))
    ));
    assert!(
        controller
            .send(&mut out, uri(&controller, 1), b"retry")
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
    assert!(controller.next_send().unwrap().is_none());
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
