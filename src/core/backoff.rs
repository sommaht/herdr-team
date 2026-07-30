//! A retry schedule: doubling delays that never overrun their budget.
//!
//! Knows nothing about herdr, agents, or panes — it is `Duration` arithmetic, which is why it sits
//! here rather than beside the retry that currently uses it. What a caller retries, and how long it is
//! willing to, stays the caller's policy; this only spells the waiting.

use std::time::Duration;

// =====================================================================================================================
// Constants
// =====================================================================================================================

/// The first delay, in milliseconds.
const FIRST_MS: u64 = 250;

/// The longest a single delay waits, in milliseconds.
///
/// Capped so a long budget spends itself on more attempts rather than on fewer, longer sleeps: a
/// caller waiting ten seconds wants to notice the moment its target is ready, not two seconds later.
const CAP_MS: u64 = 2_000;

// =====================================================================================================================
// Backoff
// =====================================================================================================================

/// The delays a retry spends waiting, doubling and capped, truncated so the total lands exactly on the
/// budget rather than overrunning it.
///
/// An iterator rather than a collected `Vec`, because the sequence is state and a step's length
/// depends on what is left — expressing it as `next` puts the doubling, the cap, and the truncation in
/// one place, and a caller consumes it lazily inside its retry loop rather than paying for a schedule
/// it will usually abandon on the second attempt.
///
/// ```ignore
/// for delay in Backoff::within(10_000) {
///     match attempt() {
///         Ok(value) => return Ok(value),
///         Err(error) if error.is_retryable() => std::thread::sleep(delay),
///         Err(error) => return Err(error),
///     }
/// }
/// ```
///
/// Note what the loop above does *not* do: an exhausted schedule ends the loop without having made a
/// final attempt. A caller that wants one attempt on a zero budget makes it after the loop, which is
/// the honest place for it — the schedule describes waiting, not trying.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Backoff {
    /// Budget not yet handed out; the sequence ends when it reaches zero.
    remaining_ms: u64,
    /// The delay this step would like, before it is truncated to what remains.
    next_ms: u64,
}

impl Backoff {
    /// The schedule that fits inside `budget_ms`.
    ///
    /// A zero budget yields nothing, which is the empty schedule rather than an error: "do not wait"
    /// is a coherent thing for a caller to ask for.
    pub fn within(budget_ms: u64) -> Self {
        Self {
            remaining_ms: budget_ms,
            next_ms: FIRST_MS,
        }
    }
}

impl Iterator for Backoff {
    type Item = Duration;

    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining_ms == 0 {
            return None;
        }
        let step = self.next_ms.min(self.remaining_ms);
        self.remaining_ms -= step;
        self.next_ms = self.next_ms.saturating_mul(2).min(CAP_MS);
        Some(Duration::from_millis(step))
    }
}

// =====================================================================================================================
// Tests
// =====================================================================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn delays(budget_ms: u64) -> Vec<u64> {
        Backoff::within(budget_ms)
            .map(|delay| u64::try_from(delay.as_millis()).expect("a delay fits its own budget"))
            .collect()
    }

    #[test]
    fn the_schedule_backs_off_and_lands_exactly_on_its_budget() {
        // 250ms doubling to a 2000ms cap, with the last delay shortened so the total never overruns.
        assert_eq!(delays(10_000), [250, 500, 1000, 2000, 2000, 2000, 2000, 250]);
        assert_eq!(delays(10_000).iter().sum::<u64>(), 10_000);
    }

    #[test]
    fn a_zero_budget_is_the_empty_schedule() {
        assert_eq!(Backoff::within(0).next(), None);
    }

    #[test]
    fn a_budget_shorter_than_one_step_is_spent_in_one_wait() {
        assert_eq!(delays(100), [100]);
        assert_eq!(delays(300), [250, 50]);
    }

    /// The schedule is lazy: a retry loop that succeeds early pays for no delay it did not use.
    #[test]
    fn the_schedule_is_produced_a_step_at_a_time() {
        let mut schedule = Backoff::within(10_000);

        assert_eq!(schedule.next(), Some(Duration::from_millis(250)));
        assert_eq!(schedule.next(), Some(Duration::from_millis(500)));
        // The remaining 9_250ms is never computed, because nothing asked for it.
    }

    #[test]
    fn every_delay_stays_inside_the_cap() {
        // A long budget spends itself on more attempts rather than on fewer, longer sleeps.
        assert!(delays(600_000).into_iter().all(|delay| delay <= CAP_MS));
    }
}
