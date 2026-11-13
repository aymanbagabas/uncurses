use super::*;
use base64::Engine;
use protocol::BASE64;

pub(crate) struct Separated<'a, T> {
    parts: &'a [T],
    separator: &'a [u8],
    part: usize,
    offset: usize,
}

impl<'a, T> Separated<'a, T> {
    pub(crate) fn new(parts: &'a [T], separator: &'a [u8]) -> Self {
        Self {
            parts,
            separator,
            part: 0,
            offset: 0,
        }
    }
}

impl<T: AsRef<[u8]>> Read for Separated<'_, T> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let mut used = 0;
        while used < buffer.len() && self.part < self.parts.len() {
            let part = self.parts[self.part].as_ref();
            let bytes = if self.offset < part.len() {
                &part[self.offset..]
            } else if self.part + 1 < self.parts.len() {
                &self.separator[self.offset - part.len()..]
            } else {
                &[]
            };
            if bytes.is_empty() {
                self.part += 1;
                self.offset = 0;
            } else {
                let count = bytes.len().min(buffer.len() - used);
                buffer[used..used + count].copy_from_slice(&bytes[..count]);
                self.offset += count;
                used += count;
            }
        }
        Ok(used)
    }
}

/// Count URI records without retaining their names or content.
#[derive(Debug, Default)]
pub(crate) struct UriCounter {
    count: u32,
    started: bool,
    comment: bool,
    significant: bool,
}

impl UriCounter {
    pub(crate) fn feed(&mut self, bytes: &[u8]) -> Result<(), Error> {
        for &byte in bytes {
            match byte {
                0 => return Err(Error::InvalidInput("NUL in URI list")),
                b'\r' | b'\n' => self.finish_line()?,
                _ => {
                    if !self.started {
                        self.comment = byte == b'#';
                        self.started = true;
                    }
                    self.significant |= byte != b' ' && byte != b'\t';
                }
            }
        }
        Ok(())
    }

    fn finish_line(&mut self) -> Result<(), Error> {
        if self.significant && !self.comment {
            self.count = self
                .count
                .checked_add(1)
                .ok_or(Error::InvalidInput("too many URI entries"))?;
        }
        self.started = false;
        self.comment = false;
        self.significant = false;
        Ok(())
    }

    pub(crate) fn finish(&mut self) -> Result<u32, Error> {
        self.finish_line()?;
        Ok(self.count)
    }
}

pub(crate) enum BodyCheck {
    Any,
    Uris(UriCounter),
    Image(Option<u64>),
    Text(Vec<u8>),
}

impl BodyCheck {
    fn feed(&mut self, bytes: &[u8]) -> Result<(), Error> {
        match self {
            Self::Uris(counter) => counter.feed(bytes),
            Self::Image(Some(remaining)) => {
                *remaining =
                    remaining
                        .checked_sub(bytes.len() as u64)
                        .ok_or(Error::InvalidInput(
                            "thumbnail byte count differs from dimensions",
                        ))?;
                Ok(())
            }
            Self::Text(tail) => {
                // Retain at most one incomplete UTF-8 character between reads.
                tail.extend_from_slice(bytes);
                match std::str::from_utf8(tail) {
                    Ok(_) => tail.clear(),
                    Err(error) if error.error_len().is_none() => {
                        tail.drain(..error.valid_up_to());
                    }
                    Err(_) => return Err(Error::InvalidInput("thumbnail text is not UTF-8")),
                }
                Ok(())
            }
            Self::Any | Self::Image(None) => Ok(()),
        }
    }

    fn finish(&mut self) -> Result<Option<u32>, Error> {
        match self {
            Self::Uris(counter) => counter.finish().map(Some),
            Self::Image(Some(0) | None) | Self::Any => Ok(None),
            Self::Image(Some(_)) => Err(Error::InvalidInput(
                "thumbnail byte count differs from dimensions",
            )),
            Self::Text(tail) if tail.is_empty() => Ok(None),
            Self::Text(_) => Err(Error::InvalidInput("thumbnail text is not UTF-8")),
        }
    }
}

impl DragDrop {
    pub(crate) fn ready_for_body(&self) -> Result<(), Error> {
        self.check_output()?;
        if self.drag.failed_body.is_some() {
            return Err(Error::InvalidState("report or cancel the failed data body"));
        }
        Ok(())
    }

    pub(crate) fn write_body(
        &mut self,
        out: &mut impl Write,
        metadata: &[&str],
        source: &mut impl Read,
        mut check: BodyCheck,
        failure: FailedBody,
    ) -> Result<Option<u32>, Error> {
        let mut incomplete = false;
        let result = self.stream_body(out, metadata, source, &mut check, &mut incomplete);
        if result.is_err() {
            self.drag.failed_body = Some(failure);
            // A thumbnail continuation would consume cancellation as image
            // data. Preserve the unfinished body instead of falsely ending it.
            self.output_failed |= incomplete;
        }
        result
    }

    fn stream_body(
        &mut self,
        out: &mut impl Write,
        metadata: &[&str],
        source: &mut impl Read,
        check: &mut BodyCheck,
        incomplete: &mut bool,
    ) -> Result<Option<u32>, Error> {
        let thumbnail = matches!(check, BodyCheck::Image(_) | BodyCheck::Text(_));
        let mut chunk_metadata = metadata.to_vec();
        if thumbnail {
            chunk_metadata.push("m=1");
        }
        let mut bytes = [0; 3072];
        let mut used = 0;
        loop {
            let count = match source.read(&mut bytes[used..]) {
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                result => result?,
            };
            used += count;
            if used == bytes.len() || count == 0 {
                if used != 0 {
                    check.feed(&bytes[..used])?;
                    let encoded = BASE64.encode(&bytes[..used]);
                    // MIME and file blocks are independent messages. Images
                    // use one chunk chain so m=0 cannot finalize them early.
                    self.write(out, &chunk_metadata, encoded.as_bytes())?;
                    *incomplete = thumbnail;
                    used = 0;
                }
                if count == 0 {
                    break;
                }
            }
        }
        let count = check.finish()?;
        self.write(out, metadata, b"")?;
        *incomplete = false;
        Ok(count)
    }
}
