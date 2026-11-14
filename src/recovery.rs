// SPDX-License-Identifier: GPL-3.0-or-later
//! Retry pacing and Linux suspend detection without a background polling thread.
use std::{io, time::Duration};

#[derive(Default)]
pub struct Backoff {
    attempts: u32,
}

impl Backoff {
    pub fn next_delay(&mut self) -> Duration {
        let seconds = (1_u64 << self.attempts.min(4)).min(15);
        self.attempts = self.attempts.saturating_add(1);
        Duration::from_secs(seconds)
    }

    pub fn reset(&mut self) {
        self.attempts = 0;
    }
}

fn clock(id: libc::clockid_t) -> io::Result<Duration> {
    let mut value = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: value is a valid, writable timespec and both clock IDs are Linux constants.
    if unsafe { libc::clock_gettime(id, &mut value) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(Duration::new(value.tv_sec as u64, value.tv_nsec as u32))
}

fn suspend_offset() -> io::Result<Option<Duration>> {
    let before = clock(libc::CLOCK_MONOTONIC)?;
    let boot = clock(libc::CLOCK_BOOTTIME)?;
    let after = clock(libc::CLOCK_MONOTONIC)?;
    // Discard snapshots interrupted by scheduling rather than mistake them for suspend.
    let span = after.saturating_sub(before);
    Ok((span < Duration::from_millis(10)).then(|| boot.saturating_sub(before + span / 2)))
}

#[derive(Debug)]
pub struct ResumeDetector {
    offset: Option<Duration>,
}

impl ResumeDetector {
    pub fn new() -> io::Result<Self> {
        Ok(Self {
            offset: suspend_offset()?,
        })
    }

    pub fn resumed(&mut self) -> io::Result<bool> {
        Ok(self.observe(suspend_offset()?))
    }

    fn observe(&mut self, offset: Option<Duration>) -> bool {
        let Some(offset) = offset else { return false };
        let resumed = self
            .offset
            .is_some_and(|previous| offset.saturating_sub(previous) > Duration::from_millis(100));
        self.offset = Some(offset);
        resumed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retries_are_bounded_and_reset_after_success() {
        let mut backoff = Backoff::default();
        for seconds in [1, 2, 4, 8, 15, 15, 15] {
            assert_eq!(backoff.next_delay(), Duration::from_secs(seconds));
        }
        backoff.reset();
        assert_eq!(backoff.next_delay(), Duration::from_secs(1));
    }

    #[test]
    fn suspend_offsets_detect_resume_without_wall_clock_time() {
        let mut detector = ResumeDetector {
            offset: Some(Duration::from_secs(2)),
        };
        assert!(!detector.observe(Some(Duration::from_millis(2001))));
        assert!(!detector.observe(None));
        assert!(detector.observe(Some(Duration::from_secs(62))));
        assert!(!detector.observe(Some(Duration::from_secs(62))));
        assert!(!detector.observe(Some(Duration::from_millis(61999))));
        assert!(ResumeDetector::new().unwrap().offset.is_some());
    }
}
