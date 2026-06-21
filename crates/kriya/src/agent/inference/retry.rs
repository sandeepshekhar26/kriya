//! Bounded retry with exponential backoff around a single inference step (R10).
//!
//! Why this lives behind its own module: an inference backend that reaches a model
//! (Ollama, claude-cli, Anthropic) can fail *transiently* — a network blip, a rate-limit
//! response, or a parse hiccup on a momentarily-malformed completion. Failing the whole run
//! on the first such error makes a regulated workstation host needlessly brittle. Instead we
//! retry the step a bounded number of times with growing backoff, and only give up — cleanly,
//! never by hanging or panicking — once the budget is exhausted. The caller (the host loop)
//! turns that clean give-up into a graceful `AgentDone` + error log.
//!
//! Deterministic/scripted backends never return `Err`, so they take the success path on the
//! first attempt and this module changes nothing about their behavior (no sleeps, no extra
//! calls). The retry only ever engages on a real transient failure.

use std::time::Duration;

use super::{Inference, StepContext, StepDecision};

/// How a transient backend error is retried. Sane defaults retry a few times with a short,
/// exponentially-growing backoff; both knobs are configurable from policy so a flaky-but-cheap
/// local model and an expensive rate-limited cloud model can be tuned differently.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryPolicy {
    /// Number of *retries* after the first attempt. `0` disables retrying (one attempt only).
    /// Total attempts = `max_retries + 1`.
    pub max_retries: u32,
    /// Backoff before the first retry. Each subsequent retry doubles it (capped at `max_backoff`).
    pub initial_backoff: Duration,
    /// Upper bound on a single backoff wait, so a large `max_retries` can't blow up the delay.
    pub max_backoff: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        // Three retries (four attempts total) starting at 250ms — long enough to ride out a
        // brief blip or a single rate-limit, short enough that a genuinely-down backend escalates
        // in well under the host's per-step result timeout. Doubling caps at 5s.
        Self {
            max_retries: 3,
            initial_backoff: Duration::from_millis(250),
            max_backoff: Duration::from_secs(5),
        }
    }
}

impl RetryPolicy {
    /// A policy that never retries — one attempt, fail fast. Useful for tests and for callers
    /// that want the old bare-`?` behavior back.
    pub fn no_retry() -> Self {
        Self { max_retries: 0, ..Self::default() }
    }

    /// The backoff to wait *before* the given retry attempt (1-based: retry #1 waits
    /// `initial_backoff`, retry #2 waits `2×`, …), clamped to `max_backoff`.
    pub fn backoff_for_retry(&self, retry: u32) -> Duration {
        // retry is 1-based; shift so the first retry uses initial_backoff (2^0). Cap the shift so
        // the doubling factor can't overflow a u32 before we clamp to max_backoff.
        let shift = retry.saturating_sub(1).min(31);
        let factor = 1u32 << shift;
        let scaled = self.initial_backoff.saturating_mul(factor);
        scaled.min(self.max_backoff)
    }
}

/// Run one inference step with bounded retry + backoff.
///
/// Returns `Ok(decision)` on the first attempt that succeeds. If every attempt (the initial one
/// plus `policy.max_retries` retries) fails, returns `Err(RetryExhausted)` carrying the count and
/// the last error — the signal the host turns into a clean escalation/abort. **Never panics and
/// always terminates**: the attempt count is bounded by construction.
///
/// `sleep` and `on_retry` are injected so the loop is testable with zero real delay and so the
/// host can log each retry. Production passes `thread::sleep`; tests pass a no-op recorder.
pub fn next_step_with_retry(
    backend: &mut dyn Inference,
    ctx: &StepContext,
    policy: &RetryPolicy,
    mut on_retry: impl FnMut(&RetryAttempt),
    mut sleep: impl FnMut(Duration),
) -> Result<StepDecision, RetryExhausted> {
    let mut last_error = String::new();
    // attempt 0 is the initial try; 1..=max_retries are the retries.
    for attempt in 0..=policy.max_retries {
        match backend.next_step(ctx) {
            Ok(decision) => return Ok(decision),
            Err(err) => {
                last_error = err;
                // If we still have retries left, back off and report; otherwise fall through
                // to exhaustion below.
                let retry_number = attempt + 1; // the retry we are *about* to make (1-based)
                if retry_number <= policy.max_retries {
                    let backoff = policy.backoff_for_retry(retry_number);
                    on_retry(&RetryAttempt {
                        retry_number,
                        max_retries: policy.max_retries,
                        backoff,
                        error: &last_error,
                    });
                    sleep(backoff);
                }
            }
        }
    }
    Err(RetryExhausted { attempts: policy.max_retries + 1, last_error })
}

/// Reported to `on_retry` before each backoff wait, so the host can log a transient failure
/// (and the operator can see the backend is being retried rather than the run silently stalling).
#[derive(Debug)]
pub struct RetryAttempt<'a> {
    /// 1-based index of the retry about to be made.
    pub retry_number: u32,
    pub max_retries: u32,
    /// How long the host will wait before this retry.
    pub backoff: Duration,
    /// The error that triggered this retry.
    pub error: &'a str,
}

/// Returned when every attempt failed. The host escalates this into a graceful end-of-run
/// rather than propagating it as a hard error that wedges or crashes the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetryExhausted {
    /// Total attempts made (initial + retries).
    pub attempts: u32,
    /// The error from the final attempt.
    pub last_error: String,
}

impl std::fmt::Display for RetryExhausted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "inference backend failed after {} attempt(s): {}",
            self.attempts, self.last_error
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NetworkProfile;
    use serde_json::Value;
    use std::cell::Cell;

    /// A backend that returns `Err` the first `fail_n` times it is called, then succeeds with a
    /// `Done` decision. Models a transient backend (blip / rate-limit / parse hiccup).
    struct FlakyBackend {
        fail_n: u32,
        calls: u32,
    }

    impl FlakyBackend {
        fn new(fail_n: u32) -> Self {
            Self { fail_n, calls: 0 }
        }
    }

    impl Inference for FlakyBackend {
        fn name(&self) -> &'static str {
            "flaky-test"
        }
        fn network_profile(&self) -> NetworkProfile {
            NetworkProfile::None
        }
        fn next_step(&mut self, _ctx: &StepContext) -> Result<StepDecision, String> {
            self.calls += 1;
            if self.calls <= self.fail_n {
                Err(format!("transient blip #{}", self.calls))
            } else {
                Ok(StepDecision::Done { summary: "recovered".into() })
            }
        }
    }

    fn ctx<'a>() -> StepContext<'a> {
        StepContext { goal: "", state: &Value::Null, tools: &[], history: &[], recent_memory: &[] }
    }

    #[test]
    fn succeeds_on_first_attempt_without_sleeping_or_retrying() {
        // Deterministic-backend equivalent: never errors → no retries, no sleeps, no on_retry.
        let mut backend = FlakyBackend::new(0);
        let slept = Cell::new(0u32);
        let retried = Cell::new(0u32);
        let out = next_step_with_retry(
            &mut backend,
            &ctx(),
            &RetryPolicy::default(),
            |_| retried.set(retried.get() + 1),
            |_| slept.set(slept.get() + 1),
        );
        assert!(matches!(out, Ok(StepDecision::Done { .. })));
        assert_eq!(backend.calls, 1, "no retries on a clean backend");
        assert_eq!(slept.get(), 0, "no backoff waits when the first attempt succeeds");
        assert_eq!(retried.get(), 0, "on_retry never fires on the success path");
    }

    #[test]
    fn retries_then_succeeds_within_budget() {
        // Fails twice, then succeeds — within a 3-retry budget. Two backoff waits, two on_retry.
        let mut backend = FlakyBackend::new(2);
        let slept = Cell::new(0u32);
        let retried = Cell::new(0u32);
        let out = next_step_with_retry(
            &mut backend,
            &ctx(),
            &RetryPolicy::default(),
            |_| retried.set(retried.get() + 1),
            |_| slept.set(slept.get() + 1),
        );
        assert!(matches!(out, Ok(StepDecision::Done { .. })), "should recover, got {out:?}");
        assert_eq!(backend.calls, 3, "two failures + one success");
        assert_eq!(slept.get(), 2, "one backoff per failed attempt before the success");
        assert_eq!(retried.get(), 2);
    }

    #[test]
    fn exhausts_and_reports_the_last_error_when_always_failing() {
        // Always fails → exhaustion after initial + max_retries attempts, carrying the last error.
        let mut backend = FlakyBackend::new(u32::MAX);
        let policy = RetryPolicy { max_retries: 3, ..RetryPolicy::default() };
        let out = next_step_with_retry(&mut backend, &ctx(), &policy, |_| {}, |_| {});
        let err = out.expect_err("always-failing backend must exhaust");
        assert_eq!(err.attempts, 4, "1 initial + 3 retries");
        assert_eq!(backend.calls, 4);
        assert!(err.last_error.contains("transient blip #4"), "carries last error: {err:?}");
    }

    #[test]
    fn no_retry_policy_makes_a_single_attempt() {
        // max_retries: 0 → fail-fast, exactly one call, no backoff.
        let mut backend = FlakyBackend::new(u32::MAX);
        let slept = Cell::new(0u32);
        let out = next_step_with_retry(
            &mut backend,
            &ctx(),
            &RetryPolicy::no_retry(),
            |_| {},
            |_| slept.set(slept.get() + 1),
        );
        let err = out.expect_err("no-retry + failing backend must error");
        assert_eq!(err.attempts, 1);
        assert_eq!(backend.calls, 1);
        assert_eq!(slept.get(), 0);
    }

    #[test]
    fn backoff_grows_exponentially_and_caps() {
        let policy = RetryPolicy {
            max_retries: 10,
            initial_backoff: Duration::from_millis(100),
            max_backoff: Duration::from_millis(800),
        };
        assert_eq!(policy.backoff_for_retry(1), Duration::from_millis(100));
        assert_eq!(policy.backoff_for_retry(2), Duration::from_millis(200));
        assert_eq!(policy.backoff_for_retry(3), Duration::from_millis(400));
        // Capped at max_backoff thereafter (would be 800, 1600, … uncapped).
        assert_eq!(policy.backoff_for_retry(4), Duration::from_millis(800));
        assert_eq!(policy.backoff_for_retry(9), Duration::from_millis(800));
        // A very large retry index must not overflow — it stays at the cap.
        assert_eq!(policy.backoff_for_retry(u32::MAX), Duration::from_millis(800));
    }
}
