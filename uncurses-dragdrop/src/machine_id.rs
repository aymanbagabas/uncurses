use std::{fmt::Write, io};

use hmac::{Hmac, Mac};
use sha2::Sha256;

pub(crate) fn current() -> io::Result<String> {
    hash(&platform_id()?)
}

fn hash(id: &str) -> io::Result<String> {
    if id.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "the platform machine ID is empty",
        ));
    }
    let mut mac = Hmac::<Sha256>::new_from_slice(b"tty-dnd-protocol-machine-id")
        .expect("HMAC accepts keys of any length");
    mac.update(id.as_bytes());
    let mut result = String::with_capacity(66);
    result.push_str("1:");
    for byte in mac.finalize().into_bytes() {
        write!(result, "{byte:02x}").expect("writing to a String cannot fail");
    }
    Ok(result)
}

#[cfg(target_os = "macos")]
fn platform_id() -> io::Result<String> {
    let mut uuid = [0u8; 16];
    let wait = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // gethostuuid exposes IOPlatformUUID as bytes. Preserve the uppercase
    // spelling of IOPlatformUUID because the protocol hashes its text.
    // SAFETY: uuid holds 16 writable bytes and wait is a valid timespec.
    if unsafe { libc::gethostuuid(uuid.as_mut_ptr(), &wait) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(format!(
        "{:02X}{:02X}{:02X}{:02X}-{:02X}{:02X}-{:02X}{:02X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}",
        uuid[0],
        uuid[1],
        uuid[2],
        uuid[3],
        uuid[4],
        uuid[5],
        uuid[6],
        uuid[7],
        uuid[8],
        uuid[9],
        uuid[10],
        uuid[11],
        uuid[12],
        uuid[13],
        uuid[14],
        uuid[15],
    ))
}

#[cfg(windows)]
fn platform_id() -> io::Result<String> {
    use windows_sys::Win32::System::Registry::KEY_WOW64_64KEY;

    windows_registry::LOCAL_MACHINE
        .options()
        .read()
        .access(KEY_WOW64_64KEY)
        .open(r"SOFTWARE\Microsoft\Cryptography")
        .and_then(|key| key.get_string("MachineGuid"))
        .map_err(io::Error::other)
}

#[cfg(not(any(target_os = "macos", windows)))]
fn platform_id() -> io::Result<String> {
    std::fs::read_to_string("/etc/machine-id").map(|id| id.trim_end().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_hash_matches_independent_vectors() {
        for (id, expected) in [
            (
                "0123456789abcdef0123456789abcdef",
                "1:a81b6c3c9d37b0caa4a9c6b7de41059238bc101829db2975223b93d39d663b08",
            ),
            (
                "00112233-4455-6677-8899-AABBCCDDEEFF",
                "1:8f671e71176f7abccd313205bb6f665cf1ec42fac00359b251f35c934bedb3ba",
            ),
            (
                " 0123456789abcdef0123456789abcdef",
                "1:37f0c09b1918c38f871cdfac941cd01b71764b22951e0e812d3494b860180b3f",
            ),
        ] {
            assert_eq!(hash(id).unwrap(), expected);
        }
    }

    #[test]
    fn empty_identity_is_an_error() {
        assert_eq!(hash("").unwrap_err().kind(), io::ErrorKind::InvalidData);
    }
}
