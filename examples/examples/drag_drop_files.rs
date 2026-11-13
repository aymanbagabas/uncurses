//! Exercise file lists, remote entries, directory trees, and thumbnails.
//!
//! Run `cargo run -p examples --example drag_drop_files -- [PATH ...]` in Kitty.
//! Drop files to inspect their entries and byte counts without saving them.
//! Supply source paths to copy files or directory trees out of the terminal.
//! Select the next thumbnail with 1 (image), 2 (text), or 0 (none). q quits.
//! `--scripted` runs an in-memory demonstration. `--native` connects its handler
//! to the native replay harness through a line-based test transport.

#[path = "support/drag_drop.rs"]
mod support;

use std::collections::VecDeque;
use std::ffi::OsStr;
use std::fs;
use std::io::{self, BufRead, Read, Write};
use std::path::{Path, PathBuf};

use base64::{Engine, engine::general_purpose::STANDARD};
use uncurses::ansi::drag_drop::REQUEST_DRAG_DROP;
use uncurses::buffer::SurfaceMut;
use uncurses::event::{Event as TerminalEvent, Key};
use uncurses::program::Program;
use uncurses::style::Style;
use uncurses::terminal::{Stdin, Stdout};
use uncurses::text::TextSurface;
use uncurses_dragdrop::{
    DragDrop, DragDropOptions, Entry, EntryKind, Error, Event, MachineId, Operation, Operations,
    ThumbnailFormat,
};
use url::Url;

const URIS: &[&str] = &["file:///demo/", "file:///standalone.bin"];

#[derive(Default)]
struct Files {
    pending: VecDeque<Entry>,
    directories: Vec<i64>,
    received: usize,
    checksum: u64,
    listed: usize,
    text_preview: bool,
    interactive: bool,
    roots: Vec<PathBuf>,
    uris: Vec<String>,
    log: VecDeque<String>,
    thumbnail: support::Thumbnail,
}

impl Files {
    fn log(&mut self, message: impl Into<String>) {
        let message = message.into();
        if !self.interactive {
            eprintln!("{message}");
        }
        self.log.push_back(message);
        if self.log.len() > 8 {
            self.log.pop_front();
        }
    }

    fn handle(
        &mut self,
        controller: &mut DragDrop,
        out: &mut impl Write,
        event: Event,
    ) -> Result<(), Error> {
        match event {
            Event::Hover(offer) => {
                if offer.operations.contains(Operations::COPY)
                    && offer.mime_types.iter().any(|mime| mime == "text/uri-list")
                {
                    controller.accept(out, &[], Operation::Copy)?;
                } else {
                    controller.reject(out)?;
                }
            }
            Event::Drop(offer) => {
                if !offer.operations.contains(Operations::COPY)
                    || !offer.mime_types.iter().any(|mime| mime == "text/uri-list")
                {
                    controller.cancel_drop(out)?;
                    return Ok(());
                }
                self.pending.clear();
                self.directories.clear();
                self.received = 0;
                self.checksum = 0xcbf29ce484222325;
                self.listed = 0;
                controller.list(out)?;
            }
            Event::UriList { remote } => self.log(format!("URI list: remote={remote}")),
            Event::Listed { entry, name } => {
                let name: String = name
                    .iter()
                    .flat_map(|byte| byte.escape_ascii())
                    .map(char::from)
                    .collect();
                self.log(format!("Listed {entry:?}: {name}"));
                self.pending.push_back(entry);
                self.listed += 1;
            }
            Event::Entry(kind) => {
                self.log(format!("Entry kind: {kind:?}"));
                if let EntryKind::Directory { handle } = kind {
                    self.directories.push(handle);
                }
            }
            Event::Data(bytes) => {
                self.received += bytes.len();
                for byte in bytes {
                    self.checksum = (self.checksum ^ u64::from(byte)).wrapping_mul(0x100000001b3);
                }
            }
            Event::DataEnd => {
                if let Some(entry) = self.pending.pop_front() {
                    controller.request(out, entry)?;
                } else {
                    for handle in self.directories.drain(..) {
                        controller.close_directory(out, handle)?;
                    }
                    controller.complete_drop(out, Operation::Copy)?;
                    self.log(format!(
                        "Drop complete: {} bytes, {} entries, checksum={:016x}",
                        self.received, self.listed, self.checksum,
                    ));
                }
            }
            Event::DragRequested(_) => {
                if self.interactive {
                    let uris: Vec<_> = self.uris.iter().map(String::as_str).collect();
                    controller.offer_files(out, &uris, Operations::COPY)?;
                    self.thumbnail.upload(controller, out)?;
                    controller.start_drag(out)?;
                    self.log(format!("Offered {} paths for copy", self.roots.len()));
                    return Ok(());
                }
                controller.offer_files(out, URIS, Operations::COPY)?;
                controller.add_thumbnail(
                    out,
                    ThumbnailFormat::Rgba {
                        width: 16,
                        height: 16,
                    },
                    &[0x88; 16 * 16 * 4],
                )?;
                {
                    controller.add_thumbnail_from(
                        out,
                        ThumbnailFormat::Rgb {
                            width: 1,
                            height: 1,
                        },
                        &mut [255, 0, 0].as_slice(),
                    )?;
                    let png = STANDARD
                        .decode(concat!(
                            "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJ",
                            "AAAADUlEQVR4nGP4z8DwHwAFAAH/iZk9HQAAAABJRU5ErkJggg==",
                        ))
                        .expect("valid example PNG");
                    controller.add_thumbnail(
                        out,
                        ThumbnailFormat::Png {
                            width: 1,
                            height: 1,
                        },
                        &png,
                    )?;
                }
                if self.text_preview {
                    controller.add_thumbnail(
                        out,
                        ThumbnailFormat::Text {
                            numerator: 1,
                            denominator: 1,
                            opacity: 512,
                        },
                        b"Files",
                    )?;
                }
                controller.start_drag(out)?;
            }
            Event::DragStarted => {
                if self.interactive {
                    self.log("Drag started");
                } else {
                    controller.select_thumbnail(out, None)?;
                    controller.select_thumbnail(out, Some(0))?;
                    self.log("Drag started; thumbnail removed and restored");
                }
            }
            Event::EntryRequested { entry } if self.interactive => {
                let Entry::UriEntry(index) = entry else {
                    return Err(Error::InvalidInput("expected a root entry"));
                };
                let path = self
                    .roots
                    .get(index as usize - 1)
                    .ok_or(Error::InvalidInput("unknown source entry"))?;
                send_path(controller, out, entry, path)?;
                self.log(format!("Sent source entry {index}"));
            }
            Event::EntryRequested { entry } => match entry {
                Entry::UriEntry(1) => {
                    let root = controller.send_directory(
                        out,
                        entry,
                        &[b"movie.bin", b"link", b"nested"],
                    )?;
                    controller.send_from(
                        out,
                        Entry::DirectoryEntry {
                            handle: root,
                            index: 1,
                        },
                        &mut io::repeat(0x5a).take(10_000),
                    )?;
                    controller.send_symlink(
                        out,
                        Entry::DirectoryEntry {
                            handle: root,
                            index: 2,
                        },
                        b"movie.bin",
                    )?;
                    let nested = controller.send_directory(
                        out,
                        Entry::DirectoryEntry {
                            handle: root,
                            index: 3,
                        },
                        &[b"empty"],
                    )?;
                    controller.send(
                        out,
                        Entry::DirectoryEntry {
                            handle: nested,
                            index: 1,
                        },
                        b"",
                    )?;
                    self.log("Sent tree: movie.bin, link, nested/empty");
                }
                Entry::UriEntry(2) => {
                    controller.send(out, entry, b"standalone")?;
                    self.log("Sent standalone file");
                }
                _ => return Err(Error::InvalidInput("unexpected example entry")),
            },
            Event::DataRequested { mime_type } => {
                let body = if self.interactive {
                    format!("{}\r\n", self.uris.join("\r\n"))
                } else {
                    format!("{}\r\n", URIS.join("\r\n"))
                };
                controller.send(out, mime_type, body.as_bytes())?;
            }
            Event::DropCanceled => {
                self.pending.clear();
                self.directories.clear();
                self.log("Drop canceled by a new drag");
            }
            Event::DropFailed(error) => {
                self.pending.clear();
                self.directories.clear();
                self.log(format!("Drop failed: {error}"));
            }
            Event::DragFailed(error) => self.log(format!("Drag failed: {error}")),
            Event::DragFinished { canceled } => {
                self.log(format!("Drag finished: canceled={canceled}"))
            }
            _ => {}
        }
        Ok(())
    }

    fn feed(
        &mut self,
        controller: &mut DragDrop,
        out: &mut impl Write,
        metadata: &str,
        payload: &[u8],
    ) -> Result<(), Error> {
        let options = metadata
            .split(':')
            .map(|pair| {
                let (key, value) = pair
                    .split_once('=')
                    .ok_or(Error::InvalidInput("expected key=value metadata"))?;
                Ok((key.to_owned(), value.to_owned()))
            })
            .collect::<Result<_, Error>>()?;
        let event = TerminalEvent::DragDrop {
            options,
            payload: payload.to_vec(),
        };
        for event in controller.handle_event(&event)? {
            self.handle(controller, out, event)?;
        }
        Ok(())
    }
}

fn replay(native: bool) -> Result<(), Box<dyn std::error::Error>> {
    // A synthetic identity forces remote handling in the replay. Real programs
    // should use MachineId::current() to identify the application host.
    let mut controller = DragDrop::new(DragDropOptions {
        machine_id: Some(format!("1:{}", "0".repeat(64)).parse()?),
        ..DragDropOptions::default()
    });
    // Kitty's fake native window has no fonts for a text thumbnail.
    let mut demo = Files {
        text_preview: !native,
        ..Files::default()
    };
    let mut out = Vec::new();
    controller.enable_drops(&mut out, &["text/uri-list"])?;
    controller.enable_drags(&mut out)?;
    if native {
        let mut stdout = io::stdout().lock();
        writeln!(stdout, "{}", STANDARD.encode(&out))?;
        stdout.flush()?;
        for line in io::stdin().lock().lines() {
            let line = line?;
            let (metadata, payload) = line
                .split_once('\t')
                .ok_or("expected metadata TAB base64")?;
            out.clear();
            demo.feed(
                &mut controller,
                &mut out,
                metadata,
                &STANDARD.decode(payload)?,
            )?;
            writeln!(stdout, "{}", STANDARD.encode(&out))?;
            stdout.flush()?;
        }
    } else {
        demo.feed(&mut controller, &mut out, "t=M:o=1", b"text/uri-list")?;
        for (metadata, bytes) in [
            ("t=r:x=1:X=1", b"file:///incoming/\r\n".as_slice()),
            ("t=r:x=1:X=1", b""),
            ("t=r:x=1:y=1:X=2", b"one\0link\0sub"),
            ("t=r:x=1:y=1:X=2", b""),
            ("t=r:Y=2:x=1", b"example file"),
            ("t=r:Y=2:x=1", b""),
            ("t=r:Y=2:x=2:X=1", b"one"),
            ("t=r:Y=2:x=2:X=1", b""),
            ("t=r:Y=2:x=3:X=3", b""),
        ] {
            demo.feed(
                &mut controller,
                &mut out,
                metadata,
                STANDARD.encode(bytes).as_bytes(),
            )?;
        }
        for metadata in ["t=o", "t=E", "t=k:x=1", "t=k:x=2", "t=e:x=4:y=0"] {
            let payload = if metadata == "t=E" {
                b"OK".as_slice()
            } else {
                b""
            };
            demo.feed(&mut controller, &mut out, metadata, payload)?;
        }
        controller.close(&mut out)?;
        println!(
            "Completed the scripted exchange; emitted {} protocol bytes.",
            out.len()
        );
    }
    Ok(())
}

fn name_bytes(name: &OsStr) -> io::Result<Vec<u8>> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        Ok(name.as_bytes().to_vec())
    }
    #[cfg(not(unix))]
    {
        name.to_str()
            .map(|name| name.as_bytes().to_vec())
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "filename is not valid UTF-8")
            })
    }
}

fn file_uri(path: &Path) -> io::Result<String> {
    let metadata = fs::symlink_metadata(path)?;
    let url = if metadata.is_dir() {
        Url::from_directory_path(path)
    } else {
        Url::from_file_path(path)
    };
    url.map(String::from).map_err(|()| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "expected an absolute file path",
        )
    })
}

fn send_path(
    controller: &mut DragDrop,
    out: &mut impl Write,
    entry: Entry,
    path: &Path,
) -> Result<(), Error> {
    let mut pending = VecDeque::from([(entry, path.to_owned())]);
    while let Some((entry, path)) = pending.pop_front() {
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.is_symlink() {
            let target = fs::read_link(path)?;
            controller.send_symlink(out, entry, &name_bytes(target.as_os_str())?)?;
        } else if metadata.is_dir() {
            let mut children = fs::read_dir(path)?.collect::<Result<Vec<_>, _>>()?;
            children.sort_by_key(|child| child.file_name());
            let names = children
                .iter()
                .map(|child| name_bytes(&child.file_name()))
                .collect::<Result<Vec<_>, _>>()?;
            let names: Vec<_> = names.iter().map(Vec::as_slice).collect();
            let handle = controller.send_directory(out, entry, &names)?;
            for (index, child) in children.into_iter().enumerate() {
                pending.push_back((
                    Entry::DirectoryEntry {
                        handle,
                        index: (index + 1) as u32,
                    },
                    child.path(),
                ));
            }
        } else if metadata.is_file() {
            controller.send_from(out, entry, &mut fs::File::open(path)?)?;
        } else {
            return Err(Error::InvalidInput(
                "only regular files, directories, and symlinks are supported",
            ));
        }
    }
    Ok(())
}

fn run(
    program: &mut Program<Stdin, Stdout>,
    controller: &mut DragDrop,
    demo: &mut Files,
) -> Result<(), Box<dyn std::error::Error>> {
    program.init()?;
    program.enter_alt_screen()?;
    program.hide_cursor()?;
    program.query_capabilities(REQUEST_DRAG_DROP)?;
    let mut awaiting_capabilities = true;
    demo.log("Checking drag and drop support");
    let quit: [Key; 2] = ["q", "ctrl+c"].map(|key| key.parse().unwrap());
    loop {
        let screen = program.screen_mut();
        screen.clear();
        screen.set_str(
            (0, 0),
            "Files: drop here to inspect; drag source paths out. q quits.",
            Style::new(),
        );
        screen.set_str(
            (0, 1),
            &format!(
                "Next drag thumbnail: {}. 1 image / 2 text / 0 none",
                demo.thumbnail.label()
            ),
            Style::new(),
        );
        screen.set_str(
            (0, 2),
            &format!(
                "{} source paths; {} received bytes; {} listed entries",
                demo.roots.len(),
                demo.received,
                demo.listed
            ),
            Style::new(),
        );
        for (row, message) in demo.log.iter().enumerate() {
            screen.set_str((0, row as u16 + 4), message, Style::new());
        }
        screen.render()?;
        let event = program.read_event()?;
        match &event {
            TerminalEvent::KeyPress(key) if quit.contains(key) => return Ok(()),
            TerminalEvent::KeyPress(key) if demo.thumbnail.select(key) => {}
            TerminalEvent::Resize(_) => program.autoresize()?,
            TerminalEvent::PrimaryDeviceAttributes(_) if awaiting_capabilities => {
                if !program.capabilities().drag_drop() {
                    return Err(io::Error::new(
                        io::ErrorKind::Unsupported,
                        "OSC 72 support was not confirmed",
                    )
                    .into());
                }
                controller.enable_drops(program.screen_mut(), &["text/uri-list"])?;
                if !demo.roots.is_empty() {
                    controller.enable_drags(program.screen_mut())?;
                }
                awaiting_capabilities = false;
                demo.log("Ready for file drops; received data is not saved");
            }
            _ => {}
        }
        let events = controller.handle_event(&event)?;
        let screen = program.screen_mut();
        screen.flush()?;
        for event in events {
            demo.handle(controller, screen.writer_mut(), event)?;
        }
        screen.writer_mut().flush()?;
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let first = args.next();
    if let Some(flag) = &first {
        if flag == "--native" || flag == "--scripted" {
            if args.next().is_some() {
                return Err("replay modes do not accept source paths".into());
            }
            return replay(flag == "--native");
        }
        if flag == "--help" {
            println!(
                "drag_drop_files [PATH ...]\n  Drop files to inspect without saving. Supply paths to copy out.\n  1: image thumbnail; 2: text thumbnail; 0: none (next drag).\n  q or Ctrl-C: quit.\n  --scripted: in-memory demonstration\n  --native: native replay transport"
            );
            return Ok(());
        }
    }
    let roots = first
        .into_iter()
        .chain(args)
        .map(std::path::absolute)
        .collect::<Result<Vec<_>, _>>()?;
    let uris = roots
        .iter()
        .map(|path| file_uri(path))
        .collect::<Result<Vec<_>, _>>()?;
    let mut controller = DragDrop::new(DragDropOptions {
        machine_id: Some(MachineId::current()?),
        ..DragDropOptions::default()
    });
    let mut demo = Files {
        interactive: true,
        roots,
        uris,
        ..Files::default()
    };
    let mut program = Program::stdio()?;
    let result = run(&mut program, &mut controller, &mut demo);
    let cleanup = controller.close(program.screen_mut()).map_err(Into::into);
    let flush = program.screen_mut().flush().map_err(Into::into);
    let finish = program.finish().map_err(Into::into);
    result.and(cleanup).and(flush).and(finish)
}
