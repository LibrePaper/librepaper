// The automatic sync scheduler. While a project window is open it checks the
// remote on a timer, pulls when the remote moved and pushes browser edits after
// a quiet period. It has no Svelte, no DOM and no app imports: the Git
// operations and the clock are injected, so the pacing can be tested alone.
//
// `ops` is { status, recover, check, pull, push }, each async and each allowed
// to throw. Their answers are the result shapes of sync.js.

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
const INTERRUPTED = "A sync was interrupted and could not be finished yet.";

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
  let again = false;
  let failures = 0;
  let dirty = false;
  let lastEditAt = null;
  let firstEditAt = null;

  const snapshot = () => ({ mode, kind, reason, lastPullAt, lastPushAt, nextAt });
  const emit = () => onChange(snapshot());

  /// The time a push becomes due, or null when nothing is waiting to go out.
  function pushDueAt() {
    if (!dirty) return null;
    const quiet = lastEditAt == null ? -Infinity : lastEditAt + pace.quietMs;
    const gap = lastPushAt == null ? -Infinity : lastPushAt + pace.minPushGapMs;
    const first = firstEditAt == null ? clock.now() : firstEditAt;
    return Math.min(Math.max(quiet, gap), first + pace.maxWaitMs);
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
    const due = pushDueAt();
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
    if (answer.kind === "auth") return { type: "paused", kind: "auth", reason: "Git could not sign in." };
    return { type: "paused", kind: "refused", reason: why || "Automatic sync stopped." };
  }

  const conflicted = () => ({ type: "paused", kind: "conflict", reason: "Conflicts need resolution." });

  async function attempts() {
    for (let attempt = 0; attempt < MAX_ATTEMPTS; attempt++) {
      const c = await ops.check();
      if (c.status === "refused" || c.status === "unknown") return refusal(c);
      if (c.status === "behind") {
        const p = await ops.pull();
        if (p.status === "pulled") {
          lastPullAt = clock.now();
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
      if (answer.status === "pushed") {
        lastPushAt = clock.now();
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
      break;
    }
    return { type: "idle" };
  }

  async function body() {
    let st = await ops.status();
    if (st.pending?.length) {
      const rec = await ops.recover();
      if (rec.status === "refused") return refusal(rec);
      if (rec.pending?.length) return waiting(INTERRUPTED);
      st = { conflict: rec.conflict, pending: [] };
    }
    if (st.conflict) return conflicted();
    return attempts();
  }

  async function round() {
    if (stopped) return;
    if (running) {
      again = true;
      return;
    }
    running = true;
    mode = "running";
    emit();
    let outcome;
    try {
      outcome = await body();
    } catch (err) {
      outcome = waiting(err?.message || String(err));
    }
    running = false;
    if (stopped) return;
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
