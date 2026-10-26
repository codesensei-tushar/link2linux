//! Liveness: the desktop pings, the phone answers, and silence ends the
//! session.

use std::time::Duration;

use tokio::time::{self, Instant, Interval, MissedTickBehavior};

/// How often a side that drives liveness pings, and when it gives up.
///
/// The desktop drives liveness. It pings often while the user can see what
/// the session does, and rarely otherwise, because every `Ping` wakes a
/// dozing phone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PingSchedule {
    /// The interval while the session carries activity the user can see:
    /// the desktop's window is focused, or a transfer or call is running.
    /// See [`Session::set_active`](super::Session::set_active).
    pub active: Duration,
    /// The interval otherwise.
    pub idle: Duration,
    /// The session is lost after this many intervals without any frame
    /// from the peer.
    pub missed: u32,
}

impl Default for PingSchedule {
    /// 15 seconds while active, 60 seconds while idle, lost after 3 silent
    /// intervals.
    fn default() -> Self {
        Self {
            active: Duration::from_secs(15),
            idle: Duration::from_secs(60),
            missed: 3,
        }
    }
}

/// The liveness state of one session.
#[derive(Debug)]
pub(super) struct Liveness {
    /// `None` on a side that only answers pings.
    schedule: Option<PingSchedule>,
    /// How long the peer may stay silent on a side that does not ping.
    silence_timeout: Duration,
    active: bool,
    /// Fires when the next `Ping` is due.
    timer: Option<Interval>,
    /// When the peer last sent a frame, or the cadence last changed,
    /// whichever is later. Silence is measured from here.
    since: Instant,
}

impl Liveness {
    /// Starts idle at `now`.
    pub(super) fn new(
        schedule: Option<PingSchedule>,
        silence_timeout: Duration,
        now: Instant,
    ) -> Self {
        let mut liveness = Self {
            schedule,
            silence_timeout,
            active: false,
            timer: None,
            since: now,
        };
        liveness.restart(now);
        liveness
    }

    /// Notes that a frame arrived from the peer at `now`.
    pub(super) fn received(&mut self, now: Instant) {
        self.since = now;
    }

    /// Whether the session carries activity the user can see.
    pub(super) fn is_active(&self) -> bool {
        self.active
    }

    /// Switches between the active and the idle cadence at `now`. Returns
    /// whether to ping right away: when a pinging side becomes active, so a
    /// dead connection shows up while the user looks.
    pub(super) fn set_active(&mut self, active: bool, now: Instant) -> bool {
        if self.active == active {
            return false;
        }
        self.active = active;
        self.restart(now);
        active && self.schedule.is_some()
    }

    /// How long the peer may stay silent now.
    pub(super) fn limit(&self) -> Duration {
        match (self.schedule, self.interval()) {
            (Some(schedule), Some(interval)) => interval * schedule.missed,
            _ => self.silence_timeout,
        }
    }

    /// When the session is lost unless a frame arrives first.
    pub(super) fn deadline(&self) -> Instant {
        self.since + self.limit()
    }

    /// Waits until the next `Ping` is due; forever on a side that does not
    /// ping. Cancel safe.
    pub(super) async fn next_ping(&mut self) {
        match &mut self.timer {
            Some(timer) => {
                timer.tick().await;
            }
            None => std::future::pending().await,
        }
    }

    /// The current ping interval, on a side that pings.
    fn interval(&self) -> Option<Duration> {
        let schedule = self.schedule?;
        Some(if self.active {
            schedule.active
        } else {
            schedule.idle
        })
    }

    /// Restarts the ping timer and the silence measurement at `now`.
    fn restart(&mut self, now: Instant) {
        self.since = now;
        self.timer = self.interval().map(|interval| {
            let mut timer = time::interval_at(now + interval, interval);
            timer.set_missed_tick_behavior(MissedTickBehavior::Delay);
            timer
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn the_limit_follows_the_cadence() {
        let now = Instant::now();
        let mut liveness = Liveness::new(Some(PingSchedule::default()), Duration::ZERO, now);
        assert_eq!(liveness.limit(), Duration::from_secs(180));
        assert!(liveness.set_active(true, now));
        assert!(!liveness.set_active(true, now));
        assert_eq!(liveness.limit(), Duration::from_secs(45));
        assert!(!liveness.set_active(false, now));
        assert_eq!(liveness.deadline(), now + Duration::from_secs(180));
    }

    #[tokio::test(start_paused = true)]
    async fn a_side_that_answers_only_waits_longer() {
        let now = Instant::now();
        let mut liveness = Liveness::new(None, Duration::from_secs(240), now);
        assert!(!liveness.set_active(true, now));
        assert_eq!(liveness.limit(), Duration::from_secs(240));
        let later = now + Duration::from_secs(5);
        liveness.received(later);
        assert_eq!(liveness.deadline(), later + Duration::from_secs(240));
    }
}
