//! A bounded, deduplicating map of future work the worker owes itself.
//!
//! `worker.rs` used to spawn one sleeping tokio task per scheduled retry and
//! per observed deadline, with `Worker::cooling` growing one entry per
//! failing task and no cap on either. This module replaces both: the worker
//! keeps one `Deadlines` map and sleeps on the earliest entry in its own
//! `select!`, so there is at most one timer for the whole worker rather than
//! one per call. Durable state stays the source of truth. Anything this map
//! refuses to hold because it is full is not lost: the refusal is
//! remembered as a time to rescan durable state, which finds the work again.

use std::collections::HashMap;
use std::time::Duration;

use tokio::time::Instant;

use super::worker::Task;

/// How many future tasks the worker will remember at once. Past this bound
/// the map stops being a map and starts being an unbounded backlog, so a
/// deadline that does not fit is refused rather than admitted anyway.
pub(crate) const DEADLINES: usize = 1024;

/// The wait after one failure; doubles per consecutive failure up to the
/// ceiling.
///
/// The retry is scheduled rather than left to chance. Compaction would be
/// re-triggered by the next flush anyway, but a deletion and an archive are
/// named only by durable state that nothing else looks at until the next
/// startup, so a transient blob-store failure on either one used to park it
/// until somebody restarted the process.
pub(crate) const BACKOFF: Duration = Duration::from_secs(60);

/// Where the doubling stops. A task failing for an hour is failing for a
/// reason a faster retry will not fix, and the point of the ceiling is that
/// it still retries at all: the state naming the task is durable, so the
/// deployment should recover on its own once whatever broke is fixed,
/// without an operator noticing.
pub(crate) const BACKOFF_CEILING: Duration = Duration::from_secs(60 * 60);

/// The wait after `failures` consecutive failures of one task.
pub(crate) fn backoff(failures: u32) -> Duration {
    BACKOFF
        .saturating_mul(
            1u32.checked_shl(failures.saturating_sub(1))
                .unwrap_or(u32::MAX),
        )
        .min(BACKOFF_CEILING)
}

/// One task's place in the map: when it is next due, how many times in a
/// row it has failed, and whether that deadline has fired yet.
struct Entry {
    due: Instant,
    /// How many consecutive failures this task has recorded. Zero means the
    /// entry exists only because a plain `at` is waiting to fire; nothing
    /// has failed.
    failures: u32,
    /// An armed entry has not fired yet. `due` hands an armed, due entry
    /// back to the caller and disarms or removes it; an unarmed entry is
    /// pure bookkeeping, kept only to remember a failure count across
    /// retries.
    armed: bool,
}

/// A bounded, deduplicating map of work the worker owes itself later.
///
/// Every kind of background work the worker runs is named by durable state
/// (a row over threshold, a label without an archive key, and so on), so
/// this map is not the record of what work exists: it is only a reminder of
/// when to look again, for the two cases where nothing else will ask.
/// Losing an entry is therefore never losing the work, only losing the
/// early wake-up for it; the worst case is the next durable rescan finds it.
/// That is what makes the bound in [`DEADLINES`] safe to enforce by
/// refusing new entries rather than growing without one.
pub(crate) struct Deadlines {
    entries: HashMap<Task, Entry>,
    /// The earliest due time of any deadline this map has refused to hold.
    /// It survives across refusals by keeping the minimum, so the worker
    /// always wakes for the oldest piece of work it turned away rather than
    /// the most recent.
    refusal: Option<Instant>,
    /// How many deadlines this map has refused since the process started.
    /// Exposed for the warning log, not for behavior.
    refused: u64,
}

/// What [`Deadlines::due`] handed back.
pub(crate) struct Due {
    pub(crate) tasks: Vec<Task>,
    /// A deadline this map refused to hold has come due, so the worker owes
    /// itself a fresh durable scan: whatever it forgot when the map was
    /// full is only findable by looking at durable state again.
    pub(crate) rescan: bool,
}

impl Deadlines {
    pub(crate) fn new() -> Self {
        Self {
            entries: HashMap::new(),
            refusal: None,
            refused: 0,
        }
    }

    /// `refusal = min(refusal, due)`, and count it. This is the map's way of
    /// not dropping refused work on the floor: the refusal time is when the
    /// worker will rescan durable state and find it again, so the earliest
    /// refused deadline is the one that matters.
    fn refuse(&mut self, due: Instant) {
        self.refusal = Some(match self.refusal {
            Some(existing) => existing.min(due),
            None => due,
        });
        self.refused += 1;
    }

    /// Record the backoff after `task` failed. Returns the wait it chose, so
    /// the caller can log it. The failure count survives until [`clear`],
    /// which is why a full map still records the failure count on an
    /// existing entry: overwriting an entry is not the same as admitting a
    /// new one, and must never be refused.
    ///
    /// [`clear`]: Deadlines::clear
    pub(crate) fn failed(&mut self, task: Task, now: Instant) -> Duration {
        let failures = self.entries.get(&task).map_or(0, |entry| entry.failures) + 1;
        let wait = backoff(failures);
        let due = now + wait;
        if let Some(entry) = self.entries.get_mut(&task) {
            entry.due = due;
            entry.failures = failures;
            entry.armed = true;
        } else if self.entries.len() == DEADLINES {
            self.refuse(due);
        } else {
            self.entries.insert(
                task,
                Entry {
                    due,
                    failures,
                    armed: true,
                },
            );
        }
        wait
    }

    /// Ask for `task` at `due` and no earlier.
    ///
    /// A task already waiting out a backoff is left alone: an ordinary delay
    /// must not shorten a backoff, or a task that keeps being asked for
    /// while it is failing would never get the slower retry the backoff is
    /// for. Asking again for a task that already has a deadline waiting
    /// pulls that deadline earlier if the new one is sooner, which is the
    /// deduplication that stops repeated observations of one pending
    /// deletion from accumulating timers.
    ///
    /// An entry that has already fired is a different matter, whatever its
    /// failure count: nothing is waiting on it any more, so the new deadline
    /// is what it is worth. A deletion that failed once, was retried, and
    /// then found itself still inside its grace period asks for exactly
    /// that, and refusing it there would leave nothing scheduled at all.
    pub(crate) fn at(&mut self, task: Task, due: Instant) {
        if let Some(entry) = self.entries.get_mut(&task) {
            if entry.armed {
                if entry.failures > 0 {
                    return;
                }
                entry.due = entry.due.min(due);
            } else {
                entry.due = due;
                entry.armed = true;
            }
        } else if self.entries.len() == DEADLINES {
            self.refuse(due);
        } else {
            self.entries.insert(
                task,
                Entry {
                    due,
                    failures: 0,
                    armed: true,
                },
            );
        }
    }

    /// Forget `task`'s failure streak. Called when the task succeeds, so the
    /// next failure (if any) starts counting from one again.
    ///
    /// A deadline the task armed while it was running survives, with its
    /// streak reset. That distinction is load bearing: `Worker::delete`
    /// succeeds by scheduling itself for the end of the document's grace
    /// period, and removing the entry here would throw that wake-up away
    /// and leave the purge waiting for the next durable scan or restart.
    pub(crate) fn clear(&mut self, task: &Task) {
        match self.entries.get_mut(task) {
            Some(entry) if entry.armed => entry.failures = 0,
            _ => {
                self.entries.remove(task);
            }
        }
    }

    /// Forget every reminder for `task`, including an armed deadline. This is
    /// used when durable work completes before its scheduled grace deadline,
    /// so an obsolete reminder cannot wake the worker for a removed row.
    pub(crate) fn remove(&mut self, task: &Task) {
        self.entries.remove(task);
    }

    /// How many times in a row `task` has failed. For the warning log: a
    /// task nobody has recorded a failure for has failed zero times.
    pub(crate) fn failures(&self, task: &Task) -> u32 {
        self.entries.get(task).map_or(0, |entry| entry.failures)
    }

    /// Is `task` waiting out a deadline that has not come due? The worker
    /// drops a duplicate wake-up for such a task instead of defeating the
    /// backoff or the grace period a plain `at` deadline represents.
    pub(crate) fn waiting(&self, task: &Task, now: Instant) -> bool {
        self.entries
            .get(task)
            .is_some_and(|entry| entry.armed && entry.due > now)
    }

    /// Everything that has come due, at most once per arming.
    ///
    /// An entry with no failures is spent once it fires: there is nothing
    /// left to remember about it, so it is removed outright. An entry with
    /// failures survives disarmed, so the failure count is still there for
    /// the next `failed` call, but the same due task is not handed out
    /// twice before something asks for it again.
    pub(crate) fn due(&mut self, now: Instant) -> Due {
        let mut tasks = Vec::new();
        self.entries.retain(|task, entry| {
            if entry.armed && entry.due <= now {
                tasks.push(*task);
                if entry.failures == 0 {
                    return false;
                }
                entry.armed = false;
            }
            true
        });
        // Prune dead bookkeeping: an unarmed entry (nothing scheduled) whose
        // last due time is far enough in the past that it can never again
        // be the deadline a retry lands on. That is a task that failed, was
        // retried, and never came back to fail or succeed again; keeping
        // its failure count for ever would let it alone fill the bound.
        self.entries.retain(|_, entry| {
            entry.armed || now.saturating_duration_since(entry.due) <= BACKOFF_CEILING
        });
        let rescan = match self.refusal {
            Some(at) if at <= now => {
                self.refusal = None;
                true
            }
            _ => false,
        };
        Due { tasks, rescan }
    }

    /// When the worker should next wake, if ever: the earliest of every
    /// armed entry's due time and the earliest refused deadline, since a
    /// refusal owes the worker a rescan just as much as an armed entry owes
    /// it a task.
    pub(crate) fn next(&self) -> Option<Instant> {
        let earliest_armed = self
            .entries
            .values()
            .filter(|entry| entry.armed)
            .map(|entry| entry.due)
            .min();
        match (earliest_armed, self.refusal) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        }
    }

    /// How many tasks are remembered right now (armed or not).
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    /// How many deadlines this map has refused since the process started.
    pub(crate) fn refused(&self) -> u64 {
        self.refused
    }
}
