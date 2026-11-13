use std::{
    io,
    time::{Duration, Instant},
};

use uncurses::{
    event::Event,
    program::Program,
    terminal::{Stdin, Stdout},
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

    pub fn read_event(
        &mut self,
        program: &mut Program<Stdin, Stdout>,
    ) -> io::Result<Option<Event>> {
        loop {
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
            let event = program.read_event()?;
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
