use std::{
    io,
    time::{Duration, Instant},
};

use uncurses::{
    event::{Event, Input},
    program::Program,
};

pub struct CapabilityQuery {
    deadline: Option<Instant>,
    quit_requested: bool,
}

impl CapabilityQuery {
    pub fn new() -> Self {
        Self {
            deadline: Some(Instant::now() + Duration::from_secs(2)),
            quit_requested: false,
        }
    }

    pub fn request_quit(&mut self) -> bool {
        self.quit_requested = true;
        self.deadline.is_none()
    }

    pub fn read_event<I: Input, O: io::Write>(
        &mut self,
        program: &mut Program<I, O>,
    ) -> io::Result<Option<Event>> {
        loop {
            let event = if let Some(event) = program.try_read_event()? {
                event
            } else {
                if let Some(deadline) = self.deadline
                    && (Instant::now() >= deadline
                        || !program
                            .poll_event(Some(deadline.saturating_duration_since(Instant::now())))?)
                {
                    if self.quit_requested {
                        return Ok(None);
                    }
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "capability query timed out",
                    ));
                }
                program.read_event()?
            };
            if matches!(event, Event::PrimaryDeviceAttributes(_)) {
                self.deadline = None;
            }
            if !self.quit_requested {
                return Ok(Some(event));
            }
            if self.deadline.is_none() {
                return Ok(None);
            }
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::{fs::File, os::fd::FromRawFd, ptr};
    use uncurses::terminal::{EnvList, Terminal};

    fn pty() -> (File, File) {
        let mut master = -1;
        let mut slave = -1;
        assert_eq!(
            unsafe {
                libc::openpty(
                    &mut master,
                    &mut slave,
                    ptr::null_mut(),
                    ptr::null_mut(),
                    ptr::null_mut(),
                )
            },
            0
        );
        unsafe { (File::from_raw_fd(master), File::from_raw_fd(slave)) }
    }

    #[test]
    fn queued_capability_reply_finishes_after_the_deadline() {
        for quitting in [false, true] {
            let (_master, slave) = pty();
            let mut program = Program::new(Terminal::new(&slave, &slave, EnvList::new())).unwrap();
            program.unread_event(Event::PrimaryDeviceAttributes(Vec::new()));
            program.unread_event(Event::Unknown(b"queued".to_vec()));
            let mut query = CapabilityQuery {
                deadline: Some(Instant::now()),
                quit_requested: quitting,
            };
            if quitting {
                assert!(query.read_event(&mut program).unwrap().is_none());
            } else {
                assert_eq!(
                    query.read_event(&mut program).unwrap(),
                    Some(Event::Unknown(b"queued".to_vec()))
                );
                assert!(matches!(
                    query.read_event(&mut program).unwrap(),
                    Some(Event::PrimaryDeviceAttributes(_))
                ));
            }
            assert!(query.deadline.is_none());
        }
    }

    #[test]
    fn an_empty_queue_still_obeys_the_deadline() {
        for quitting in [false, true] {
            let (_master, slave) = pty();
            let mut program = Program::new(Terminal::new(&slave, &slave, EnvList::new())).unwrap();
            let mut query = CapabilityQuery {
                deadline: Some(Instant::now()),
                quit_requested: quitting,
            };
            let result = query.read_event(&mut program);
            if quitting {
                assert!(result.unwrap().is_none());
            } else {
                assert_eq!(result.unwrap_err().kind(), io::ErrorKind::TimedOut);
            }
        }
    }
}
