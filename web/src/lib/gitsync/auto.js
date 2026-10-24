// The automatic sync scheduler. While a project window is open it checks the
// remote on a timer, pulls when the remote moved and pushes browser edits after
// a quiet period. It has no Svelte, no DOM and no app imports: the Git
// operations and the clock are injected, so the pacing can be tested alone.
//
// `ops` is { status, recover, check, pull, push }, each async and each allowed
// to throw. Their answers are the result shapes of sync.js. `recover` also
// answers `blocking`, the leftover entries of the current connection, and
// `needsPull`, true when each of those can only be settled by a fresh
// successful pull, and the round then pulls instead of waiting.

export const PACE = Object.freeze({
  checkMs: 120_000,
  quietMs: 180_000,
  minPushGapMs: 600_000,
  maxWaitMs: 1_800_000,
  backoffMs: [60_000, 120_000, 240_000, 480_000, 960_000, 1_800_000],
});

const realClock = {
  now: () => Date.now(),
  setTimeout: (fn, ms) => globalThis.setTimeout(fn, ms),
  clearTimeout: (handle) => globalThis.clearTimeout(handle),
};

const MAX_ATTEMPTS = 3;
const ABORTED = { type: "aborted" };
const INTERRUPTED = "A sync was interrupted and could not be finished yet.";

/**
 * @param {{ ops: Record<string, () => Promise<any>>, clock?: typeof realClock, pace?: typeof PACE, onChange?: (state: object) => void }} options
 */
export function createAutoSync({ ops, clock = realClock, pace = PACE, onChange = () => {} }) {
  let mode = "off";
  let kind = "";
  let reason = "";
  let lastPullAt = null;
  let lastPushAt = null;
  let nextAt = null;
  let timer = null;
  let stopped = true;
  let running = false;
  // Start and stop each move the generation, so a round that outlives its run
  // can tell and leave the new run alone.
  let gen = 0;
  let again = false;
  let failures = 0;
  let dirty = false;
  let lastEditAt = null;
  let firstEditAt = null;
  // A push that did not land (the branch kept moving) waits a check interval
  // before the next try, so a busy branch never makes a tight loop.
  let retryAt = null;

  const snapshot = () => ({ mode, kind, reason, lastPullAt, lastPushAt, nextAt });
  const emit = () => onChange(snapshot());

  /// The time a push becomes due, or null when nothing is waiting to go out.
  function pushDueAt() {
    if (!dirty) return null;
    const quiet = lastEditAt == null ? -Infinity : lastEditAt + pace.quietMs;
    const gap = lastPushAt == null ? -Infinity : lastPushAt + pace.minPushGapMs;
    const first = firstEditAt == null ? clock.now() : firstEditAt;
    const due = Math.min(Math.max(quiet, gap), first + pace.maxWaitMs);
    return retryAt == null ? due : Math.max(due, retryAt);
  }

  function pushDue() {
    const due = pushDueAt();
    return due != null && clock.now() >= due;
  }

  /// The time of the next round, planned from now.
  function plan() {
    const now = clock.now();
    if (mode === "waiting") {
      const steps = pace.backoffMs;
      return now + steps[Math.min(Math.max(failures - 1, 0), steps.length - 1)];
    }
    // A paused scheduler cannot push, so only its next check matters.
    const due = mode === "paused" ? null : pushDueAt();
    return Math.max(now, due == null ? now + pace.checkMs : Math.min(now + pace.checkMs, due));
  }

  function arm(at) {
    if (timer != null) clock.clearTimeout(timer);
    nextAt = at;
    timer = clock.setTimeout(fire, Math.max(0, at - clock.now()));
  }

  function schedule() {
    arm(plan());
    emit();
  }

  function fire() {
    timer = null;
    nextAt = null;
    round();
  }

  function restore(first) {
    dirty = true;
    firstEditAt = firstEditAt == null ? first : Math.min(firstEditAt, first);
  }

  function waiting(why) {
    return { type: "waiting", reason: why };
  }

  function refusal(answer) {
    const why = answer.reason || answer.refusals?.[0]?.reason;
    if (answer.kind === "network") return waiting(why || "The remote could not be reached.");
    // The page is still receiving the document, which settles by itself.
    if (answer.kind === "catching-up") return waiting(why);
    if (answer.kind === "auth") return { type: "paused", kind: "auth", reason: "Git could not sign in." };
    return { type: "paused", kind: "refused", reason: why || "Automatic sync stopped." };
  }

  const conflicted = () => ({ type: "paused", kind: "conflict", reason: "Conflicts need resolution." });

  // `alive` is false once the run that began the round has stopped or been
  // replaced. Every awaited step checks it before the next one starts.
  async function attempts(alive) {
    for (let attempt = 0; attempt < MAX_ATTEMPTS; attempt++) {
      const c = await ops.check();
      if (!alive()) return ABORTED;
      if (c.status === "refused" || c.status === "unknown") return refusal(c);
      if (c.status === "behind") {
        const p = await ops.pull();
        if (!alive()) return ABORTED;
        // A pull that changed no file can still move the checkpoint, so it
        // counts as a pull and the panel reloads. A pull can also make a local
        // file pushable without an edit (a rule that ignored it went away), so
        // the next push looks again.
        if (p.status === "pulled" || p.status === "nothing") {
          lastPullAt = clock.now();
          restore(lastPullAt);
          emit();
        } else if (p.status === "conflict") {
          return conflicted();
        } else if (p.status === "refused") {
          return refusal(p);
        }
      }
      if (!dirty || !pushDue()) break;
      const first = firstEditAt == null ? clock.now() : firstEditAt;
      dirty = false;
      firstEditAt = null;
      let answer;
      try {
        answer = await ops.push();
      } catch (err) {
        restore(first);
        throw err;
      }
      if (!alive()) return ABORTED;
      if (answer.status === "pushed") {
        lastPushAt = clock.now();
        retryAt = null;
        break;
      }
      if (answer.status === "behind") {
        restore(first);
        continue;
      }
      if (answer.status === "moved") {
        restore(first);
        break;
      }
      if (answer.status === "refused") {
        restore(first);
        return refusal(answer);
      }
      // The pull inside the push found a conflict. The edits stay pending, so
      // content kept by the resolution still goes out once sync resumes.
      if (answer.status === "conflict") {
        restore(first);
        return conflicted();
      }
      retryAt = null;
      break;
    }
    if (pushDue()) retryAt = clock.now() + pace.checkMs;
    return { type: "idle" };
  }

  async function body(alive) {
    let st = await ops.status();
    if (!alive()) return ABORTED;
    if (st.pending?.length) {
      const rec = await ops.recover();
      if (!alive()) return ABORTED;
      if (rec.status === "refused") return refusal(rec);
      // Entries of another connection wait for that connection and do not
      // hold up this one.
      if (rec.blocking?.length) {
        if (!rec.needsPull) return waiting(INTERRUPTED);
        const p = await ops.pull();
        if (!alive()) return ABORTED;
        if (p.status === "conflict") return conflicted();
        if (p.status === "refused") return refusal(p);
        if (p.status === "pulled" || p.status === "nothing") {
          lastPullAt = clock.now();
          emit();
        }
        st = { conflict: null, pending: [] };
      } else {
        st = { conflict: rec.conflict, pending: [] };
      }
    }
    if (st.conflict) return conflicted();
    return attempts(alive);
  }

  async function round() {
    if (stopped) return;
    if (running) {
      again = true;
      return;
    }
    running = true;
    const mine = gen;
    const alive = () => !stopped && mine === gen;
    mode = "running";
    emit();
    let outcome;
    try {
      outcome = await body(alive);
    } catch (err) {
      outcome = waiting(err?.message || String(err));
    }
    running = false;
    if (!alive()) {
      // A start that came while this round was still settling asked for a
      // round of its own through `again`; it runs now, never alongside this one.
      if (!stopped && again) {
        again = false;
        round();
      }
      return;
    }
    if (outcome.type === "idle") {
      failures = 0;
      mode = "idle";
      kind = "";
      reason = "";
    } else if (outcome.type === "paused") {
      mode = "paused";
      kind = outcome.kind;
      reason = outcome.reason;
    } else {
      failures += 1;
      mode = "waiting";
      kind = "";
      reason = outcome.reason;
    }
    if (again) {
      again = false;
      emit();
      round();
      return;
    }
    schedule();
  }

  return {
    start() {
      if (!stopped) return;
      stopped = false;
      gen += 1;
      mode = "idle";
      kind = "";
      reason = "";
      failures = 0;
      dirty = true;
      lastEditAt = null;
      firstEditAt = clock.now();
      round();
    },
    stop() {
      stopped = true;
      gen += 1;
      again = false;
      if (timer != null) clock.clearTimeout(timer);
      timer = null;
      nextAt = null;
      mode = "off";
      kind = "";
      reason = "";
      emit();
    },
    edited() {
      if (stopped) return;
      const now = clock.now();
      dirty = true;
      lastEditAt = now;
      if (firstEditAt == null) firstEditAt = now;
      if (running) return;
      const at = plan();
      if (nextAt == null || at < nextAt) arm(at);
      emit();
    },
    poke() {
      if (stopped) return;
      if (timer != null) clock.clearTimeout(timer);
      timer = null;
      nextAt = null;
      round();
    },
    get state() {
      return snapshot();
    },
  };
}
