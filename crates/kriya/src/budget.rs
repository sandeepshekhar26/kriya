//! Rate budgeting. A runaway or looping agent shouldn't be able to hammer the app or run up
//! unbounded model cost; the host enforces two independent sliding-window caps:
//!   - **actions/minute** — bursts of typed actions against the app (safety), and
//!   - **api-calls/hour** — calls to the (possibly paid, possibly remote) inference backend (cost).
//!
//! Time is passed in as milliseconds so the logic is deterministic and unit-testable.

const MINUTE_MS: u128 = 60_000;
const HOUR_MS: u128 = 3_600_000;

/// A single trailing-window rate limit. `max == None` means unlimited.
struct SlidingWindow {
    max: Option<u32>,
    window_ms: u128,
    times: Vec<u128>,
}

impl SlidingWindow {
    fn new(max: Option<u32>, window_ms: u128) -> Self {
        Self { max, window_ms, times: Vec::new() }
    }

    /// Record an event at `now_ms`, or return `Err(max)` if it would exceed the cap. With no
    /// configured limit this is always `Ok`.
    fn check_and_record(&mut self, now_ms: u128) -> Result<(), u32> {
        let Some(max) = self.max else {
            return Ok(());
        };
        // Drop timestamps outside the trailing window.
        self.times.retain(|t| now_ms.saturating_sub(*t) < self.window_ms);
        if self.times.len() as u32 >= max {
            return Err(max);
        }
        self.times.push(now_ms);
        Ok(())
    }
}

pub struct BudgetTracker {
    actions: SlidingWindow,
    api_calls: SlidingWindow,
}

impl BudgetTracker {
    /// A tracker with the per-minute action cap. The api-calls/hour cap defaults to unlimited;
    /// add it with [`BudgetTracker::with_api_calls_per_hour`].
    pub fn new(max_per_minute: Option<u32>) -> Self {
        Self {
            actions: SlidingWindow::new(max_per_minute, MINUTE_MS),
            api_calls: SlidingWindow::new(None, HOUR_MS),
        }
    }

    /// Set the trailing-hour cap on inference/API calls. Chainable on top of [`BudgetTracker::new`].
    pub fn with_api_calls_per_hour(mut self, max: Option<u32>) -> Self {
        self.api_calls = SlidingWindow::new(max, HOUR_MS);
        self
    }

    /// Record an action attempt at `now_ms`. `Err` (with a human-readable reason) if it would
    /// exceed the per-minute action budget; otherwise records it and returns `Ok`.
    pub fn check_and_record(&mut self, now_ms: u128) -> Result<(), String> {
        self.actions
            .check_and_record(now_ms)
            .map_err(|max| format!("budget exceeded: {max} actions/minute"))
    }

    /// Record an inference/API call at `now_ms`. `Err` (with a human-readable reason) if it would
    /// exceed the per-hour api-call budget; otherwise records it and returns `Ok`. Independent of
    /// the per-minute action window.
    pub fn check_and_record_api_call(&mut self, now_ms: u128) -> Result<(), String> {
        self.api_calls
            .check_and_record(now_ms)
            .map_err(|max| format!("budget exceeded: {max} api calls/hour"))
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

    #[test]
    fn no_api_call_limit_by_default() {
        let mut b = BudgetTracker::new(Some(60)); // action cap set, api cap unset
        for t in (0..1_000_000u128).step_by(1000) {
            assert!(b.check_and_record_api_call(t).is_ok());
        }
    }

    #[test]
    fn enforces_api_calls_per_hour_in_window() {
        let mut b = BudgetTracker::new(None).with_api_calls_per_hour(Some(3));
        assert!(b.check_and_record_api_call(0).is_ok());
        assert!(b.check_and_record_api_call(60_000).is_ok());
        assert!(b.check_and_record_api_call(120_000).is_ok());
        // fourth within the same hour is rejected
        assert!(b.check_and_record_api_call(180_000).is_err());
        // ...until the earliest call ages out of the trailing hour
        assert!(b.check_and_record_api_call(3_600_001).is_ok());
    }

    #[test]
    fn action_and_api_windows_are_independent() {
        // A tight action cap must not consume api-call budget, and vice versa.
        let mut b = BudgetTracker::new(Some(1)).with_api_calls_per_hour(Some(2));
        assert!(b.check_and_record(0).is_ok()); // action 1/1
        assert!(b.check_and_record(1).is_err()); // action 2 over the per-minute cap
        // The api-call window is untouched by the action attempts above.
        assert!(b.check_and_record_api_call(0).is_ok()); // api 1/2
        assert!(b.check_and_record_api_call(1).is_ok()); // api 2/2
        assert!(b.check_and_record_api_call(2).is_err()); // api 3 over the per-hour cap
    }
}
