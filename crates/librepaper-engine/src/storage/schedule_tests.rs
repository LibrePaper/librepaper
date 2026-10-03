//! Tests for `storage::schedule::Deadlines`, the bounded deduplicating map
//! that replaced one spawned timer per retry and per scheduled deletion or
//! archive (see the module headers of `storage::schedule` and
//! `storage::worker`).
//!
//! Every test here is pure and deterministic: `#[tokio::test(start_paused =
//! true)]` freezes `tokio::time::Instant::now()` at a fixed point and
//! `tokio::time::advance` moves it forward by an exact amount, so nothing
//! here depends on wall-clock timing or a real scheduler. Nothing here
//! touches PostgreSQL; the end-to-end claims about the worker actually
//! purging or compacting durable rows live in `worker_recovery_tests.rs`
//! instead.
//!
//! What this file does not prove: that `Worker::run` wires `Deadlines`
//! correctly into its own `select!`, or that `fire_due` and `process` call
//! `waiting`/`clear` at the right points. Those are properties of
//! `worker.rs`, exercised end to end (with a real channel and a real
//! database) in `worker_recovery_tests.rs`.

use std::time::Duration;

use tokio::time::{advance, Instant};
use uuid::Uuid;

use super::schedule::{backoff, Deadlines, BACKOFF, BACKOFF_CEILING, DEADLINES};
use super::worker::Task;

/// A task that keeps failing has to keep being retried, and has to stop
/// getting faster about it. The ceiling is the part worth pinning: an
/// unbounded doubling reaches days, which is indistinguishable from giving
/// up. Ported verbatim from the `worker.rs` test of the same name, which the
/// other agent deletes as part of this cutover.
#[test]
fn the_backoff_doubles_and_then_stops_doubling() {
    assert_eq!(backoff(1), BACKOFF);
    assert_eq!(backoff(2), BACKOFF * 2);
    assert_eq!(backoff(3), BACKOFF * 4);
    assert_eq!(backoff(7), BACKOFF_CEILING);
    assert_eq!(backoff(1_000), BACKOFF_CEILING);
    // Failure counts start at one; a zero would be a caller's bug, and it
    // must not underflow into the ceiling.
    assert_eq!(backoff(0), BACKOFF);
}

/// The accumulated-timer bug this map exists to fix: two observations of
/// one pending task used to spawn two timers. A repeated `at` must instead
/// keep one entry, and keep the earlier of the two deadlines, since the
/// earlier one is the one that already promised to be honored.
#[tokio::test(start_paused = true)]
async fn a_repeated_at_for_one_task_keeps_one_entry_and_the_earlier_deadline() {
    let mut deadlines = Deadlines::new();
    let task = Task::Compact(Uuid::new_v4());
    let now = Instant::now();

    deadlines.at(task, now + Duration::from_secs(120));
    deadlines.at(task, now + Duration::from_secs(30));
    assert_eq!(
        deadlines.len(),
        1,
        "a repeated observation of the same task must not grow the map"
    );

    // Only the earlier deadline (30s) has passed; the later one (120s) has
    // not. If the map had kept the later deadline instead, this would still
    // be empty.
    advance(Duration::from_secs(31)).await;
    let due = deadlines.due(Instant::now());
    assert_eq!(
        due.tasks,
        vec![task],
        "the earlier of the two deadlines is the one that governs"
    );
}

/// A backoff is a promise not to retry sooner than a computed wait. An
/// ordinary `at` -- the kind a durable scan or a fresh observation issues --
/// must never shorten it, or a task that keeps failing would be retried at
/// the ordinary rate and the backoff would do nothing.
#[tokio::test(start_paused = true)]
async fn at_never_shortens_a_failure_backoff() {
    let mut deadlines = Deadlines::new();
    let task = Task::Compact(Uuid::new_v4());
    let now = Instant::now();

    let wait = deadlines.failed(task, now);
    assert_eq!(wait, BACKOFF);

    // An ordinary `at` asks for the task much sooner than the backoff would.
    deadlines.at(task, now + Duration::from_secs(1));

    advance(Duration::from_secs(2)).await;
    assert!(
        deadlines.due(Instant::now()).tasks.is_empty(),
        "the backoff must not be shortened by an ordinary `at`"
    );

    // Once the real backoff has elapsed, the task is due.
    advance(wait).await;
    assert_eq!(deadlines.due(Instant::now()).tasks, vec![task]);
}

/// Consecutive failures must back off further each time, the failure count
/// must survive a `due` that hands the task out (so the worker can act on
/// it and, if it fails again, back off further still), and `clear` must
/// reset that count once the task finally succeeds.
#[tokio::test(start_paused = true)]
async fn consecutive_failures_grow_the_wait_survive_a_due_and_clear_resets_them() {
    let mut deadlines = Deadlines::new();
    let task = Task::Compact(Uuid::new_v4());
    let now = Instant::now();

    let first_wait = deadlines.failed(task, now);
    assert_eq!(first_wait, BACKOFF);

    advance(first_wait).await;
    let due = deadlines.due(Instant::now());
    assert_eq!(due.tasks, vec![task], "the first backoff comes due");
    assert_eq!(
        deadlines.len(),
        1,
        "the failure count survives being handed out once"
    );

    let second_wait = deadlines.failed(task, Instant::now());
    assert_eq!(
        second_wait,
        BACKOFF * 2,
        "a second consecutive failure backs off further than the first"
    );

    // The retry this second failure armed has not fired, so clearing it
    // cancels the retry outright: the task succeeded, there is nothing left
    // to try again.
    advance(second_wait + Duration::from_secs(1)).await;
    assert_eq!(deadlines.due(Instant::now()).tasks, vec![task]);
    deadlines.clear(&task);
    assert_eq!(deadlines.len(), 0, "a spent entry is forgotten entirely");

    let reset_wait = deadlines.failed(task, Instant::now());
    assert_eq!(
        reset_wait, BACKOFF,
        "clear resets the failure count, so the next failure starts over"
    );
}

/// The case that decides whether a deletion is ever purged. `Worker::delete`
/// succeeds by scheduling itself for the end of the document's grace period,
/// and the worker calls `clear` on every success. If `clear` removed the
/// entry that the task had just armed, nothing would be waiting for the
/// grace to end and the purge would sit there until a restart.
#[tokio::test(start_paused = true)]
async fn clear_keeps_a_deadline_the_task_armed_while_it_ran() {
    let mut deadlines = Deadlines::new();
    let task = Task::Delete(Uuid::new_v4());
    let now = Instant::now();

    // It failed once, the retry came due, and the retry found the document
    // still inside its grace period.
    let wait = deadlines.failed(task, now);
    advance(wait).await;
    assert_eq!(deadlines.due(Instant::now()).tasks, vec![task]);
    deadlines.at(task, Instant::now() + Duration::from_secs(600));
    deadlines.clear(&task);

    assert!(
        deadlines.waiting(&task, Instant::now()),
        "the grace period the task asked for must outlive its own success"
    );
    advance(Duration::from_secs(601)).await;
    assert_eq!(
        deadlines.due(Instant::now()).tasks,
        vec![task],
        "the grace period comes due on its own, with no restart and no rescan"
    );
    assert_eq!(
        deadlines.failures(&task),
        0,
        "the success ended the failure streak even though the deadline stayed"
    );
}

/// An `at` for a task whose retry has already fired is an ordinary request
/// for a future deadline, not an attempt to shorten a backoff: nothing is
/// waiting on that entry any more. Refusing it there would leave the task
/// with no deadline at all.
#[tokio::test(start_paused = true)]
async fn at_arms_an_entry_whose_backoff_has_already_fired() {
    let mut deadlines = Deadlines::new();
    let task = Task::Delete(Uuid::new_v4());

    let wait = deadlines.failed(task, Instant::now());
    advance(wait).await;
    assert_eq!(deadlines.due(Instant::now()).tasks, vec![task]);

    deadlines.at(task, Instant::now() + Duration::from_secs(30));
    assert!(deadlines.waiting(&task, Instant::now()));
    assert_eq!(
        deadlines.failures(&task),
        1,
        "arming a new deadline does not forget that it failed once"
    );
    advance(Duration::from_secs(31)).await;
    assert_eq!(deadlines.due(Instant::now()).tasks, vec![task]);
}

/// `waiting` is the check the worker uses to drop a duplicate wake-up
/// without defeating a backoff or a grace period: true while the deadline
/// has not come due, false once it has.
#[tokio::test(start_paused = true)]
async fn waiting_is_true_before_the_deadline_and_false_after() {
    let mut deadlines = Deadlines::new();
    let task = Task::Compact(Uuid::new_v4());
    let now = Instant::now();

    deadlines.at(task, now + Duration::from_secs(10));
    assert!(deadlines.waiting(&task, Instant::now()));

    advance(Duration::from_secs(11)).await;
    assert!(!deadlines.waiting(&task, Instant::now()));
}

/// `due` must not hand out one arming of a task twice: the second call with
/// nothing having re-armed it in between finds nothing new.
#[tokio::test(start_paused = true)]
async fn due_hands_out_a_task_at_most_once_per_arming() {
    let mut deadlines = Deadlines::new();
    let task = Task::Compact(Uuid::new_v4());
    let now = Instant::now();

    deadlines.at(task, now + Duration::from_secs(5));
    advance(Duration::from_secs(6)).await;

    let first = deadlines.due(Instant::now());
    assert_eq!(first.tasks, vec![task]);

    let second = deadlines.due(Instant::now());
    assert!(
        second.tasks.is_empty(),
        "asking again without a fresh arming must not hand the same task out twice"
    );
}

/// The map is bounded at [`DEADLINES`] entries. Filling it and then asking
/// for one more must not grow it; the refusal must be counted; and once the
/// refused deadline itself comes due, `due().rescan` must fire exactly once,
/// which is the signal the worker uses to fall back on a durable scan for
/// work this map could not remember.
#[tokio::test(start_paused = true)]
async fn a_full_map_refuses_rather_than_growing_and_rescans_once_the_refusal_is_due() {
    let mut deadlines = Deadlines::new();
    let now = Instant::now();

    for _ in 0..DEADLINES {
        deadlines.at(
            Task::Compact(Uuid::new_v4()),
            now + Duration::from_secs(1_000),
        );
    }
    assert_eq!(deadlines.len(), DEADLINES);
    assert_eq!(deadlines.refused(), 0);

    deadlines.at(Task::Compact(Uuid::new_v4()), now + Duration::from_secs(50));
    assert_eq!(
        deadlines.len(),
        DEADLINES,
        "a full map does not grow past its bound"
    );
    assert_eq!(deadlines.refused(), 1, "the refusal is counted");

    advance(Duration::from_secs(51)).await;
    let due = deadlines.due(Instant::now());
    assert!(
        due.rescan,
        "the refused deadline has come due, so a durable rescan is owed"
    );

    let due_again = deadlines.due(Instant::now());
    assert!(
        !due_again.rescan,
        "the rescan bit is consumed by the first due() that observes it"
    );
}

/// The refusal remembers the earliest refused deadline, not the latest, so
/// the worker rescans durable state as soon as the most urgent refused work
/// would have been due, rather than waiting for whichever refusal happened
/// to be recorded last.
#[tokio::test(start_paused = true)]
async fn the_refusal_time_is_the_earliest_refused_deadline_not_the_latest() {
    let mut deadlines = Deadlines::new();
    let now = Instant::now();

    for _ in 0..DEADLINES {
        deadlines.at(
            Task::Compact(Uuid::new_v4()),
            now + Duration::from_secs(1_000),
        );
    }

    // The first refusal is the earlier of the two; the second is later and
    // must not push the remembered refusal time back.
    deadlines.at(
        Task::Compact(Uuid::new_v4()),
        now + Duration::from_secs(200),
    );
    deadlines.at(
        Task::Compact(Uuid::new_v4()),
        now + Duration::from_secs(500),
    );
    assert_eq!(deadlines.refused(), 2);

    advance(Duration::from_secs(201)).await;
    assert!(
        deadlines.due(Instant::now()).rescan,
        "the earliest refused deadline (200s) has passed, so a rescan is owed \
         even though a later refusal (500s) has not come due yet"
    );
}

/// An entry with no armed deadline exists only to remember a failure count.
/// If nothing ever asks for that task again, keeping the count forever would
/// let dead bookkeeping fill the bound on its own, with no live work behind
/// it. `due` prunes such an entry once it is older than [`BACKOFF_CEILING`].
#[tokio::test(start_paused = true)]
async fn an_unarmed_bookkeeping_entry_older_than_the_backoff_ceiling_is_pruned_by_due() {
    let mut deadlines = Deadlines::new();
    let task = Task::Compact(Uuid::new_v4());
    let now = Instant::now();

    let wait = deadlines.failed(task, now);
    advance(wait + Duration::from_secs(1)).await;

    let due = deadlines.due(Instant::now());
    assert_eq!(due.tasks, vec![task], "the first backoff comes due");
    assert_eq!(
        deadlines.len(),
        1,
        "the failure count is kept, unarmed, after being handed out"
    );

    // Nothing re-fails or re-arms the task. Once the unarmed entry is older
    // than the ceiling, it is dead weight and must be pruned.
    advance(BACKOFF_CEILING + Duration::from_secs(1)).await;
    let stale = deadlines.due(Instant::now());
    assert!(stale.tasks.is_empty());
    assert_eq!(
        deadlines.len(),
        0,
        "a dead failure count older than the ceiling cannot be allowed to fill the bound"
    );
}

/// This is the no-idle-polling property in miniature: with nothing armed
/// and nothing refused, there is nothing to wake up for, so the worker's
/// `select!` has no timer branch to build and blocks on its channel alone.
#[test]
fn next_is_none_when_empty() {
    let deadlines = Deadlines::new();
    assert_eq!(deadlines.next(), None);
}

/// `next` is the earliest of every armed deadline and any refusal, which is
/// exactly what the worker sleeps until: whichever comes first is worth
/// waking early for, be it real work or an overdue rescan.
#[tokio::test(start_paused = true)]
async fn next_is_the_earliest_of_an_armed_deadline_and_a_refusal() {
    let mut deadlines = Deadlines::new();
    let now = Instant::now();

    deadlines.at(
        Task::Compact(Uuid::new_v4()),
        now + Duration::from_secs(100),
    );
    assert_eq!(deadlines.next(), Some(now + Duration::from_secs(100)));

    // Fill the map so the next `at` is refused with an earlier deadline
    // than the one already armed above.
    for _ in 0..DEADLINES {
        deadlines.at(
            Task::Compact(Uuid::new_v4()),
            now + Duration::from_secs(1_000),
        );
    }
    deadlines.at(Task::Compact(Uuid::new_v4()), now + Duration::from_secs(10));
    assert_eq!(
        deadlines.next(),
        Some(now + Duration::from_secs(10)),
        "the refusal at 10s is earlier than the armed deadline at 100s"
    );
}
