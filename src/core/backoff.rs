//! A retry schedule: doubling delays that never overrun their budget.

use std::time::Duration;

// =====================================================================================================================
// Constants
// =====================================================================================================================

/// The first delay, in milliseconds.
const FIRST_MS: u64 = 250;

/// The longest a single delay waits, in milliseconds.
///
/// Capped so a long budget buys more attempts rather than longer sleeps.
const CAP_MS: u64 = 2_000;

// =====================================================================================================================
// Backoff
// =====================================================================================================================

/// The delays a retry spends waiting: doubling, capped, and truncated so the total lands exactly on
/// the budget.
///
/// The schedule describes waiting, not trying — an exhausted schedule ends a retry loop without a
/// final attempt, so a caller wanting one attempt on a zero budget makes it after the loop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Backoff {
    /// Budget not yet handed out; the sequence ends when it reaches zero.
    remaining_ms: u64,
    /// The delay this step would like, before it is truncated to what remains.
    next_ms: u64,
}

impl Backoff {
    /// The schedule that fits inside `budget_ms`; a zero budget is the empty schedule.
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

    #[test]
    fn the_schedule_is_produced_a_step_at_a_time() {
        let mut schedule = Backoff::within(10_000);

        assert_eq!(schedule.next(), Some(Duration::from_millis(250)));
        assert_eq!(schedule.next(), Some(Duration::from_millis(500)));
    }

    #[test]
    fn every_delay_stays_inside_the_cap() {
        assert!(delays(600_000).into_iter().all(|delay| delay <= CAP_MS));
    }
}
