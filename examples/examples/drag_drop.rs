//! Bidirectional text, HTML, image, and file transfers in Kitty 0.47 or newer.
//!
//! Run `cargo run -p examples --example drag_drop -- --help` for usage.

#[path = "support/capability_query.rs"]
mod capability_query;
#[path = "support/drag_source.rs"]
mod drag_source;
#[path = "support/drop_receiver.rs"]
mod drop_receiver;
#[path = "support/drag_drop.rs"]
mod support;

use std::{
    collections::{HashMap, VecDeque},
    io::{self, Write},
    path::PathBuf,
    time::Duration,
};

use uncurses::{
    ansi::drag_drop::REQUEST_DRAG_DROP,
    buffer::SurfaceMut,
    event::{Event as TerminalEvent, Key},
    program::Program,
    style::Style,
    terminal::{Stdin, Stdout},
    text::TextSurface,
};
use uncurses_dragdrop::{
    DragDrop, DragDropOptions, DropOffer, Entry, Error, Event, MachineId, Operation, Operations,
    path_to_uri,
};

use drag_source::{Data, DirectoryPool, Source};
use drop_receiver::Receiver;

const MIMES: &[&str] = &[
    "text/uri-list",
    "text/plain",
    "text/plain;charset=utf-8",
    "UTF8_STRING",
    "image/png",
    "image/jpeg",
    "text/html",
];
const TEXT: &[u8] = b"Hello from uncurses!\n";
const HTML: &[u8] = b"<!doctype html><meta charset=\"utf-8\"><p>Hello from uncurses!</p>\n";
const PNG: &[u8] = include_bytes!("support/drag_drop.png");
const JPEG: &[u8] = include_bytes!("support/drag_drop.jpg");
const HELP: &str = "\
drag_drop [--receive-dir DIR] [--] [PATH ...]

Drop files, text, HTML, PNG, or JPEG onto this window.
Preview by default. --receive-dir saves each drop in a new directory.
Drag supplied paths or generated content out of the window.

f: files   t: text   h: HTML   p: PNG   j: JPEG
1: image thumbnail   2: text thumbnail   0: none
q or Ctrl-C: quit

Over SSH, run this program on the host that owns PATH.
The receive directory also belongs to that host.";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Content {
    Files,
    Text,
    Html,
    Png,
    Jpeg,
}

impl Content {
    fn mimes(self) -> &'static [&'static str] {
        match self {
            Self::Files => &["text/uri-list"],
            Self::Text => &["text/plain", "text/plain;charset=utf-8", "UTF8_STRING"],
            Self::Html => &["text/html", "text/plain"],
            Self::Png => &["image/png"],
            Self::Jpeg => &["image/jpeg"],
        }
    }

    fn body(self, mime: &str, uri_list: &[u8]) -> Result<Vec<u8>, Error> {
        if !self.mimes().contains(&mime) {
            return Err(Error::InvalidInput("requested MIME type was not offered"));
        }
        Ok(match self {
            Self::Files => uri_list.to_vec(),
            Self::Text => TEXT.repeat(512),
            Self::Html if mime == "text/plain" => TEXT.to_vec(),
            Self::Html => HTML.to_vec(),
            Self::Png => PNG.to_vec(),
            Self::Jpeg => JPEG.to_vec(),
        })
    }
}

fn copy_type(offer: &DropOffer) -> Option<&'static str> {
    if !offer.operations.contains(Operations::COPY) {
        return None;
    }
    MIMES
        .iter()
        .copied()
        .find(|mime| offer.mime_types.iter().any(|offered| offered == mime))
}

struct Demo {
    roots: Vec<PathBuf>,
    destination: Option<PathBuf>,
    selected: Content,
    active: Option<Content>,
    sources: HashMap<Entry, Source>,
    source_dirs: DirectoryPool,
    uri_list: Vec<u8>,
    receiver: Receiver,
    thumbnail: support::Thumbnail,
    log: VecDeque<String>,
}

impl Demo {
    fn new(roots: Vec<PathBuf>, destination: Option<PathBuf>) -> Self {
        Self {
            selected: if roots.is_empty() {
                Content::Text
            } else {
                Content::Files
            },
            roots,
            destination,
            active: None,
            sources: HashMap::new(),
            source_dirs: DirectoryPool::default(),
            uri_list: Vec::new(),
            receiver: Receiver::default(),
            thumbnail: support::Thumbnail::default(),
            log: VecDeque::new(),
        }
    }

    fn log(&mut self, message: impl Into<String>) {
        self.log.push_back(message.into());
        if self.log.len() > 8 {
            self.log.pop_front();
        }
    }

    fn handle_event(
        &mut self,
        controller: &mut DragDrop,
        out: &mut impl Write,
        event: &TerminalEvent,
    ) -> Result<(), Error> {
        let events = match controller.handle_event(event) {
            Ok(events) => events,
            Err(error @ (Error::LimitExceeded(_) | Error::Protocol(_))) => {
                let message = error.to_string();
                self.cancel_drop(controller, out, &message)?;
                return self.cancel_drag(controller, out, &message);
            }
            Err(error) => return Err(error),
        };
        for event in events {
            self.event(controller, out, event)?;
        }
        Ok(())
    }

    fn key(&mut self, key: &Key) {
        if self.thumbnail.select(key) {
            return;
        }
        for (binding, content) in [
            ("f", Content::Files),
            ("t", Content::Text),
            ("h", Content::Html),
            ("p", Content::Png),
            ("j", Content::Jpeg),
        ] {
            if *key == binding.parse().unwrap() {
                if content == Content::Files && self.roots.is_empty() {
                    self.log("Supply source paths to offer files");
                } else {
                    self.selected = content;
                }
                return;
            }
        }
    }

    fn event(
        &mut self,
        controller: &mut DragDrop,
        out: &mut impl Write,
        event: Event,
    ) -> Result<(), Error> {
        match event {
            Event::Hover(offer) => {
                if let Some(mime) = copy_type(&offer) {
                    controller.accept(out, &[mime], Operation::Copy)?;
                } else {
                    controller.reject(out)?;
                    self.log("Drop rejected: no supported copy format");
                }
            }
            Event::Drop(offer) => {
                let Some(mime) = copy_type(&offer) else {
                    controller.cancel_drop(out)?;
                    self.log("Drop canceled: no supported copy format");
                    return Ok(());
                };
                if let Err(error) = self.receiver.begin(mime, self.destination.as_deref()) {
                    return self.cancel_drop(controller, out, &error.to_string());
                }
                self.log(format!("Receiving {mime}"));
                if mime == "text/uri-list" {
                    controller.list(out)?;
                } else {
                    controller.request(out, mime)?;
                }
            }
            Event::UriList { remote } => self.log(format!("URI list: remote={remote}")),
            Event::Listed { .. } | Event::Entry(_) | Event::Data(_) | Event::DataEnd
                if !self.receiver.active => {}
            Event::Listed { entry, name } => {
                let preview: String = name
                    .iter()
                    .flat_map(|byte| byte.escape_ascii())
                    .map(char::from)
                    .collect();
                self.log(format!("Listed {preview}"));
                match self.receiver.listed(entry, name, controller.options()) {
                    Ok(true) => {}
                    Ok(false) => self.log("Skipped URI: no safe file entry"),
                    Err(error) => {
                        self.cancel_drop(controller, out, &error.to_string())?;
                    }
                }
            }
            Event::Entry(kind) => {
                if let Err(error) = self.receiver.kind(kind) {
                    return self.cancel_drop(controller, out, &error.to_string());
                }
            }
            Event::Data(bytes) => {
                if let Err(error) = self.receiver.data(&bytes) {
                    return self.cancel_drop(controller, out, &error.to_string());
                }
            }
            Event::DataEnd => {
                if let Err(error) = self.receiver.end_body() {
                    return self.cancel_drop(controller, out, &error.to_string());
                }
                for directory in self.receiver.closed.drain(..) {
                    controller.close_directory(out, directory)?;
                }
                if let Some(entry) = self.receiver.next() {
                    controller.request(out, entry)?;
                } else {
                    controller.complete_drop(out, Operation::Copy)?;
                    out.flush()?;
                    let saved = self.receiver.finish();
                    self.log(format!(
                        "Drop complete: {} bytes, {} entries, checksum={:016x}",
                        self.receiver.bytes, self.receiver.listed, self.receiver.checksum,
                    ));
                    self.log(format!("{} bytes; copy complete", self.receiver.bytes));
                    if let Some(path) = saved {
                        self.log(format!("Saved {path:?}"));
                    }
                }
            }
            Event::DropCanceled => {
                self.receiver.abort()?;
                self.log("Drop canceled by a new drag");
            }
            Event::DropFailed(error) => {
                self.receiver.abort()?;
                self.log(format!("Drop failed: {error}"));
            }
            Event::DragRequested(_) => {
                let content = self.selected;
                if content == Content::Files {
                    self.source_dirs = DirectoryPool::new(controller.options().max_directories);
                    let sources = self
                        .roots
                        .iter()
                        .map(|path| {
                            Ok((
                                Source::new(path, &mut self.source_dirs)?,
                                path_to_uri(path)?,
                            ))
                        })
                        .collect::<io::Result<Vec<_>>>();
                    let sources = match sources {
                        Ok(sources) => sources,
                        Err(error) => {
                            return self.cancel_drag(controller, out, &error.to_string());
                        }
                    };
                    let (sources, uris): (Vec<_>, Vec<_>) = sources.into_iter().unzip();
                    let uris: Vec<_> = uris.iter().map(String::as_str).collect();
                    let entries = controller.offer_files(out, &uris, Operations::COPY)?;
                    self.uri_list = uris.join("\r\n").into_bytes();
                    self.sources = entries.into_iter().zip(sources).collect();
                } else {
                    controller.offer(out, content.mimes(), Operations::COPY)?;
                    for mime in content.mimes() {
                        controller.send(out, *mime, &content.body(mime, &self.uri_list)?)?;
                    }
                }
                self.active = Some(content);
                self.thumbnail.upload(controller, out)?;
                controller.start_drag(out)?;
            }
            Event::DragStarted => self.log("Drag started"),
            Event::DataRequested { mime_type } => {
                let content = self.active.ok_or(Error::InvalidState("no active offer"))?;
                controller.send(out, &mime_type, &content.body(&mime_type, &self.uri_list)?)?;
            }
            Event::DragFinished { canceled } => {
                self.clear_drag();
                self.log(format!("Drag finished: canceled={canceled}"));
            }
            Event::DragFailed(error) => {
                self.clear_drag();
                self.log(format!("Drag failed: {error}"));
            }
            _ => {}
        }
        Ok(())
    }

    fn send_next(&mut self, controller: &mut DragDrop, out: &mut impl Write) -> Result<(), Error> {
        let Some(entry) = controller.next_send()? else {
            return Ok(());
        };
        let source = self
            .sources
            .get(&entry)
            .ok_or(Error::InvalidInput("unknown source entry"))?
            .clone();
        let data = match source.read(&mut self.source_dirs) {
            Ok(data) => data,
            Err(error) => {
                return self.cancel_drag(controller, out, &error.to_string());
            }
        };
        let result = match data {
            Data::Symlink(target) => controller.send_symlink(out, &entry, target),
            Data::Directory(children) => controller
                .send_directory(out, &entry, children.iter().map(|child| &child.name))
                .map(|entries| self.sources.extend(entries.into_iter().zip(children))),
            Data::File(mut file) => controller.send_from(out, &entry, &mut file),
        };
        if let Err(error) = result {
            return self.source_error(controller, out, error);
        }
        if matches!(&entry, Entry::File(file) if file.root_index().is_none()) {
            self.sources.remove(&entry);
        }
        self.log(format!("Sent {:?}", source.name));
        Ok(())
    }

    fn source_error(
        &mut self,
        controller: &mut DragDrop,
        out: &mut impl Write,
        error: Error,
    ) -> Result<(), Error> {
        if !matches!(error, Error::Io(_)) {
            return Err(error);
        }
        // A terminal write failure poisons the controller, so cancellation
        // returns OutputFailed without another write. Preserve the I/O error.
        match self.cancel_drag(controller, out, &error.to_string()) {
            Err(Error::OutputFailed) => Err(error),
            result => result,
        }
    }

    fn cancel_drop(
        &mut self,
        controller: &mut DragDrop,
        out: &mut impl Write,
        message: &str,
    ) -> Result<(), Error> {
        controller.cancel_drop(out)?;
        self.receiver.abort()?;
        self.log(format!("Drop canceled: {message}"));
        Ok(())
    }

    fn clear_drag(&mut self) {
        self.sources.clear();
        self.source_dirs = DirectoryPool::default();
        self.uri_list.clear();
        self.active = None;
    }

    fn cancel_drag(
        &mut self,
        controller: &mut DragDrop,
        out: &mut impl Write,
        message: &str,
    ) -> Result<(), Error> {
        controller.cancel_drag(out)?;
        self.clear_drag();
        self.log(format!("Drag canceled: {message}"));
        Ok(())
    }
}

fn run(
    program: &mut Program<Stdin, Stdout>,
    controller: &mut DragDrop,
    demo: &mut Demo,
) -> Result<(), Box<dyn std::error::Error>> {
    program.init()?;
    program.enter_alt_screen()?;
    program.hide_cursor()?;
    program.query_capabilities(REQUEST_DRAG_DROP)?;
    let mut query = capability_query::CapabilityQuery::new();
    let mut awaiting_capabilities = true;
    demo.log("Checking drag and drop support");
    let quit: [Key; 2] = ["q", "ctrl+c"].map(|key| key.parse().unwrap());
    loop {
        let screen = program.screen_mut();
        screen.clear();
        screen.set_str(
            (0, 0),
            "Drop content here or drag it out. q quits.",
            Style::new(),
        );
        screen.set_str(
            (0, 1),
            &format!(
                "Next drag: {}. f files / t text / h HTML / p PNG / j JPEG",
                demo.selected.mimes()[0]
            ),
            Style::new(),
        );
        screen.set_str(
            (0, 2),
            &format!(
                "Thumbnail: {}. 1 image / 2 text / 0 none",
                demo.thumbnail.label()
            ),
            Style::new(),
        );
        let destination = demo.destination.as_deref().map_or_else(
            || "Preview only".into(),
            |path| format!("Save each drop under {path:?}"),
        );
        screen.set_str((0, 3), &destination, Style::new());
        screen.set_str((0, 5), &demo.receiver.preview, Style::new());
        for (row, message) in demo.log.iter().enumerate() {
            screen.set_str((0, row as u16 + 7), message, Style::new());
        }
        screen.render()?;
        if controller.next_send()?.is_some() && !program.poll_event(Some(Duration::ZERO))? {
            let screen = program.screen_mut();
            screen.flush()?;
            demo.send_next(controller, screen.writer_mut())?;
            screen.writer_mut().flush()?;
            continue;
        }
        let Some(event) = query.read_event(program)? else {
            return Ok(());
        };
        match &event {
            TerminalEvent::KeyPress(key) if quit.contains(key) => {
                if query.request_quit() {
                    return Ok(());
                }
            }
            TerminalEvent::KeyPress(key) => demo.key(key),
            TerminalEvent::Resize(_) => program.autoresize()?,
            TerminalEvent::PrimaryDeviceAttributes(_) if awaiting_capabilities => {
                if !program.capabilities().drag_drop() {
                    return Err(io::Error::new(
                        io::ErrorKind::Unsupported,
                        "OSC 72 support was not confirmed",
                    )
                    .into());
                }
                controller.enable_drops(program.screen_mut(), MIMES)?;
                controller.enable_drags(program.screen_mut())?;
                awaiting_capabilities = false;
                demo.log("Ready for text, images, HTML, and file drops");
            }
            _ => {}
        }
        let screen = program.screen_mut();
        screen.flush()?;
        demo.handle_event(controller, screen.writer_mut(), &event)?;
        screen.writer_mut().flush()?;
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let mut roots = Vec::new();
    let mut destination = None;
    let mut options = true;
    while let Some(arg) = args.next() {
        if options && arg == "--help" {
            println!("{HELP}");
            return Ok(());
        } else if options && arg == "--" {
            options = false;
        } else if options && arg == "--receive-dir" {
            destination = Some(std::path::absolute(
                args.next().ok_or("--receive-dir requires a directory")?,
            )?);
        } else if options && arg.as_encoded_bytes().starts_with(b"-") {
            return Err(format!("unknown option: {arg:?}").into());
        } else {
            roots.push(PathBuf::from(arg));
        }
    }
    let mut demo = Demo::new(roots, destination);
    let mut controller = DragDrop::new(DragDropOptions {
        machine_id: Some(MachineId::current()?),
        ..DragDropOptions::default()
    });
    let mut program = Program::stdio()?;
    let result = run(&mut program, &mut controller, &mut demo);
    let partial = demo.receiver.abort().map_err(Into::into);
    let cleanup = controller.close(program.screen_mut()).map_err(Into::into);
    let flush = program.screen_mut().flush().map_err(Into::into);
    let finish = program.finish().map_err(Into::into);
    result.and(partial).and(cleanup).and(flush).and(finish)
}

#[cfg(test)]
#[path = "support/drag_drop_tests.rs"]
mod tests;
