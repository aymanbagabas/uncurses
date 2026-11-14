use super::*;
use std::{collections::HashSet, fs};

#[cfg(unix)]
#[test]
fn source_paths_preserve_final_directory_dots() {
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir(temp.path().join("directory")).unwrap();
    fs::write(temp.path().join("file"), b"file").unwrap();
    std::os::unix::fs::symlink("directory", temp.path().join("link")).unwrap();
    let mut pool = DirectoryPool::new(Some(128));
    assert!(Source::new(&temp.path().join("file/."), &mut pool).is_err());
    let source = Source::new(&temp.path().join("link/."), &mut pool).unwrap();
    assert!(matches!(
        source.read(&mut pool).unwrap(),
        Data::Directory(_)
    ));
}

#[test]
fn root_sources_share_a_bounded_parent_handle() {
    let temp = tempfile::tempdir().unwrap();
    let mut pool = DirectoryPool::new(Some(1));
    let sources: Vec<_> = (0..32)
        .map(|index| {
            let path = temp.path().join(index.to_string());
            fs::write(&path, b"data").unwrap();
            Source::new(&path, &mut pool).unwrap()
        })
        .collect();
    for source in sources {
        assert!(matches!(source.read(&mut pool).unwrap(), Data::File(_)));
    }
}

#[test]
fn wide_trees_cancel_at_the_handle_limit_or_finish_without_it() {
    let temp = tempfile::tempdir().unwrap();
    for name in ["a", "b", "c"] {
        let directory = temp.path().join(name);
        fs::create_dir(&directory).unwrap();
        fs::write(directory.join("leaf"), b"leaf").unwrap();
    }
    for limit in [Some(2), None] {
        let mut controller = DragDrop::new(DragDropOptions {
            max_directories: limit,
            ..DragDropOptions::default()
        });
        let mut demo = Demo::new(vec![temp.path().to_owned()], None);
        let mut out = Vec::new();
        controller.enable_drags(&mut out).unwrap();
        for (metadata, payload) in [("t=o", ""), ("t=E", "OK"), ("t=k:x=1", "")] {
            for event in feed(&mut controller, metadata, payload) {
                demo.event(&mut controller, &mut out, event).unwrap();
            }
        }
        while controller.next_send().unwrap().is_some() {
            demo.send_next(&mut controller, &mut out).unwrap();
        }
        assert_eq!(demo.active.is_none(), limit.is_some());
        if limit.is_some() {
            assert!(demo.sources.is_empty());
            assert!(demo.log.iter().any(|line| line.contains("handle limit")));
            demo.selected = Content::Text;
            for event in feed(&mut controller, "t=o", "") {
                demo.event(&mut controller, &mut out, event).unwrap();
            }
            assert!(demo.active == Some(Content::Text));
        }
    }
}

#[test]
fn path_limit_cancels_the_drop_and_removes_partial_storage() {
    use base64::{Engine, engine::general_purpose::STANDARD_NO_PAD};
    let destination = tempfile::tempdir().unwrap();
    let mut controller = DragDrop::new(DragDropOptions {
        max_path_bytes: Some(3),
        ..DragDropOptions::default()
    });
    let mut demo = Demo::new(Vec::new(), Some(destination.path().to_owned()));
    let mut out = Vec::new();
    controller
        .enable_drops(&mut out, &["text/uri-list"])
        .unwrap();
    for (metadata, payload) in [
        ("t=M:o=1", "text/uri-list".to_owned()),
        (
            "t=r:x=1",
            STANDARD_NO_PAD.encode(b"file:///long\nfile:///also-long\n"),
        ),
    ] {
        for event in feed(&mut controller, metadata, &payload) {
            demo.event(&mut controller, &mut out, event).unwrap();
        }
    }
    assert!(!demo.receiver.active);
    assert_eq!(fs::read_dir(destination.path()).unwrap().count(), 0);
    assert!(demo.log.iter().any(|line| line.contains("path limit")));
    assert!(feed(&mut controller, "t=r:x=1", "").is_empty());
}

#[test]
fn browser_links_do_not_end_the_example() {
    use base64::{Engine, engine::general_purpose::STANDARD_NO_PAD};
    let mut controller = DragDrop::default();
    let mut demo = Demo::new(Vec::new(), None);
    let mut out = Vec::new();
    controller
        .enable_drops(&mut out, &["text/uri-list"])
        .unwrap();
    for (metadata, payload) in [
        ("t=M:o=1", "text/uri-list".to_owned()),
        (
            "t=r:x=1",
            STANDARD_NO_PAD.encode(b"https://example.com/\nfile:///safe.txt"),
        ),
        ("t=r:x=1", String::new()),
        ("t=r:x=1:y=2", String::new()),
    ] {
        for event in feed(&mut controller, metadata, &payload) {
            demo.event(&mut controller, &mut out, event).unwrap();
        }
    }
    assert!(
        demo.log
            .iter()
            .any(|line| line.contains("https://example.com/"))
    );
    assert!(demo.log.iter().any(|line| line.contains("Drop complete")));
}

#[cfg(unix)]
#[test]
fn source_directories_skip_socket_entries() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("file"), b"file").unwrap();
    fs::create_dir(temp.path().join("directory")).unwrap();
    std::os::unix::fs::symlink("file", temp.path().join("link")).unwrap();
    let _socket = std::os::unix::net::UnixListener::bind(temp.path().join("socket")).unwrap();
    let mut pool = DirectoryPool::new(Some(128));
    let Data::Directory(children) = Source::new(temp.path(), &mut pool)
        .unwrap()
        .read(&mut pool)
        .unwrap()
    else {
        panic!("expected a directory");
    };
    assert_eq!(
        children
            .iter()
            .map(|child| child.name.to_str().unwrap())
            .collect::<Vec<_>>(),
        ["directory", "file", "link"]
    );
}

#[test]
fn completed_parent_directories_close_before_their_descendants() {
    use base64::{Engine, engine::general_purpose::STANDARD_NO_PAD};
    let mut demo = Demo::new(Vec::new(), None);
    let mut controller = DragDrop::default();
    let mut out = Vec::new();
    controller
        .enable_drops(&mut out, &["text/uri-list"])
        .unwrap();
    for (metadata, payload) in [
        ("t=M:o=1", "text/uri-list".to_owned()),
        ("t=r:x=1:X=1", STANDARD_NO_PAD.encode(b"file:///root/")),
        ("t=r:x=1:X=1", String::new()),
        ("t=r:x=1:y=1:X=37", STANDARD_NO_PAD.encode(b"child")),
        ("t=r:x=1:y=1:X=37", String::new()),
        ("t=r:Y=37:x=1:X=38", STANDARD_NO_PAD.encode(b"leaf")),
        ("t=r:Y=37:x=1:X=38", String::new()),
    ] {
        for event in feed(&mut controller, metadata, &payload) {
            demo.event(&mut controller, &mut out, event).unwrap();
        }
    }
    let close = b"\x1b]72;t=r:Y=37\x1b\\";
    assert!(out.windows(close.len()).any(|bytes| bytes == close));
    let close_child = b"\x1b]72;t=r:Y=38\x1b\\";
    assert!(
        !out.windows(close_child.len())
            .any(|bytes| bytes == close_child)
    );
    for event in feed(&mut controller, "t=r:Y=38:x=1", "") {
        demo.event(&mut controller, &mut out, event).unwrap();
    }
    assert!(
        out.windows(close_child.len())
            .any(|bytes| bytes == close_child)
    );
}

#[cfg(unix)]
#[test]
fn renamed_source_ancestors_do_not_redirect_file_reads() {
    let temp = tempfile::tempdir().unwrap();
    let offered = temp.path().join("offered");
    let outside = temp.path().join("outside");
    fs::create_dir(&offered).unwrap();
    fs::create_dir(&outside).unwrap();
    fs::write(offered.join("file"), b"original").unwrap();
    fs::write(outside.join("file"), b"secret").unwrap();
    let mut demo = Demo::new(vec![offered.clone()], None);
    let original_uri = path_to_uri(&offered).unwrap();
    let mut controller = DragDrop::default();
    let mut out = Vec::new();
    controller.enable_drags(&mut out).unwrap();
    for (metadata, payload) in [("t=o", ""), ("t=E", "OK"), ("t=k:x=1", "")] {
        for event in feed(&mut controller, metadata, payload) {
            demo.event(&mut controller, &mut out, event).unwrap();
        }
    }
    demo.send_next(&mut controller, &mut out).unwrap();
    fs::rename(&offered, temp.path().join("original")).unwrap();
    std::os::unix::fs::symlink(&outside, &offered).unwrap();
    out.clear();
    demo.send_next(&mut controller, &mut out).unwrap();
    assert!(out.windows(11).any(|bytes| bytes == b"b3JpZ2luYWw"));
    out.clear();
    for event in feed(&mut controller, "t=e:x=5:y=0", "") {
        demo.event(&mut controller, &mut out, event).unwrap();
    }
    use base64::{Engine, engine::general_purpose::STANDARD_NO_PAD};
    let expected = STANDARD_NO_PAD.encode(original_uri);
    assert!(
        out.windows(expected.len())
            .any(|bytes| bytes == expected.as_bytes())
    );
}

fn feed(controller: &mut DragDrop, metadata: &str, payload: &str) -> Vec<Event> {
    controller.handle_event(&raw(metadata, payload)).unwrap()
}

fn raw(metadata: &str, payload: &str) -> TerminalEvent {
    TerminalEvent::DragDrop {
        options: metadata
            .split(':')
            .map(|field| {
                let (key, value) = field.split_once('=').unwrap();
                (key.to_owned(), value.to_owned())
            })
            .collect(),
        payload: payload.as_bytes().to_vec(),
    }
}

#[test]
fn rejected_destinations_cancel_only_the_drop() {
    use base64::{Engine, engine::general_purpose::STANDARD_NO_PAD};
    for (uris, bodies) in [
        (
            "file:///a/same\nfile:///b/same\n",
            vec![("t=r:x=1:y=1", ""), ("t=r:x=1:y=2", "")],
        ),
        ("file:///link\n", vec![("t=r:x=1:y=1:X=1", "../outside")]),
        (
            "file:///folder/\n",
            vec![("t=r:x=1:y=1:X=37", "../invalid\0")],
        ),
    ] {
        let destination = tempfile::tempdir().unwrap();
        let mut controller = DragDrop::default();
        let mut demo = Demo::new(Vec::new(), Some(destination.path().to_owned()));
        let mut out = Vec::new();
        controller.enable_drops(&mut out, MIMES).unwrap();
        for (metadata, payload) in [
            ("t=M:o=1", "text/uri-list".to_owned()),
            ("t=r:x=1", STANDARD_NO_PAD.encode(uris)),
            ("t=r:x=1", String::new()),
        ] {
            demo.handle_event(&mut controller, &mut out, &raw(metadata, &payload))
                .unwrap();
        }
        for (metadata, bytes) in bodies {
            if !bytes.is_empty() {
                demo.handle_event(
                    &mut controller,
                    &mut out,
                    &raw(metadata, &STANDARD_NO_PAD.encode(bytes)),
                )
                .unwrap();
            }
            demo.handle_event(&mut controller, &mut out, &raw(metadata, ""))
                .unwrap();
        }
        assert!(!demo.receiver.active);
        assert_eq!(fs::read_dir(destination.path()).unwrap().count(), 0);
        assert!(
            demo.log
                .iter()
                .any(|line| line.starts_with("Drop canceled:"))
        );
        for (metadata, payload) in [
            ("t=M:o=1", "text/plain"),
            ("t=r:x=1", "b2s"),
            ("t=r:x=1", ""),
        ] {
            demo.handle_event(&mut controller, &mut out, &raw(metadata, payload))
                .unwrap();
        }
        let saved = fs::read_dir(destination.path())
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        assert_eq!(fs::read(saved.join("text.txt")).unwrap(), b"ok");
    }
}

struct BrokenOutput {
    flush_only: bool,
}

#[test]
fn source_read_failures_recover_but_terminal_write_failures_propagate() {
    struct BrokenSource(bool);
    impl io::Read for BrokenSource {
        fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
            if self.0 {
                return Err(io::Error::other("source read failed"));
            }
            self.0 = true;
            bytes.fill(b'a');
            Ok(bytes.len())
        }
    }

    for output_failure in [false, true] {
        let file = tempfile::NamedTempFile::new().unwrap();
        let mut controller = DragDrop::default();
        let mut demo = Demo::new(vec![file.path().to_owned()], None);
        let mut out = Vec::new();
        controller.enable_drags(&mut out).unwrap();
        for (metadata, payload) in [("t=o", ""), ("t=E", "OK"), ("t=k:x=1", "")] {
            demo.handle_event(&mut controller, &mut out, &raw(metadata, payload))
                .unwrap();
        }
        let entry = controller.next_send().unwrap().unwrap();
        if output_failure {
            let mut out = BrokenOutput { flush_only: false };
            let error = controller.send(&mut out, entry, b"data").unwrap_err();
            let error = demo
                .source_error(&mut controller, &mut out, error)
                .unwrap_err();
            assert!(matches!(error, Error::Io(_)));
            assert_eq!(error.to_string(), "terminal write failed");
        } else {
            out.clear();
            let error = controller
                .send_from(&mut out, entry, &mut BrokenSource(false))
                .unwrap_err();
            assert!(!out.is_empty());
            demo.source_error(&mut controller, &mut out, error).unwrap();
            assert!(demo.sources.is_empty());
            assert!(demo.uri_list.is_empty());
            assert!(
                demo.log
                    .iter()
                    .any(|line| line.contains("source read failed"))
            );
            demo.selected = Content::Text;
            demo.handle_event(&mut controller, &mut out, &raw("t=o", ""))
                .unwrap();
            assert!(demo.active == Some(Content::Text));
        }
    }
}

impl Write for BrokenOutput {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.flush_only {
            Ok(bytes.len())
        } else {
            Err(io::Error::other("terminal write failed"))
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        Err(io::Error::other("terminal flush failed"))
    }
}

#[test]
fn failed_completion_output_keeps_storage_owned_for_cleanup() {
    for flush_only in [false, true] {
        let destination = tempfile::tempdir().unwrap();
        let mut controller = DragDrop::default();
        let mut demo = Demo::new(Vec::new(), Some(destination.path().to_owned()));
        let mut out = Vec::new();
        controller.enable_drops(&mut out, MIMES).unwrap();
        for (metadata, payload) in [("t=M:o=1", "text/plain"), ("t=r:x=1", "b2s")] {
            demo.handle_event(&mut controller, &mut out, &raw(metadata, payload))
                .unwrap();
        }
        assert!(
            demo.handle_event(
                &mut controller,
                &mut BrokenOutput { flush_only },
                &raw("t=r:x=1", "")
            )
            .is_err()
        );
        demo.receiver.abort().unwrap();
        assert_eq!(fs::read_dir(destination.path()).unwrap().count(), 0);
    }
}

#[test]
fn disappeared_sources_cancel_only_the_drag() {
    let source = tempfile::NamedTempFile::new().unwrap();
    let path = source.path().to_owned();
    let mut controller = DragDrop::default();
    let mut demo = Demo::new(vec![path.clone()], None);
    let mut out = Vec::new();
    controller.enable_drags(&mut out).unwrap();
    for (metadata, payload) in [("t=o", ""), ("t=E", "OK"), ("t=k:x=1", "")] {
        demo.handle_event(&mut controller, &mut out, &raw(metadata, payload))
            .unwrap();
    }
    source.close().unwrap();
    demo.send_next(&mut controller, &mut out).unwrap();
    assert!(demo.active.is_none());
    assert!(demo.sources.is_empty());
    assert!(
        demo.log
            .iter()
            .any(|line| line.starts_with("Drag canceled:"))
    );
    demo.selected = Content::Text;
    demo.handle_event(&mut controller, &mut out, &raw("t=o", ""))
        .unwrap();
    assert!(demo.active == Some(Content::Text));
}

#[cfg(windows)]
#[test]
fn unpaired_windows_child_names_cancel_the_drag() {
    use std::{ffi::OsString, os::windows::ffi::OsStringExt};
    let directory = tempfile::tempdir().unwrap();
    fs::write(
        directory.path().join(OsString::from_wide(&[0xd800])),
        b"data",
    )
    .unwrap();
    let mut controller = DragDrop::default();
    let mut demo = Demo::new(vec![directory.path().to_owned()], None);
    let mut out = Vec::new();
    controller.enable_drags(&mut out).unwrap();
    for (metadata, payload) in [("t=o", ""), ("t=E", "OK"), ("t=k:x=1", "")] {
        demo.handle_event(&mut controller, &mut out, &raw(metadata, payload))
            .unwrap();
    }
    demo.send_next(&mut controller, &mut out).unwrap();
    assert!(demo.active.is_none());
    assert!(demo.sources.is_empty());
    assert!(
        demo.log
            .iter()
            .any(|line| line.starts_with("Drag canceled:"))
    );
    demo.selected = Content::Text;
    demo.handle_event(&mut controller, &mut out, &raw("t=o", ""))
        .unwrap();
    assert!(demo.active == Some(Content::Text));
}

#[test]
fn received_children_precede_siblings_and_keep_chunked_listing_order() {
    use base64::{Engine, engine::general_purpose::STANDARD_NO_PAD};
    let mut demo = Demo::new(Vec::new(), None);
    let mut controller = DragDrop::new(DragDropOptions {
        max_directories: Some(2),
        ..DragDropOptions::default()
    });
    let mut out = Vec::new();
    controller
        .enable_drops(&mut out, &["text/uri-list"])
        .unwrap();
    for (metadata, payload) in [
        ("t=M:o=1", "text/uri-list".to_owned()),
        ("t=r:x=1", STANDARD_NO_PAD.encode(b"file:///root")),
        ("t=r:x=1", String::new()),
        (
            "t=r:x=1:y=1:X=37",
            STANDARD_NO_PAD.encode(b"first\0second\0third"),
        ),
        ("t=r:x=1:y=1:X=37", String::new()),
        ("t=r:Y=37:x=1:X=38", STANDARD_NO_PAD.encode(b"a\0")),
        ("t=r:Y=37:x=1:X=38", STANDARD_NO_PAD.encode(b"b")),
        ("t=r:Y=37:x=1:X=38", String::new()),
        ("t=r:Y=38:x=1", STANDARD_NO_PAD.encode(b"first")),
        ("t=r:Y=38:x=1", String::new()),
        ("t=r:Y=38:x=2", STANDARD_NO_PAD.encode(b"second")),
        ("t=r:Y=38:x=2", String::new()),
        ("t=r:Y=37:x=2:X=39", STANDARD_NO_PAD.encode(b"c")),
        ("t=r:Y=37:x=2:X=39", String::new()),
        ("t=r:Y=39:x=1", STANDARD_NO_PAD.encode(b"third")),
        ("t=r:Y=39:x=1", String::new()),
        ("t=r:Y=37:x=3:X=40", String::new()),
    ] {
        demo.handle_event(&mut controller, &mut out, &raw(metadata, &payload))
            .unwrap();
    }
    assert!(
        demo.log
            .iter()
            .any(|line| line.starts_with("Drop complete"))
    );
    assert!(demo.receiver.preview.contains("firstsecondthird"));
}

#[test]
fn malformed_uri_lists_cancel_both_roles_and_allow_later_transfers() {
    use base64::{Engine, engine::general_purpose::STANDARD_NO_PAD};
    let source = tempfile::NamedTempFile::new().unwrap();
    let destination = tempfile::tempdir().unwrap();
    let mut controller = DragDrop::default();
    let mut demo = Demo::new(
        vec![source.path().to_owned()],
        Some(destination.path().to_owned()),
    );
    let mut out = Vec::new();
    controller.enable_drags(&mut out).unwrap();
    controller.enable_drops(&mut out, MIMES).unwrap();
    for (metadata, payload) in [("t=o", ""), ("t=E", "OK"), ("t=M:o=1", "text/uri-list")] {
        demo.handle_event(&mut controller, &mut out, &raw(metadata, payload))
            .unwrap();
    }
    demo.handle_event(
        &mut controller,
        &mut out,
        &raw("t=r:x=1", &STANDARD_NO_PAD.encode(b"file:///queued\n")),
    )
    .unwrap();
    assert_eq!(demo.receiver.listed, 1);
    assert!(!demo.sources.is_empty());
    assert_eq!(fs::read_dir(destination.path()).unwrap().count(), 1);

    out.clear();
    demo.handle_event(
        &mut controller,
        &mut out,
        &raw("t=r:x=1", &STANDARD_NO_PAD.encode(b"file:///bad\0name")),
    )
    .unwrap();
    assert_eq!(out, b"\x1b]72;t=r:o=0\x1b\\\x1b]72;t=E:y=-1\x1b\\");
    assert!(demo.active.is_none());
    assert!(demo.sources.is_empty());
    assert!(!demo.receiver.active);
    assert!(demo.receiver.next().is_none());
    assert_eq!(fs::read_dir(destination.path()).unwrap().count(), 0);
    assert!(demo.log.iter().any(|line| line.contains("NUL in URI list")));

    for (metadata, payload) in [
        ("t=r:x=1", String::new()),
        ("t=M:o=1", "text/plain".to_owned()),
        ("t=r:x=1", STANDARD_NO_PAD.encode(b"new")),
        ("t=r:x=1", String::new()),
    ] {
        demo.handle_event(&mut controller, &mut out, &raw(metadata, &payload))
            .unwrap();
    }
    let saved = fs::read_dir(destination.path())
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    assert_eq!(fs::read(saved.join("text.txt")).unwrap(), b"new");
    demo.selected = Content::Text;
    demo.handle_event(&mut controller, &mut out, &raw("t=o", ""))
        .unwrap();
    assert!(demo.active == Some(Content::Text));
}

#[test]
fn control_limit_cancels_both_roles_and_drains_continuations() {
    for more in [false, true] {
        let source = tempfile::NamedTempFile::new().unwrap();
        let destination = tempfile::tempdir().unwrap();
        let mut controller = DragDrop::new(DragDropOptions {
            max_control_bytes: Some(64),
            ..DragDropOptions::default()
        });
        let mut demo = Demo::new(
            vec![source.path().to_owned()],
            Some(destination.path().to_owned()),
        );
        let mut out = Vec::new();
        controller.enable_drags(&mut out).unwrap();
        controller.enable_drops(&mut out, MIMES).unwrap();
        for (metadata, payload) in [("t=o", ""), ("t=E", "OK"), ("t=M:o=1", "text/plain")] {
            demo.handle_event(&mut controller, &mut out, &raw(metadata, payload))
                .unwrap();
        }
        let metadata = if more { "t=E:m=1" } else { "t=E" };
        demo.handle_event(&mut controller, &mut out, &raw(metadata, &"x".repeat(65)))
            .unwrap();
        assert!(demo.active.is_none());
        assert!(demo.sources.is_empty());
        assert!(!demo.receiver.active);
        assert_eq!(fs::read_dir(destination.path()).unwrap().count(), 0);
        if more {
            demo.handle_event(&mut controller, &mut out, &raw("t=o", ""))
                .unwrap();
            assert!(demo.active.is_none());
        }
        demo.selected = Content::Text;
        demo.handle_event(&mut controller, &mut out, &raw("t=o", ""))
            .unwrap();
        assert!(demo.active == Some(Content::Text));
    }
}

#[test]
fn selecting_the_next_drag_preserves_the_active_mime_body() {
    let mut controller = DragDrop::default();
    let mut demo = Demo::new(Vec::new(), None);
    let mut out = Vec::new();
    controller.enable_drags(&mut out).unwrap();
    for event in feed(&mut controller, "t=o", "") {
        demo.event(&mut controller, &mut out, event).unwrap();
    }
    for event in feed(&mut controller, "t=E", "OK") {
        demo.event(&mut controller, &mut out, event).unwrap();
    }
    demo.key(&"p".parse().unwrap());
    assert!(demo.selected == Content::Png);
    assert!(demo.active == Some(Content::Text));
    out.clear();
    for event in feed(&mut controller, "t=e:x=5:y=0", "") {
        demo.event(&mut controller, &mut out, event).unwrap();
    }
    assert!(out.windows(8).any(|bytes| bytes == b"SGVsbG8g"));
    demo.event(&mut controller, &mut out, Event::DropCanceled)
        .unwrap();
    assert!(demo.active == Some(Content::Text));
}

#[test]
fn offered_formats_supply_their_own_bytes() {
    assert_eq!(Content::Html.body("text/html", &[]).unwrap(), HTML);
    assert_eq!(Content::Html.body("text/plain", &[]).unwrap(), TEXT);
    assert_eq!(
        Content::Text.body("UTF8_STRING", &[]).unwrap(),
        TEXT.repeat(512)
    );
    assert_eq!(Content::Png.body("image/png", &[]).unwrap(), PNG);
    assert_eq!(Content::Jpeg.body("image/jpeg", &[]).unwrap(), JPEG);
    assert!(Content::Png.body("text/plain", &[]).is_err());
}

#[test]
fn replacement_drops_preserve_outgoing_file_sources() {
    let source = tempfile::NamedTempFile::new().unwrap();
    let destination = tempfile::tempdir().unwrap();
    let mut demo = Demo::new(
        vec![source.path().to_owned()],
        Some(destination.path().to_owned()),
    );
    let mut controller = DragDrop::default();
    let mut out = Vec::new();
    controller.enable_drags(&mut out).unwrap();
    for event in feed(&mut controller, "t=o", "") {
        demo.event(&mut controller, &mut out, event).unwrap();
    }
    let sources: HashSet<_> = demo.sources.keys().cloned().collect();
    assert_eq!(sources.len(), 1);
    demo.receiver
        .begin("text/plain", Some(destination.path()))
        .unwrap();
    demo.receiver.data(b"unfinished").unwrap();
    demo.event(&mut controller, &mut out, Event::DropCanceled)
        .unwrap();
    assert_eq!(
        demo.sources.keys().cloned().collect::<HashSet<_>>(),
        sources
    );
    assert!(demo.active == Some(Content::Files));
    assert_eq!(fs::read_dir(destination.path()).unwrap().count(), 0);
    assert_eq!(demo.receiver.bytes, 0);
    assert_eq!(demo.receiver.listed, 0);
    assert_eq!(demo.receiver.checksum, 0);
    assert!(demo.receiver.preview.is_empty());
}
