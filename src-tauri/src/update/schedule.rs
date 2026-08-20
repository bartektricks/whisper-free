//! When an automatic check is due.
//!
//! Pure, so the rule is testable without waiting a day — the same split as
//! [`crate::state::is_valid`] and [`crate::should_unload_refiner`].

use std::time::{Duration, Instant};

/// How long between automatic checks, once the user has switched them on.
///
/// Releases here are cut by hand, a few times a month at most, so anything
/// shorter is asking a question whose answer has not changed.
pub const CHECK_INTERVAL: Duration = Duration::from_hours(24);

/// How often the watchdog wakes to ask whether a check is due.
///
/// Shorter than the interval so that switching the setting on is noticed
/// within half an hour rather than at the next daily boundary. A tick that
/// finds nothing to do takes two locks and no network.
pub const CHECK_POLL: Duration = Duration::from_mins(30);

/// How long after launch the first check may run.
///
/// Nowhere near `RunEvent::Ready`, where `platform::settle_launch_activation`
/// is settling the activation macOS deferred, and late enough that a machine
/// still finishing its login is not competing for the network.
pub const STARTUP_DELAY: Duration = Duration::from_secs(10);

/// Is a check due, given when the last one was attempted?
///
/// `last` records the *attempt*, not the success: a broken endpoint must not
/// turn into a request every time the watchdog ticks.
#[must_use]
pub fn is_check_due(last: Option<Instant>, now: Instant, interval: Duration) -> bool {
    // Never checked is always due — that is the launch check.
    last.is_none_or(|at| now.saturating_duration_since(at) >= interval)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_check_of_a_session_is_always_due() {
        assert!(is_check_due(None, Instant::now(), CHECK_INTERVAL));
    }

    #[test]
    fn a_recent_check_is_not_repeated() {
        let now = Instant::now();
        let recent = now.checked_sub(Duration::from_mins(90)).unwrap_or(now);
        assert!(!is_check_due(Some(recent), now, CHECK_INTERVAL));
    }

    #[test]
    fn a_day_old_check_is_due_again() {
        let now = Instant::now();
        let stale = now
            .checked_sub(CHECK_INTERVAL + Duration::from_mins(1))
            .unwrap_or(now);
        assert!(is_check_due(Some(stale), now, CHECK_INTERVAL));
    }

    #[test]
    fn the_boundary_itself_is_due() {
        let now = Instant::now();
        let exactly = now.checked_sub(CHECK_INTERVAL).unwrap_or(now);
        assert!(is_check_due(Some(exactly), now, CHECK_INTERVAL));
    }

    #[test]
    fn a_clock_that_went_backwards_does_not_check_forever() {
        // `Instant` is monotonic, so this cannot really happen — but
        // `saturating_duration_since` is what makes it a no-op rather than a
        // panic, and the test is what keeps that call from being simplified
        // away into a subtraction the arithmetic lint would reject anyway.
        let now = Instant::now();
        let future = now + Duration::from_mins(5);
        assert!(!is_check_due(Some(future), now, CHECK_INTERVAL));
    }

    #[test]
    fn the_watchdog_ticks_more_often_than_it_checks() {
        // Otherwise turning the setting on would not be noticed until the next
        // daily boundary, which reads as the button doing nothing.
        assert!(CHECK_POLL < CHECK_INTERVAL);
    }
}
