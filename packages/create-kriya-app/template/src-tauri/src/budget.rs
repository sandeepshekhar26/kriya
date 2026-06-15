//! Action-rate budgeting. A runaway or looping agent shouldn't be able to hammer the app;
//! the host enforces a sliding-window cap on how many actions it may take per minute.
//!
//! Time is passed in as milliseconds so the logic is deterministic and unit-testable.

const WINDOW_MS: u128 = 60_000;

pub struct BudgetTracker {
    max_per_minute: Option<u32>,
    times: Vec<u128>,
}

impl BudgetTracker {
    pub fn new(max_per_minute: Option<u32>) -> Self {
        Self { max_per_minute, times: Vec::new() }
    }

    /// Record an action attempt at `now_ms`. Returns `Err` (with a human-readable reason)
    /// if it would exceed the per-minute budget; otherwise records it and returns `Ok`.
    /// With no configured limit this is always `Ok`.
    pub fn check_and_record(&mut self, now_ms: u128) -> Result<(), String> {
        let Some(max) = self.max_per_minute else {
            return Ok(());
        };
        // Drop timestamps outside the trailing one-minute window.
        self.times.retain(|t| now_ms.saturating_sub(*t) < WINDOW_MS);
        if self.times.len() as u32 >= max {
            return Err(format!("budget exceeded: {max} actions/minute"));
        }
        self.times.push(now_ms);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_limit_allows_everything() {
        let mut b = BudgetTracker::new(None);
        for t in 0..1000u128 {
            assert!(b.check_and_record(t).is_ok());
        }
    }

    #[test]
    fn enforces_per_minute_cap_in_window() {
        let mut b = BudgetTracker::new(Some(2));
        assert!(b.check_and_record(0).is_ok());
        assert!(b.check_and_record(1_000).is_ok());
        // third within the same minute is rejected
        assert!(b.check_and_record(2_000).is_err());
    }

    #[test]
    fn window_slides_so_old_actions_expire() {
        let mut b = BudgetTracker::new(Some(2));
        assert!(b.check_and_record(0).is_ok());
        assert!(b.check_and_record(1_000).is_ok());
        assert!(b.check_and_record(2_000).is_err());
        // once the first two age out of the 60s window, capacity returns
        assert!(b.check_and_record(61_001).is_ok());
    }
}
