//! Automatic checks do not take ownership of a manually initiated download.
use super::Status;
use std::time::{Duration, Instant};
const PERIOD: Duration = Duration::from_secs(6 * 60 * 60);

#[derive(Debug, PartialEq, Eq)]
pub enum Action {
    Check,
    Download,
    Install,
    Cancel,
}
#[derive(Debug, PartialEq, Eq)]
enum Phase {
    Check,
    Download,
}
pub struct Schedule {
    enabled: bool,
    check_requested: bool,
    next: Instant,
    phase: Option<Phase>,
    failures: u32,
}
impl Schedule {
    pub fn new(enabled: bool, recovering: bool, now: Instant) -> Self {
        Self {
            enabled,
            check_requested: enabled && !recovering,
            next: if recovering { now + PERIOD } else { now },
            phase: None,
            failures: 0,
        }
    }
    pub fn manual(&mut self, now: Instant) {
        self.phase = None;
        self.check_requested = false;
        self.next = now + PERIOD;
    }
    pub fn tick(
        &mut self,
        enabled: bool,
        busy: bool,
        status: &Status,
        now: Instant,
    ) -> Option<Action> {
        if enabled != self.enabled {
            self.enabled = enabled;
            self.check_requested = enabled;
            if enabled {
                self.next = now;
            } else if self.phase.take().is_some() {
                return Some(Action::Cancel);
            }
        }
        if !enabled {
            return None;
        }
        if self.phase.is_some() {
            match status {
                Status::Error(_) => {
                    self.phase = None;
                    self.next =
                        now + Duration::from_secs((300u64 << self.failures.min(4)).min(3600));
                    self.failures = self.failures.saturating_add(1);
                }
                Status::Cancelled | Status::Current => {
                    self.phase = None;
                    self.failures = 0;
                    self.next = now + PERIOD;
                }
                Status::Available(_) if self.phase == Some(Phase::Check) && !busy => {
                    self.phase = Some(Phase::Download);
                    return Some(Action::Download);
                }
                Status::Ready(_) if self.phase == Some(Phase::Download) && !busy => {
                    self.phase = None;
                    self.failures = 0;
                    self.next = now + PERIOD;
                    return Some(Action::Install);
                }
                _ => {}
            }
        } else if !busy
            && (self.check_requested
                || (now >= self.next
                    && matches!(
                        status,
                        Status::Idle | Status::Current | Status::Error(_) | Status::Cancelled
                    )))
        {
            self.check_requested = false;
            self.phase = Some(Phase::Check);
            return Some(Action::Check);
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn enabling_at_runtime_checks_without_restart() {
        let now = Instant::now();
        let mut s = Schedule::new(false, false, now);
        assert_eq!(s.tick(false, false, &Status::Idle, now), None);
        assert_eq!(s.tick(true, false, &Status::Idle, now), Some(Action::Check));
        assert_eq!(s.tick(true, true, &Status::Checking, now), None);
    }
    #[test]
    fn enabling_after_a_manual_offer_rechecks_when_the_worker_is_free() {
        let now = Instant::now();
        let mut s = Schedule::new(false, false, now);
        assert_eq!(s.tick(true, true, &Status::Downloading(1, 2), now), None);
        assert_eq!(
            s.tick(true, false, &Status::Ready("3.0.0".into()), now),
            Some(Action::Check)
        );
        let mut s = Schedule::new(false, false, now);
        assert_eq!(
            s.tick(true, false, &Status::Available("3.0.0".into()), now),
            Some(Action::Check)
        );
    }
    #[test]
    fn terminal_states_retry_or_schedule_without_state_changes() {
        let now = Instant::now();
        let mut s = Schedule::new(true, false, now);
        assert_eq!(s.tick(true, false, &Status::Idle, now), Some(Action::Check));
        s.tick(true, false, &Status::Error("offline".into()), now);
        assert_eq!(
            s.tick(
                true,
                false,
                &Status::Error("offline".into()),
                now + Duration::from_secs(299)
            ),
            None
        );
        assert_eq!(
            s.tick(
                true,
                false,
                &Status::Error("offline".into()),
                now + Duration::from_secs(300)
            ),
            Some(Action::Check)
        );
        s.tick(true, false, &Status::Current, now);
        assert_eq!(
            s.tick(true, false, &Status::Current, now + PERIOD),
            Some(Action::Check)
        );
    }
    #[test]
    fn download_and_install_are_each_issued_once() {
        let now = Instant::now();
        let mut s = Schedule::new(true, false, now);
        s.tick(true, false, &Status::Idle, now);
        assert_eq!(
            s.tick(true, false, &Status::Available("3.0.0".into()), now),
            Some(Action::Download)
        );
        assert_eq!(
            s.tick(true, false, &Status::Available("3.0.0".into()), now),
            None
        );
        assert_eq!(
            s.tick(true, false, &Status::Ready("3.0.0".into()), now),
            Some(Action::Install)
        );
        assert_eq!(
            s.tick(true, false, &Status::Ready("3.0.0".into()), now),
            None
        );
    }
    #[test]
    fn manual_work_and_recovery_are_not_automatically_installed() {
        let now = Instant::now();
        let mut s = Schedule::new(true, true, now);
        assert_eq!(s.tick(true, false, &Status::Idle, now), None);
        s.manual(now);
        assert_eq!(
            s.tick(true, false, &Status::Ready("3.0.0".into()), now),
            None
        );
        assert_eq!(s.tick(false, true, &Status::Downloading(1, 5), now), None);
        assert_eq!(s.tick(true, true, &Status::Downloading(1, 5), now), None);
    }
    #[test]
    fn turning_off_automatic_updates_cancels_its_own_job() {
        let now = Instant::now();
        let mut s = Schedule::new(true, false, now);
        s.tick(true, false, &Status::Idle, now);
        assert_eq!(
            s.tick(false, true, &Status::Checking, now),
            Some(Action::Cancel)
        );
        assert_eq!(s.tick(false, true, &Status::Checking, now), None);
    }
}
