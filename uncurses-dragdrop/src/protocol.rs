use base64::{
    Engine,
    engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig},
};

use crate::{Error, Operations, Position, RemoteError};

pub(crate) const BASE64: GeneralPurpose = GeneralPurpose::new(
    &base64::alphabet::STANDARD,
    GeneralPurposeConfig::new()
        .with_encode_padding(false)
        .with_decode_padding_mode(DecodePaddingMode::Indifferent),
);

#[derive(Debug, Default)]
pub(crate) struct Message {
    pub metadata: Vec<(String, String)>,
    pub payload: Vec<u8>,
    pub discarded: bool,
}

impl Message {
    pub fn value(&self, key: &str) -> Option<&str> {
        value(&self.metadata, key)
    }

    pub fn number(&self, key: &str) -> Result<i64, Error> {
        let text = self.value(key).unwrap_or("0");
        text.parse::<i64>()
            .ok()
            .filter(|n| (i64::from(i32::MIN)..=i64::from(u32::MAX)).contains(n))
            .ok_or_else(|| Error::Protocol(format!("invalid {key} value")))
    }

    pub fn unsigned(&self, key: &str) -> Result<u32, Error> {
        u32::try_from(self.number(key)?)
            .map_err(|_| Error::Protocol(format!("negative {key} value")))
    }

    pub fn signed(&self, key: &str) -> Result<i32, Error> {
        i32::try_from(self.number(key)?)
            .map_err(|_| Error::Protocol(format!("invalid signed {key} value")))
    }

    pub fn position(&self) -> Result<Position, Error> {
        Ok(Position {
            column: self.unsigned("x")?,
            row: self.unsigned("y")?,
            pixel_x: self.signed("X")?,
            pixel_y: self.signed("Y")?,
        })
    }

    pub fn operations(&self) -> Result<Operations, Error> {
        u8::try_from(self.unsigned("o")?)
            .ok()
            .and_then(Operations::from_bits)
            .ok_or_else(|| Error::Protocol("invalid operation flags".into()))
    }

    pub fn remote_error(&self) -> Result<RemoteError, Error> {
        let text = std::str::from_utf8(&self.payload)
            .map_err(|_| Error::Protocol("error text is not UTF-8".into()))?;
        let (code, description) = text
            .split_once(':')
            .map_or((text, None), |(code, description)| {
                (code, Some(description))
            });
        let error = RemoteError {
            code: code.into(),
            description: description.map(str::to_owned),
        };
        error_payload(&error).map_err(|_| Error::Protocol("invalid remote error".into()))?;
        Ok(error)
    }

    pub fn decode_data(&mut self, final_chunk: bool) -> Result<Vec<u8>, Error> {
        // Keep the last quartet until message completion to validate padding
        // and trailing bits without treating a chunk boundary as base64 EOF.
        let count = if final_chunk {
            self.payload.len()
        } else {
            self.payload.len().saturating_sub(1) / 4 * 4
        };
        if !final_chunk && self.payload[..count].contains(&b'=') {
            return Err(Error::Protocol("base64 padding before message end".into()));
        }
        let decoded = BASE64
            .decode(&self.payload[..count])
            .map_err(|error| Error::Protocol(error.to_string()))?;
        self.payload.drain(..count);
        Ok(decoded)
    }
}

pub(crate) fn value<'a>(metadata: &'a [(String, String)], key: &str) -> Option<&'a str> {
    metadata
        .iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.as_str())
}

pub(crate) fn mime_list(payload: &[u8]) -> Result<Vec<String>, Error> {
    let text = std::str::from_utf8(payload)
        .map_err(|_| Error::Protocol("MIME list is not UTF-8".into()))?;
    let mimes: Vec<_> = text.split_ascii_whitespace().map(str::to_owned).collect();
    if mimes.is_empty() || mimes.iter().any(|mime| !valid_mime(mime)) {
        return Err(Error::Protocol("invalid MIME list".into()));
    }
    Ok(mimes)
}

pub(crate) fn valid_mime(mime: &str) -> bool {
    !mime.is_empty() && mime.bytes().all(|byte| (0x21..=0x7e).contains(&byte))
}

pub(crate) fn join_mimes(mimes: &[&str], allow_empty: bool) -> Result<String, Error> {
    if (!allow_empty && mimes.is_empty()) || mimes.iter().any(|mime| !valid_mime(mime)) {
        return Err(Error::InvalidInput("expected escape-safe MIME types"));
    }
    if mimes
        .iter()
        .enumerate()
        .any(|(index, mime)| mimes[..index].contains(mime))
    {
        return Err(Error::InvalidInput("duplicate MIME type"));
    }
    Ok(mimes.join(" "))
}

pub(crate) fn error_payload(error: &RemoteError) -> Result<String, Error> {
    if error.code.is_empty()
        || !error
            .code
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
        || error
            .description
            .as_ref()
            .is_some_and(|text| text.chars().any(char::is_control))
    {
        return Err(Error::InvalidInput("expected an escape-safe POSIX error"));
    }
    Ok(match &error.description {
        Some(description) => format!("{}:{description}", error.code),
        None => error.code.clone(),
    })
}
