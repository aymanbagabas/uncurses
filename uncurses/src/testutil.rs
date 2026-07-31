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
