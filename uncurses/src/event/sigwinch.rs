//! SIGWINCH fan-out for Unix window-resize notifications.
//!
//! ## Purpose
//!
//! Unix terminals report window-size changes out-of-band through `SIGWINCH`.
//! This module installs one shared signal handler and lets each event source
//! register a pipe fd that receives a one-byte wake whenever the signal fires.
//!
//! ## Key types
//!
//! * [`subscribe`] leases a slot and its pipe, returning a [`Subscription`].
//! * [`Subscription`] releases the slot on drop; the pipe returns to the pool
//!   and is never closed.
//!
//! ## Gotchas
//!
//! The signal handler only performs async-signal-safe work: relaxed atomic loads
//! and `write(2)` to already-open fds. It does not query terminal size; the
//! event source does that later on the normal thread after the pipe becomes
//! readable.
//!
//! A signal disposition is process-global — the kernel has no per-thread or
//! per-object handler — so this module installs exactly one handler no matter
//! how many sources exist, and fans out to them through [`SUBSCRIBERS`]. To
//! avoid stealing `SIGWINCH` from whoever owned it first (another library, or
//! a C/Go/Python host embedding this one), the handler we displace is recorded
//! and forwarded to on every signal. With no subscribers left the handler does
//! nothing but forward, so it is never uninstalled — restoring the previous
//! disposition would also mean publishing `PREV_HANDLER` more than once, which
//! is precisely what makes the `{address, arity}` pair tearable (see
//! [`remember`]).
//!
//! ## Requirement when embedding this crate in a host process
//!
//! Because the disposition is never restored, the code backing it must stay
//! mapped for the life of the process. A host that loads uncurses as a shared
//! object must not `dlclose` it: the kernel would still hold `&handler` as the
//! `SIGWINCH` disposition and the next resize would jump into unmapped memory.
//! Link with `-Wl,-z,nodelete` or open with `RTLD_NODELETE`. The same applies
//! to whoever owned `SIGWINCH` before us — a recorded predecessor belonging to
//! an unloaded object is dangling through no fault of ours.
//!
//! For the same reason a host that wants its own handler to run *after* ours
//! must install it before the first [`subscribe`] call; a handler installed
//! afterwards displaces ours and is responsible for its own chaining.
#[cfg(unix)]
use std::os::fd::{AsRawFd, OwnedFd};
#[cfg(unix)]
use std::sync::OnceLock;
#[cfg(unix)]
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicUsize, Ordering};

#[cfg(unix)]
static INSTALLED: std::sync::Once = std::sync::Once::new();

/// The `SIGWINCH` disposition in place before [`install_handler`] replaced it,
/// so [`chain`] can forward to it. `SIG_DFL` and `SIG_IGN` both mean "nothing
/// to call" — the default action for `SIGWINCH` is to ignore it.
#[cfg(unix)]
static PREV_HANDLER: AtomicUsize = AtomicUsize::new(libc::SIG_DFL);

/// Whether [`PREV_HANDLER`] expects the three-argument `SA_SIGINFO` form
/// rather than the one-argument form.
#[cfg(unix)]
static PREV_SIGINFO: AtomicBool = AtomicBool::new(false);

/// Whether [`PREV_HANDLER`] was installed `SA_RESETHAND`, i.e. asked to run
/// exactly once. Published alongside the address; see [`remember`].
///
/// Only as good as the platform's reporting: Linux returns the flag from a
/// query, Darwin does not (there it is set-only, so `sigaction` hands back
/// `SA_RESTART` alone). Where it cannot be observed a one-shot predecessor is
/// forwarded to on every signal, which is the status quo, not a regression.
#[cfg(unix)]
static PREV_ONESHOT: AtomicBool = AtomicBool::new(false);

/// Set the first time a one-shot predecessor is forwarded to, so it is never
/// called twice. Only meaningful when [`PREV_ONESHOT`] is set.
#[cfg(unix)]
static ONESHOT_FIRED: AtomicBool = AtomicBool::new(false);

#[cfg(unix)]
const MAX_SUBSCRIBERS: usize = 32;

/// Slot value meaning "no subscriber".
#[cfg(unix)]
const FREE: i32 = -1;

/// Slot value meaning "claimed, but the fd is not published yet". The handler
/// skips any negative value, so a reserved slot is simply not notified.
#[cfg(unix)]
const RESERVED: i32 = -2;

/// Slot table for `subscribe`. `FREE` means free; any non-negative value is a
/// winch pipe write end. Reads and writes from the signal handler use
/// [`Ordering::Relaxed`] — visibility of newer registrations is established by
/// the AcqRel exchange in [`subscribe`].
#[cfg(unix)]
static SUBSCRIBERS: [AtomicI32; MAX_SUBSCRIBERS] =
    [const { AtomicI32::new(FREE) }; MAX_SUBSCRIBERS];

/// One winch pipe per slot, created on first use and then **never closed**.
///
/// A descriptor published in [`SUBSCRIBERS`] has to stay valid for as long as
/// a signal handler might still be holding it. The handler loads the fd and
/// only then calls `write`, so anything that closes it in between lets the
/// byte land in whatever descriptor the kernel has since handed that number
/// to — an unrelated file or socket. Clearing the slot first cannot help: it
/// does not revoke an fd another thread already loaded, and a handler cannot
/// take a lock or wait for quiescence to find out.
///
/// So the fds are made immortal instead, and a slot's pipe is leased to
/// whoever claims it next. [`MAX_SUBSCRIBERS`] caps that at 32 pipes (64 fds).
/// A late write from a handler that missed the unsubscribe now lands in the
/// same pipe it always did, where it is harmless: the next lessee drains it,
/// and a spurious wake only costs one `TIOCGWINSZ` that reports no change.
/// This is also what keeps a `fork`ed child safe — it inherits fds that can
/// never be recycled out from under the handler.
#[cfg(unix)]
static PIPES: [OnceLock<(OwnedFd, OwnedFd)>; MAX_SUBSCRIBERS] =
    [const { OnceLock::new() }; MAX_SUBSCRIBERS];

/// Process that created each entry of [`PIPES`], so an inherited pipe is never
/// *leased* after a `fork`: both processes would hold the same kernel pipe and
/// drain each other's wakes, silently losing resizes.
///
/// Two limits are deliberate. A subscription that was already published when
/// `fork` ran stays published in the child, so that one pipe really is shared
/// until the inherited source is dropped — resetting the table in a
/// `pthread_atfork` child handler would be worse, since the inherited
/// `Subscription` would then free a slot the child had re-leased. And because
/// [`PIPES`] entries are `OnceLock`s, a skipped slot stays skipped for that
/// process, so capacity is [`MAX_SUBSCRIBERS`] minus the slots an ancestor
/// initialised. Both only bite on `fork` without `exec`, where two processes
/// share one terminal.
#[cfg(unix)]
static PIPE_PIDS: [AtomicI32; MAX_SUBSCRIBERS] = [const { AtomicI32::new(0) }; MAX_SUBSCRIBERS];

#[cfg(unix)]
extern "C" fn handler(sig: libc::c_int, info: *mut libc::siginfo_t, ctx: *mut libc::c_void) {
    // POSIX requires a handler that calls a function which may set `errno` to
    // leave it as it found it: the interrupted thread may be sitting between a
    // failed libc call and its own read of `errno`.
    let errno = unsafe { errno_location() };
    let saved = if errno.is_null() {
        0
    } else {
        unsafe { *errno }
    };

    // Notify every registered subscriber. write(2) is async-signal-safe
    // (POSIX.1-2017, Sec. 2.4.3).
    for slot in SUBSCRIBERS.iter() {
        let fd = slot.load(Ordering::Relaxed);
        if fd >= 0 {
            let buf = b"w";
            // SAFETY: every fd ever published here belongs to a pipe in
            // `PIPES`, which is never closed, so this stays a valid descriptor
            // even if the `Subscription` that published it has already dropped.
            // The write end is non-blocking, so this cannot block the handler.
            unsafe {
                let _ = libc::write(fd, buf.as_ptr() as *const _, 1);
            }
        }
    }

    // Give the predecessor the errno the interrupted code had, not whatever our
    // writes just left behind. Restoring here rather than only after `chain`
    // also limits the damage if the predecessor never returns — though such a
    // predecessor is not actually supported; see `chain`.
    if !errno.is_null() {
        unsafe { *errno = saved };
    }

    chain(sig, info, ctx);

    if !errno.is_null() {
        unsafe { *errno = saved };
    }
}

/// Address of the calling thread's `errno`, or null on a target whose
/// accessor we do not know (in which case the save/restore is skipped).
#[cfg(unix)]
unsafe fn errno_location() -> *mut libc::c_int {
    #[cfg(any(
        target_os = "linux",
        target_os = "emscripten",
        target_os = "hurd",
        target_os = "redox",
        target_os = "dragonfly",
        target_os = "fuchsia",
        target_os = "l4re"
    ))]
    unsafe {
        libc::__errno_location()
    }
    #[cfg(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "tvos",
        target_os = "watchos",
        target_os = "visionos",
        target_os = "freebsd"
    ))]
    unsafe {
        libc::__error()
    }
    #[cfg(any(
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "android",
        target_os = "cygwin",
        target_os = "nuttx"
    ))]
    unsafe {
        libc::__errno()
    }
    #[cfg(target_os = "haiku")]
    unsafe {
        libc::_errnop()
    }
    #[cfg(target_os = "aix")]
    unsafe {
        libc::_Errno()
    }
    #[cfg(any(target_os = "solaris", target_os = "illumos"))]
    unsafe {
        libc::___errno()
    }
    #[cfg(not(any(
        target_os = "linux",
        target_os = "emscripten",
        target_os = "hurd",
        target_os = "redox",
        target_os = "dragonfly",
        target_os = "fuchsia",
        target_os = "l4re",
        target_os = "macos",
        target_os = "ios",
        target_os = "tvos",
        target_os = "watchos",
        target_os = "visionos",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "android",
        target_os = "cygwin",
        target_os = "nuttx",
        target_os = "haiku",
        target_os = "aix",
        target_os = "solaris",
        target_os = "illumos"
    )))]
    std::ptr::null_mut()
}

/// Forward the signal to the handler we displaced, so anyone who was watching
/// `SIGWINCH` before us keeps receiving it.
///
/// The predecessor is called through live Rust frames, so it must return
/// normally. A handler that escapes via `siglongjmp` — an old curses/readline
/// idiom — would deallocate those frames, which the Rust Reference classifies
/// as undefined behaviour. That is a limitation of chaining from Rust, not
/// something the errno handling below can paper over.
#[cfg(unix)]
fn chain(sig: libc::c_int, info: *mut libc::siginfo_t, ctx: *mut libc::c_void) {
    // Acquire pairs with the Release store in `remember`: observing a
    // non-sentinel address guarantees the matching arity is also visible.
    let prev = PREV_HANDLER.load(Ordering::Acquire);
    if prev == libc::SIG_DFL || prev == libc::SIG_IGN {
        return;
    }
    // A predecessor installed SA_RESETHAND asked to run exactly once: the
    // kernel would have reset the disposition as it entered. We call it
    // directly, so the kernel cannot do that for us and we consume the single
    // shot here instead. Calling such a handler twice can be a use-after-free,
    // since it is entitled to tear its state down on the way out.
    if PREV_ONESHOT.load(Ordering::Relaxed) && ONESHOT_FIRED.swap(true, Ordering::AcqRel) {
        return;
    }
    let siginfo = PREV_SIGINFO.load(Ordering::Relaxed);
    dispatch(prev, siginfo, sig, info, ctx);
}

/// Perform the indirect call. Split out of [`chain`] so it can be exercised
/// without mutating the process-global statics. Relaxed atomic loads and an
/// indirect call are async-signal-safe.
#[cfg(unix)]
fn dispatch(
    prev: usize,
    siginfo: bool,
    sig: libc::c_int,
    info: *mut libc::siginfo_t,
    ctx: *mut libc::c_void,
) {
    // Neither sentinel is a callable address; `SIG_DFL` for `SIGWINCH` means
    // "ignore", so both are a no-op for us.
    if prev == libc::SIG_DFL || prev == libc::SIG_IGN {
        return;
    }
    // SAFETY: `prev` was read from a `sigaction` query, so it is either one of
    // the sentinels rejected above or a valid handler address, whose arity is
    // `siginfo` as reported by that same query. Going through a raw pointer
    // rather than transmuting the integer directly keeps this well-defined
    // under strict provenance.
    unsafe {
        let addr = prev as *const ();
        if siginfo {
            let f: extern "C" fn(libc::c_int, *mut libc::siginfo_t, *mut libc::c_void) =
                std::mem::transmute(addr);
            f(sig, info, ctx);
        } else {
            let f: extern "C" fn(libc::c_int) = std::mem::transmute(addr);
            f(sig);
        }
    }
}

/// Record `old` as the disposition to forward to.
///
/// Called exactly once, from [`install_handler`], *before* our handler is
/// installed — so no signal handler can ever observe a half-published pair.
/// The arity and the one-shot flag are stored first and the address released
/// last, so a handler that later Acquire-loads a non-sentinel address is
/// guaranteed to see the values that belong to it.
///
/// Publishing a *second* time would reintroduce a tear that no memory ordering
/// can fix: with two published pairs a live handler can read the address from
/// one and the arity from the other (its two loads are separated in time, so
/// even `SeqCst` permits it), then call a three-argument handler through a
/// one-argument fn type. Two independent atomics cannot express this
/// invariant; publishing once makes it unrepresentable.
#[cfg(unix)]
fn remember(old: &libc::sigaction) {
    PREV_SIGINFO.store((old.sa_flags & libc::SA_SIGINFO) != 0, Ordering::Relaxed);
    PREV_ONESHOT.store((old.sa_flags & libc::SA_RESETHAND) != 0, Ordering::Relaxed);
    PREV_HANDLER.store(old.sa_sigaction, Ordering::Release);
}

#[cfg(unix)]
fn install_handler() {
    INSTALLED.call_once(|| unsafe {
        // Query and publish the incumbent *before* our handler goes live, so it
        // can never run with nothing recorded and silently drop the forward.
        let mut old: libc::sigaction = std::mem::zeroed();
        let queried = libc::sigaction(libc::SIGWINCH, std::ptr::null(), &mut old) == 0;
        if queried {
            remember(&old);
        }
        let prev = PREV_HANDLER.load(Ordering::Relaxed);
        let displacing = queried && prev != libc::SIG_DFL && prev != libc::SIG_IGN;

        let mut sa: libc::sigaction = std::mem::zeroed();
        sa.sa_sigaction = handler as *const () as usize;
        // SA_SIGINFO: `chain` needs siginfo/ucontext to forward faithfully to a
        //   three-argument predecessor.
        // SA_ONSTACK: we call the predecessor as a plain function, and the Go
        //   runtime aborts with "non-Go code set up signal handler without
        //   SA_ONSTACK flag" if entered off the alternate stack. It is ignored
        //   when no sigaltstack is installed, so it costs nothing otherwise.
        sa.sa_flags = libc::SA_SIGINFO | libc::SA_ONSTACK;
        if displacing {
            // Only one disposition exists per signal, so our flags govern the
            // whole process — including the code we displaced. Inherit the bits
            // that change *its* observable behaviour: a handler installed with
            // sa_flags = 0 is using the classic idiom where a blocking read
            // returns EINTR so the resize gets noticed, and forcing SA_RESTART
            // would silently break it. uncurses is EINTR-safe either way; its
            // pollers retry against an absolute deadline.
            sa.sa_flags |= old.sa_flags & (libc::SA_RESTART | libc::SA_NODEFER);
            // Keep blocked what the predecessor expects blocked; our own
            // fan-out needs nothing blocked.
            sa.sa_mask = old.sa_mask;
        } else {
            sa.sa_flags |= libc::SA_RESTART;
            libc::sigemptyset(&mut sa.sa_mask);
        }
        let mut displaced: libc::sigaction = std::mem::zeroed();
        libc::sigaction(libc::SIGWINCH, &sa, &mut displaced);

        // What we actually displaced can differ from what we queried a moment
        // ago. A third party may have installed in between — but the dangerous
        // case is a one-shot incumbent that *fired* in that window: the kernel
        // reset its disposition and its owner is now entitled to have torn its
        // state down, while `ONESHOT_FIRED` is still false because the call did
        // not go through us. Forwarding to the queried address would then be
        // the exact use-after-free `PREV_ONESHOT` exists to prevent.
        //
        // So on any mismatch, degrade to forwarding to nobody. Losing the chain
        // is a liveness cost across a two-syscall window; calling freed code is
        // not survivable. Storing a *sentinel* is also the one publication that
        // is safe after our handler is live: `chain` rejects sentinels before it
        // ever reads the arity or the one-shot flag, so this cannot tear the
        // pair the way re-recording a real address would.
        if displaced.sa_sigaction != PREV_HANDLER.load(Ordering::Relaxed) {
            PREV_HANDLER.store(libc::SIG_DFL, Ordering::Release);
        }
    });
}

/// RAII handle that unregisters its slot on drop, returning the slot's pipe to
/// the pool. The pipe itself is never closed; see [`PIPES`].
#[cfg(unix)]
#[derive(Debug)]
pub(crate) struct Subscription {
    slot: usize,
}

#[cfg(unix)]
impl Subscription {
    /// Read end of this subscription's pipe, readable once the handler has
    /// fired. Valid for the whole process lifetime, but only meaningful while
    /// this `Subscription` is alive.
    pub(crate) fn read_fd(&self) -> i32 {
        PIPES[self.slot]
            .get()
            .expect("slot pipe is initialised before the slot is published")
            .0
            .as_raw_fd()
    }
}

#[cfg(unix)]
impl Drop for Subscription {
    fn drop(&mut self) {
        // Release ordering pairs with the AcqRel in `subscribe` so that any
        // subsequent registrant observes the slot as free. The pipe stays open
        // and is handed to the next lessee.
        SUBSCRIBERS[self.slot].store(FREE, Ordering::Release);
    }
}

/// Claim a slot and start receiving a one-byte wake from the SIGWINCH handler
/// on every resize. Read the wake from [`Subscription::read_fd`].
///
/// Installs the shared handler on first call.
#[cfg(unix)]
pub(crate) fn subscribe() -> std::io::Result<Subscription> {
    install_handler();
    for (i, slot) in SUBSCRIBERS.iter().enumerate() {
        if slot
            .compare_exchange(FREE, RESERVED, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            continue;
        }
        // The slot is ours exclusively, so we are the only possible initialiser
        // of this pipe, and the only reader of it until we drop.
        let pid = unsafe { libc::getpid() };
        let pipe = match PIPES[i].get() {
            // Inherited from a `fork`: the pipe object is shared with the
            // process that made it, so leasing it here would have the two
            // draining each other. Leave the slot free and take a fresh one.
            Some(_) if PIPE_PIDS[i].load(Ordering::Relaxed) != pid => {
                slot.store(FREE, Ordering::Release);
                continue;
            }
            Some(pipe) => pipe,
            None => match super::source_unix::make_self_pipe() {
                Ok(pipe) => {
                    let _ = PIPES[i].set(pipe);
                    PIPE_PIDS[i].store(pid, Ordering::Relaxed);
                    PIPES[i].get().expect("just set")
                }
                Err(e) => {
                    slot.store(FREE, Ordering::Release);
                    return Err(e);
                }
            },
        };
        // A recycled pipe can still hold bytes written after the previous
        // lessee unsubscribed; left there they would surface as a spurious
        // resize on this source's first poll.
        super::source_unix::drain_pipe(pipe.0.as_raw_fd());
        slot.store(pipe.1.as_raw_fd(), Ordering::Release);
        return Ok(Subscription { slot: i });
    }
    Err(std::io::Error::other("too many SIGWINCH subscribers"))
}

#[cfg(not(unix))]
#[derive(Debug)]
#[allow(dead_code)]
pub(crate) struct Subscription;

#[cfg(not(unix))]
#[allow(dead_code)]
pub(crate) fn subscribe() -> std::io::Result<Subscription> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "SIGWINCH not supported on this platform",
    ))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    static ONE_ARG: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    static THREE_ARG: AtomicBool = AtomicBool::new(false);

    extern "C" fn prev_one_arg(_sig: libc::c_int) {
        ONE_ARG.fetch_add(1, Ordering::SeqCst);
    }

    extern "C" fn prev_three_arg(
        _sig: libc::c_int,
        _info: *mut libc::siginfo_t,
        _ctx: *mut libc::c_void,
    ) {
        THREE_ARG.store(true, Ordering::SeqCst);
    }

    /// `dispatch` takes the handler and its arity as arguments rather than
    /// reading the process-global statics, so this cannot race the rest of the
    /// suite (which installs and raises real `SIGWINCH`s).
    #[test]
    fn dispatch_rejects_sentinels_and_honours_arity() {
        let (info, ctx) = (std::ptr::null_mut(), std::ptr::null_mut());

        // Neither sentinel is a callable address — `SIG_DFL` is literally 0 —
        // so reaching the next assertion proves they were rejected, not called.
        for sentinel in [libc::SIG_DFL, libc::SIG_IGN] {
            dispatch(sentinel, false, libc::SIGWINCH, info, ctx);
            dispatch(sentinel, true, libc::SIGWINCH, info, ctx);
        }

        dispatch(
            prev_one_arg as *const () as usize,
            false,
            libc::SIGWINCH,
            info,
            ctx,
        );
        assert_eq!(
            ONE_ARG.load(Ordering::SeqCst),
            1,
            "one-argument handler not called"
        );

        dispatch(
            prev_three_arg as *const () as usize,
            true,
            libc::SIGWINCH,
            info,
            ctx,
        );
        assert!(
            THREE_ARG.load(Ordering::SeqCst),
            "SA_SIGINFO handler not called"
        );
    }

    /// End-to-end, in a re-executed child process so `INSTALLED` is guaranteed
    /// not to have fired yet: install a sentinel handler, let `subscribe` put
    /// ours on top, raise `SIGWINCH`, and require that *both* the subscriber
    /// pipe was woken and the displaced handler was chained to.
    ///
    /// The sentinel is installed `SA_RESETHAND`, and the signal is raised
    /// twice, so this also pins the one-shot emulation: the kernel can no
    /// longer reset the disposition on our behalf, so `chain` must consume the
    /// single shot itself and call the predecessor exactly once.
    #[test]
    fn install_chains_to_the_incumbent() {
        const VAR: &str = "UNCURSES_SIGWINCH_CHAIN_CHILD";
        const NAME: &str = "event::sigwinch::tests::install_chains_to_the_incumbent";

        if std::env::var_os(VAR).is_some() {
            return in_child();
        }

        let out = std::process::Command::new(std::env::current_exe().unwrap())
            .args([NAME, "--exact", "--nocapture", "--test-threads=1"])
            .env(VAR, "1")
            .output()
            .expect("re-exec test binary");

        assert!(
            out.status.success(),
            "child process failed:\n--- stdout ---\n{}\n--- stderr ---\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr),
        );
        // A filter that matches nothing still exits 0, so a rename of this test
        // would silently turn the whole check into a no-op. Require that the
        // child actually ran it.
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            stdout.contains("1 passed"),
            "child ran no test — is NAME still the path of this test?\n{stdout}"
        );
    }

    fn in_child() {
        unsafe {
            let mut sa: libc::sigaction = std::mem::zeroed();
            sa.sa_sigaction = prev_one_arg as *const () as usize;
            sa.sa_flags = libc::SA_RESTART | libc::SA_RESETHAND;
            libc::sigemptyset(&mut sa.sa_mask);
            assert_eq!(
                libc::sigaction(libc::SIGWINCH, &sa, std::ptr::null_mut()),
                0,
                "could not install the sentinel handler"
            );

            // Darwin drops SA_RESETHAND from a query, so ask this platform
            // whether the flag is observable at all rather than assuming it.
            let mut probe: libc::sigaction = std::mem::zeroed();
            assert_eq!(
                libc::sigaction(libc::SIGWINCH, std::ptr::null(), &mut probe),
                0,
                "could not query the sentinel handler"
            );
            let oneshot_observable = (probe.sa_flags & libc::SA_RESETHAND) != 0;

            // The leased pipe is already non-blocking, so a missing wake fails
            // fast instead of hanging the test binary forever.
            let sub = subscribe().expect("subscribe");
            assert_eq!(libc::raise(libc::SIGWINCH), 0, "raise() failed");

            let mut b = [0u8; 1];
            assert_eq!(
                libc::read(sub.read_fd(), b.as_mut_ptr().cast(), 1),
                1,
                "subscriber pipe was not woken"
            );
            assert_eq!(
                ONE_ARG.load(Ordering::SeqCst),
                1,
                "the displaced handler was not chained to"
            );

            // `raise` runs the handler before it returns, so by this point a
            // second call would already have happened.
            assert_eq!(libc::raise(libc::SIGWINCH), 0, "second raise() failed");
            assert_eq!(
                ONE_ARG.load(Ordering::SeqCst),
                if oneshot_observable { 1 } else { 2 },
                "SA_RESETHAND emulation disagrees with what this platform reports \
                 (observable: {oneshot_observable})"
            );
            assert_eq!(
                libc::read(sub.read_fd(), b.as_mut_ptr().cast(), 1),
                1,
                "subscriber pipe was not woken by the second signal"
            );

            // The branch above only exercises the emulation on platforms that
            // report SA_RESETHAND. Drive it directly so it is covered
            // everywhere: this child process is ours alone, so publishing the
            // flag by hand is safe and hits the real `chain` code path.
            PREV_ONESHOT.store(true, Ordering::SeqCst);
            ONESHOT_FIRED.store(false, Ordering::SeqCst);
            ONE_ARG.store(0, Ordering::SeqCst);
            for _ in 0..2 {
                assert_eq!(libc::raise(libc::SIGWINCH), 0, "raise() failed");
                assert_eq!(libc::read(sub.read_fd(), b.as_mut_ptr().cast(), 1), 1);
            }
            assert_eq!(
                ONE_ARG.load(Ordering::SeqCst),
                1,
                "a one-shot predecessor was forwarded to more than once"
            );

            // A pooled pipe must never be leased in a process that did not
            // create it: after a fork both copies hold the same kernel pipe and
            // would drain each other's wakes. Free a slot, fork, and require
            // that the child does not get that slot's pipe back. Running inside
            // this re-exec'd child means libtest is --test-threads=1, so the
            // fork has no other threads to inherit locks from.
            let recycled_fd = {
                let pooled = subscribe().expect("subscribe");
                pooled.read_fd()
            };

            let pid = libc::fork();
            assert!(pid >= 0, "fork() failed");
            if pid == 0 {
                let child = subscribe().expect("subscribe in child");
                // Had the child reused the inherited pipe it would be handed
                // the very same descriptor back.
                libc::_exit(i32::from(child.read_fd() == recycled_fd));
            }
            let mut status = 0;
            assert_eq!(libc::waitpid(pid, &mut status, 0), pid, "waitpid failed");
            assert!(
                libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0,
                "child leased a winch pipe inherited from its parent"
            );
        }
    }
}
