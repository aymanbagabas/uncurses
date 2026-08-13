//! Test-only helpers shared across modules.

/// Open a pty master/slave pair. The master must stay open for the slave
/// to remain usable, and Darwin rejects `TIOCSWINSZ` on a master that has
/// no slave, so both halves are returned and both must be held.
///
/// A pty master is a tty in every environment, including CI with captured
/// output — unlike stderr, which makes tests skip. Returns `None` where
/// the platform or sandbox has no usable pty; callers may skip on that,
/// but nothing past it.
///
/// The slave is probed with `tcgetattr` before being handed back. illumos
/// and Solaris ptys are STREAMS devices that only behave as terminals once
/// `ptem` and `ldterm` are pushed onto them, so a bare slave there answers
/// `EINVAL`. Failing the probe here keeps the skip at the "is there a
/// usable pty" boundary rather than letting a later assertion decide.
///
/// `ptsname` returns a pointer into static storage, so a concurrent call
/// from another test thread can clobber the name between reading it and
/// opening the slave. `ptsname_r` is not portable (glibc spells it, and
/// the `libc` bindings do not offer it everywhere), so serialize the
/// whole sequence instead — this is a test helper, contention is free.
pub(crate) fn open_pty_pair() -> Option<(std::fs::File, std::fs::File)> {
    use std::fs::File;
    use std::os::fd::FromRawFd;
    use std::sync::Mutex;

    static PTSNAME: Mutex<()> = Mutex::new(());
    let _guard = PTSNAME.lock().unwrap_or_else(|e| e.into_inner());

    unsafe {
        let master = libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY);
        if master < 0 {
            return None;
        }
        if libc::grantpt(master) != 0 || libc::unlockpt(master) != 0 {
            libc::close(master);
            return None;
        }
        let name = libc::ptsname(master);
        if name.is_null() {
            libc::close(master);
            return None;
        }
        let slave = libc::open(name, libc::O_RDWR | libc::O_NOCTTY);
        if slave < 0 {
            libc::close(master);
            return None;
        }
        let mut probe: libc::termios = std::mem::zeroed();
        if libc::tcgetattr(slave, &mut probe) != 0 {
            libc::close(slave);
            libc::close(master);
            return None;
        }
        Some((File::from_raw_fd(master), File::from_raw_fd(slave)))
    }
}

/// Read whatever a descriptor has buffered right now, without blocking.
///
/// A pty master holds what was written to its slave, so this is how a test
/// observes the bytes a `Screen` actually put on the wire. The descriptor is
/// switched to non-blocking first: there is no sentinel to read up to, so a
/// blocking read would sit there once the buffered output ran out.
pub(crate) fn drain(f: &dyn std::os::fd::AsFd) -> Vec<u8> {
    use std::os::fd::AsRawFd;

    let fd = f.as_fd().as_raw_fd();
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFL);
        if flags < 0 || libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) < 0 {
            return Vec::new();
        }
    }
    let mut out = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let n = unsafe { libc::read(fd, chunk.as_mut_ptr().cast(), chunk.len()) };
        if n <= 0 {
            return out;
        }
        out.extend_from_slice(&chunk[..n as usize]);
    }
}

/// An [`AsFd`](std::os::fd::AsFd) that hands out a different descriptor on each
/// borrow, so a descriptor can be readable by `tcgetattr` and then rejected by
/// a later `tcsetattr`.
///
/// Raw mode is a read-then-write sequence, and its failure handling only
/// matters when the write fails on a descriptor the read accepted -- a pty
/// whose master went away mid-session, say. Nothing else can produce that
/// ordering on demand, so the borrows are scripted instead: the descriptor for
/// borrow `n` is `fds[n]`, and the last entry repeats once the script runs out.
///
/// The borrows are stored as [`BorrowedFd`](std::os::fd::BorrowedFd), so the
/// compiler keeps them tied to the descriptors they came from.
pub(crate) struct ScriptedFd<'a> {
    fds: Vec<std::os::fd::BorrowedFd<'a>>,
    borrows: std::cell::Cell<usize>,
}

impl<'a> ScriptedFd<'a> {
    /// # Panics
    ///
    /// Panics if `fds` is empty: a script with no descriptors has nothing to
    /// hand out, and every borrow would be a bug.
    pub(crate) fn new(fds: &[&'a dyn std::os::fd::AsFd]) -> Self {
        assert!(!fds.is_empty(), "a scripted descriptor needs a script");
        Self {
            fds: fds.iter().map(|f| f.as_fd()).collect(),
            borrows: std::cell::Cell::new(0),
        }
    }
}

impl std::os::fd::AsFd for ScriptedFd<'_> {
    fn as_fd(&self) -> std::os::fd::BorrowedFd<'_> {
        let n = self.borrows.get();
        self.borrows.set(n + 1);
        self.fds[n.min(self.fds.len() - 1)]
    }
}

/// Read a descriptor's terminal attributes directly, so assertions observe the
/// device rather than the code under test.
pub(crate) fn attrs(f: &dyn std::os::fd::AsFd) -> libc::termios {
    use std::os::fd::AsRawFd;
    let mut t: libc::termios = unsafe { std::mem::zeroed() };
    assert_eq!(
        unsafe { libc::tcgetattr(f.as_fd().as_raw_fd(), &mut t) },
        0,
        "tcgetattr failed: {}",
        std::io::Error::last_os_error()
    );
    t
}

/// Whether the descriptor post-processes output.
pub(crate) fn opost(f: &dyn std::os::fd::AsFd) -> bool {
    attrs(f).c_oflag & libc::OPOST != 0
}

/// Force `OPOST` to a known value so assertions do not depend on whatever a
/// fresh pty happens to default to -- POSIX leaves the initial attributes
/// implementation-defined -- and stamp `VMIN`/`VTIME` with values raw mode does
/// not use. A pty commonly defaults to `VMIN` 1, which is also raw mode's
/// value, so restoring it would otherwise be unobservable.
pub(crate) fn prime(f: &dyn std::os::fd::AsFd, opost: bool) {
    use std::os::fd::AsRawFd;
    let mut t = attrs(f);
    if opost {
        t.c_oflag |= libc::OPOST;
    } else {
        t.c_oflag &= !libc::OPOST;
    }
    t.c_cc[libc::VMIN] = 4;
    t.c_cc[libc::VTIME] = 7;
    assert_eq!(
        unsafe { libc::tcsetattr(f.as_fd().as_raw_fd(), libc::TCSANOW, &t) },
        0,
        "tcsetattr failed: {}",
        std::io::Error::last_os_error()
    );
}
