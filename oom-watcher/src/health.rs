//! Process liveness as the kubelet's probe sees it.
//!
//! `/metrics` used to be the liveness probe, which meant it answered 200 for as long as
//! axum was up — including with the watch loop wedged behind it. [`Health`] is the state
//! that makes the probe mean something: the watch loop stamps a heartbeat on every wakeup,
//! and a stamp that stops advancing is the one failure the process cannot report itself.
//!
//! What this does **not** assert, and cannot: that OOM events are still being delivered.
//! The heartbeat is driven by a ticker inside the loop's `select!`, so it advances whether
//! or not the ring buffer is readable. Nothing cheap can prove the event path end to end —
//! an OOM cannot be synthesized, and a ring buffer that genuinely failed surfaces as an
//! error that ends the task, which `main`'s `select!` already turns into a non-zero exit.
//! What a stale heartbeat does prove is that the loop is no longer being scheduled or is
//! blocked inside event handling.
//!
//! Deliberately axum-free: `watch` stamps through an injected closure and never learns
//! that an HTTP surface exists, the same way it takes an injected clock.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// How often the watch loop stamps its heartbeat when no events are arriving. A quiet node
/// legitimately parks on epoll for hours, so the loop needs its own reason to wake.
pub const HEARTBEAT_INTERVAL_SECONDS: u64 = 30;

/// How stale a heartbeat may get before the probe fails. Three intervals: one missed tick
/// is a busy scheduler, three in a row is a loop that has stopped running.
pub const HEARTBEAT_STALE_AFTER_SECONDS: u64 = 3 * HEARTBEAT_INTERVAL_SECONDS;

/// What the liveness probe should conclude. Rendered to a status code by
/// [`crate::http`], which is the only thing that knows about HTTP.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Liveness {
    /// The watch loop has not started yet — the probe is still attaching, or the pod cache
    /// is still taking its first list. Not an error; the probe's `initialDelaySeconds` and
    /// failure threshold cover it.
    Starting,
    /// The loop stamped a heartbeat recently enough.
    Live { age_seconds: u64 },
    /// The loop has not stamped in [`HEARTBEAT_STALE_AFTER_SECONDS`]. It is alive as a task
    /// — a task that *ended* would have exited the process — but it is not running.
    Stale { age_seconds: u64 },
}

/// The heartbeat the watch loop stamps and the liveness handler reads.
#[derive(Debug, Default)]
pub struct Health {
    started: AtomicBool,
    last_beat_seconds: AtomicU64,
}

impl Health {
    pub fn new() -> Self {
        Self::default()
    }

    /// The watch loop is about to run: the probe is attached and the pod cache has had its
    /// chance to sync. Seeds the heartbeat so a loop that parks immediately on a quiet node
    /// does not read as stale before its first tick.
    pub fn mark_started(&self, now_seconds: u64) {
        self.last_beat_seconds.store(now_seconds, Ordering::Relaxed);
        // Release pairs with the Acquire in `liveness`, so a reader that sees `started`
        // also sees the seed above rather than the zero it was built with.
        self.started.store(true, Ordering::Release);
    }

    /// One watch-loop wakeup, event or tick.
    pub fn beat(&self, now_seconds: u64) {
        self.last_beat_seconds.store(now_seconds, Ordering::Relaxed);
    }

    /// What the probe should conclude at `now_seconds`.
    pub fn liveness(&self, now_seconds: u64) -> Liveness {
        if !self.started.load(Ordering::Acquire) {
            return Liveness::Starting;
        }
        // Saturating: a clock that steps backwards reads as a fresh beat, not a huge age.
        let age_seconds =
            now_seconds.saturating_sub(self.last_beat_seconds.load(Ordering::Relaxed));
        if age_seconds > HEARTBEAT_STALE_AFTER_SECONDS {
            Liveness::Stale { age_seconds }
        } else {
            Liveness::Live { age_seconds }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_starting_until_the_watch_loop_starts() {
        let health = Health::new();

        assert_eq!(health.liveness(1_000), Liveness::Starting);
    }

    #[test]
    fn a_started_loop_that_has_not_ticked_yet_is_live() {
        let health = Health::new();
        health.mark_started(1_000);

        // Seeded by mark_started, so the gap before the first tick is not counted as
        // silence — otherwise a quiet node would fail its probe at startup.
        assert_eq!(health.liveness(1_000), Liveness::Live { age_seconds: 0 });
    }

    #[test]
    fn stays_live_up_to_and_including_the_staleness_limit() {
        let health = Health::new();
        health.mark_started(1_000);

        assert_eq!(
            health.liveness(1_000 + HEARTBEAT_STALE_AFTER_SECONDS),
            Liveness::Live {
                age_seconds: HEARTBEAT_STALE_AFTER_SECONDS
            }
        );
    }

    #[test]
    fn goes_stale_one_second_past_the_limit() {
        let health = Health::new();
        health.mark_started(1_000);

        assert_eq!(
            health.liveness(1_001 + HEARTBEAT_STALE_AFTER_SECONDS),
            Liveness::Stale {
                age_seconds: HEARTBEAT_STALE_AFTER_SECONDS + 1
            }
        );
    }

    #[test]
    fn a_beat_clears_staleness() {
        let health = Health::new();
        health.mark_started(1_000);
        let stale_at = 1_001 + HEARTBEAT_STALE_AFTER_SECONDS;
        assert!(matches!(health.liveness(stale_at), Liveness::Stale { .. }));

        health.beat(stale_at);

        assert_eq!(health.liveness(stale_at), Liveness::Live { age_seconds: 0 });
    }

    #[test]
    fn a_clock_that_steps_backwards_reads_as_a_fresh_beat() {
        let health = Health::new();
        health.mark_started(1_000);

        assert_eq!(health.liveness(900), Liveness::Live { age_seconds: 0 });
    }
}
