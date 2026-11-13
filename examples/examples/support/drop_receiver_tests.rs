use super::*;

#[test]
fn incoming_names_stay_inside_the_destination() {
    for name in ["", ".", "..", "../escape", "/absolute", "a/b", "a/", "a\0b"] {
        assert!(component(OsStr::new(name)).is_err(), "{name:?}");
    }
    component(OsStr::new("line\nbreak")).unwrap();
    assert_eq!(
        root_name(b"file:///tmp/photo%20%23.png").unwrap(),
        "photo #.png"
    );
    assert!(root_name(b"https://example.com/image.png").is_err());
    assert!(root_name(b"file:///").is_err());
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        assert_eq!(root_name(b"file:///tmp/%ff").unwrap().as_bytes(), b"\xff");
    }
}

#[test]
fn file_uri_names_are_independent_of_the_source_platform() {
    for uri in [
        b"file:///home/user/photo%20%23.png".as_slice(),
        b"file:///C:/Users/user/photo%20%23.png",
        b"file://desktop/share/photo%20%23.png",
    ] {
        assert_eq!(root_name(uri).unwrap(), "photo #.png");
    }
    assert_eq!(root_name(b"file:///home/user/photos/").unwrap(), "photos");
    assert_eq!(
        root_name(b"file:///C:/Users/user/photos/").unwrap(),
        "photos"
    );
    for uri in [
        b"file:///tmp/encoded%2Fseparator".as_slice(),
        b"file:///tmp/encoded%00nul",
        b"https://example.com/photo.png",
        b"file:///",
    ] {
        assert!(root_name(uri).is_err(), "{uri:?}");
    }
}

#[test]
fn preview_keeps_a_contiguous_prefix_across_chunks() {
    let mut receiver = Receiver::default();
    receiver.begin("text/plain", None).unwrap();
    receiver.data(&[b'a'; 255]).unwrap();
    receiver.data(b"\xff").unwrap();
    receiver.data(b"z").unwrap();
    assert_eq!(receiver.preview, "a".repeat(255));
    assert_eq!(receiver.bytes, 257);
    receiver.begin("text/plain", None).unwrap();
    receiver.data(b"new").unwrap();
    assert_eq!(receiver.preview, "new");
}

#[test]
fn preview_is_bounded_and_does_not_create_storage() {
    let mut receiver = Receiver::default();
    receiver.begin("text/html", None).unwrap();
    receiver.data(b"<script>\x1b[31m").unwrap();
    receiver.data(&[0xff; 10_000]).unwrap();
    assert!(receiver.preview.contains("\\x1b"));
    assert!(receiver.preview.len() <= PREVIEW_LIMIT);
    assert_eq!(receiver.bytes, 10_013);
    receiver.end_body().unwrap();
    assert!(receiver.finish().is_none());
}

#[test]
fn saves_exact_bytes_in_distinct_directories_and_removes_partial_drops() {
    let destination = tempfile::tempdir().unwrap();
    let mut receiver = Receiver::default();
    receiver
        .begin("image/png", Some(destination.path()))
        .unwrap();
    receiver.data(b"\x89PNG\0\xff").unwrap();
    receiver.end_body().unwrap();
    let first = receiver.finish().unwrap();
    assert_eq!(fs::read(first.join("image.png")).unwrap(), b"\x89PNG\0\xff");

    receiver
        .begin("image/png", Some(destination.path()))
        .unwrap();
    receiver.data(b"second").unwrap();
    receiver.end_body().unwrap();
    let second = receiver.finish().unwrap();
    assert_ne!(first, second);
    assert_eq!(fs::read(second.join("image.png")).unwrap(), b"second");

    receiver
        .begin("text/plain", Some(destination.path()))
        .unwrap();
    receiver.data(b"incomplete").unwrap();
    let partial = receiver.storage.as_ref().unwrap().path().to_owned();
    receiver.abort().unwrap();
    assert!(!partial.exists());
    assert!(first.exists() && second.exists());
}

#[test]
fn duplicate_file_names_do_not_overwrite_data() {
    let destination = tempfile::tempdir().unwrap();
    let mut receiver = Receiver::default();
    receiver
        .begin("text/uri-list", Some(destination.path()))
        .unwrap();
    receiver.current = Some("same".into());
    receiver.kind(EntryKind::File).unwrap();
    receiver.data(b"original").unwrap();
    receiver.end_body().unwrap();
    receiver.current = Some("same".into());
    assert_eq!(
        receiver.kind(EntryKind::File).unwrap_err().kind(),
        io::ErrorKind::AlreadyExists
    );
    assert_eq!(
        fs::read(receiver.storage.as_ref().unwrap().path().join("same")).unwrap(),
        b"original"
    );
    receiver.abort().unwrap();
}

#[cfg(unix)]
#[test]
fn symlinks_are_preserved_without_following_them() {
    let destination = tempfile::tempdir().unwrap();
    let mut receiver = Receiver::default();
    receiver
        .begin("text/uri-list", Some(destination.path()))
        .unwrap();
    receiver.current = Some("link".into());
    receiver.kind(EntryKind::Symlink).unwrap();
    receiver.data(b"missing").unwrap();
    receiver.end_body().unwrap();
    let saved = receiver.finish().unwrap();
    assert_eq!(
        fs::read_link(saved.join("link")).unwrap(),
        Path::new("missing")
    );

    for target in [b"../outside".as_slice(), b"/absolute"] {
        receiver
            .begin("text/uri-list", Some(destination.path()))
            .unwrap();
        receiver.current = Some("link".into());
        receiver.kind(EntryKind::Symlink).unwrap();
        receiver.data(target).unwrap();
        assert!(receiver.end_body().is_err());
        receiver.abort().unwrap();
    }
}
