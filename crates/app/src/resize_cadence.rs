use std::time::{Duration, Instant};

#[derive(Debug)]
pub(crate) struct ResizeFrameCadence {
    interval: Duration,
    last_flush: Instant,
    deadline: Option<Instant>,
}

impl ResizeFrameCadence {
    pub(crate) fn new(now: Instant, interval: Duration) -> Self {
        Self {
            interval,
            last_flush: now,
            deadline: None,
        }
    }

    pub(crate) fn queue(&mut self, now: Instant) {
        let earliest = self.last_flush.checked_add(self.interval).unwrap_or(now);
        let next = if now >= earliest { now } else { earliest };
        self.deadline = Some(self.deadline.map_or(next, |pending| pending.min(next)));
    }

    pub(crate) fn take_due(&mut self, now: Instant) -> bool {
        if self.deadline.is_none_or(|deadline| now < deadline) {
            return false;
        }
        self.deadline = None;
        self.last_flush = now;
        true
    }

    pub(crate) const fn deadline(&self) -> Option<Instant> {
        self.deadline
    }

    pub(crate) fn cancel(&mut self, now: Instant) {
        self.deadline = None;
        self.last_flush = now;
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::ResizeFrameCadence;

    const FRAME: Duration = Duration::from_millis(16);

    #[test]
    fn burst_is_limited_to_one_rebuild_per_frame() {
        let started = Instant::now();
        let mut cadence = ResizeFrameCadence::new(started, FRAME);

        cadence.queue(started + Duration::from_millis(1));
        cadence.queue(started + Duration::from_millis(5));
        cadence.queue(started + Duration::from_millis(12));

        assert_eq!(cadence.deadline(), Some(started + FRAME));
        assert!(!cadence.take_due(started + Duration::from_millis(15)));
        assert!(cadence.take_due(started + FRAME));
        assert_eq!(cadence.deadline(), None);
    }

    #[test]
    fn final_resize_gets_a_trailing_rebuild() {
        let started = Instant::now();
        let mut cadence = ResizeFrameCadence::new(started, FRAME);

        cadence.queue(started + Duration::from_millis(1));
        assert!(cadence.take_due(started + FRAME));

        cadence.queue(started + Duration::from_millis(17));
        assert_eq!(
            cadence.deadline(),
            Some(started + Duration::from_millis(32))
        );
        assert!(!cadence.take_due(started + Duration::from_millis(31)));
        assert!(cadence.take_due(started + Duration::from_millis(32)));
    }

    #[test]
    fn resize_after_idle_can_rebuild_immediately() {
        let started = Instant::now();
        let mut cadence = ResizeFrameCadence::new(started, FRAME);
        let after_idle = started + Duration::from_secs(1);

        cadence.queue(after_idle);

        assert_eq!(cadence.deadline(), Some(after_idle));
        assert!(cadence.take_due(after_idle));
    }
}
