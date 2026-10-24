// The automatic sync scheduler, driven by a fake clock and scripted operations.

import assert from "node:assert/strict";

import { createAutoSync, PACE } from "../../src/lib/gitsync/auto.js";

const T0 = 1_000_000;
const settle = async () => {
  for (let i = 0; i < 20; i++) await new Promise((resolve) => setImmediate(resolve));
};

function fakeClock() {
  let now = T0;
  let seq = 0;
  let timers = [];
  const clock = {
    now: () => now,
    setTimeout(fn, ms) {
      const handle = ++seq;
      timers.push({ handle, at: now + ms, fn });
      return handle;
    },
    clearTimeout(handle) {
      timers = timers.filter((t) => t.handle !== handle);
    },
    settle,
    async advance(ms) {
      const target = now + ms;
      await settle();
      for (;;) {
        const due = timers.filter((t) => t.at <= target).sort((a, b) => a.at - b.at || a.handle - b.handle)[0];
        if (!due) break;
        timers = timers.filter((t) => t !== due);
        now = Math.max(now, due.at);
        due.fn();
        await settle();
      }
      now = target;
      await settle();
    },
    async to(offset) {
      await clock.advance(T0 + offset - now);
    },
    pending: () => timers.length,
  };
  return clock;
}

const DEFAULTS = {
  status: { conflict: null, pending: [] },
  recover: { pending: [], conflict: null },
  check: { status: "current" },
  pull: { status: "pulled" },
  push: { status: "pushed" },
};

// Each script is a function of the call index, a value, or an array whose last
// element repeats.
function fakeOps(clock, scripts = {}) {
  const calls = [];
  const counts = {};
  let inFlight = 0;
  let maxInFlight = 0;
  const ops = {};
  for (const name of Object.keys(DEFAULTS)) {
    ops[name] = async () => {
      const n = counts[name] ?? 0;
      counts[name] = n + 1;
      calls.push({ name, at: clock.now() });
      inFlight += 1;
      maxInFlight = Math.max(maxInFlight, inFlight);
      try {
        const script = scripts[name] ?? DEFAULTS[name];
        const answer = typeof script === "function" ? script(n) : Array.isArray(script) ? script[Math.min(n, script.length - 1)] : script;
        return await answer;
      } finally {
        inFlight -= 1;
      }
    };
  }
  return {
    ops,
    calls,
    names: () => calls.map((c) => c.name),
    count: (name) => calls.filter((c) => c.name === name).length,
    maxInFlight: () => maxInFlight,
  };
}

function setup(scripts, extra = {}) {
  const clock = fakeClock();
  const fake = fakeOps(clock, scripts);
  const sync = createAutoSync({ ops: fake.ops, clock, ...extra });
  return { clock, fake, sync };
}

const refused = (kind, reason) => ({ status: "refused", kind, refusals: [{ reason }] });

// 1. Start runs status, check and push in order, and a current check skips the pull.
{
  const { clock, fake, sync } = setup();
  sync.start();
  await clock.advance(0);
  assert.deepEqual(fake.names(), ["status", "check", "push"]);
  assert.equal(sync.state.mode, "idle");
  assert.equal(sync.state.lastPushAt, T0);
  assert.equal(sync.state.nextAt, T0 + PACE.checkMs);
  sync.stop();
}

// 2. A behind check pulls and records the pull time.
{
  const { clock, fake, sync } = setup({ check: { status: "behind" } });
  sync.start();
  await clock.advance(0);
  assert.deepEqual(fake.names(), ["status", "check", "pull", "push"]);
  assert.equal(sync.state.lastPullAt, T0);
  sync.stop();
}

// 3. After a push, an edit waits for the quiet period.
{
  const { clock, fake, sync } = setup();
  sync.start();
  await clock.advance(0);
  await clock.to(700_000);
  assert.equal(fake.count("push"), 1);
  sync.edited();
  await clock.to(700_000 + PACE.quietMs - 1_000);
  assert.equal(fake.count("push"), 1);
  await clock.to(700_000 + PACE.quietMs);
  assert.equal(fake.count("push"), 2);
  sync.stop();
}

// 4. A second batch soon after a push waits for the minimum gap.
{
  const { clock, fake, sync } = setup();
  sync.start();
  await clock.advance(0);
  await clock.to(130_000);
  sync.edited();
  await clock.to(PACE.minPushGapMs - 1_000);
  assert.equal(fake.count("push"), 1);
  await clock.to(PACE.minPushGapMs);
  assert.equal(fake.count("push"), 2);
  assert.equal(sync.state.lastPushAt, T0 + PACE.minPushGapMs);
  sync.stop();
}

// 5. Constant editing still pushes within the maximum wait.
{
  const { clock, fake, sync } = setup({ push: [{ status: "nothing" }, { status: "pushed" }] });
  sync.start();
  await clock.advance(0);
  assert.equal(fake.count("push"), 1);
  await clock.to(60_000);
  sync.edited();
  const deadline = 60_000 + PACE.maxWaitMs;
  for (let t = 120_000; t < deadline; t += 60_000) {
    await clock.to(t);
    sync.edited();
  }
  await clock.to(deadline - 1_000);
  assert.equal(fake.count("push"), 1);
  await clock.to(deadline);
  assert.equal(fake.count("push"), 2);
  sync.stop();
}

// 6. A behind push pulls and pushes again, at most three pushes in a round.
{
  const { clock, fake, sync } = setup({
    check: [{ status: "current" }, { status: "behind" }, { status: "current" }],
    push: [{ status: "behind" }, { status: "pushed" }],
  });
  sync.start();
  await clock.advance(0);
  assert.deepEqual(fake.names(), ["status", "check", "push", "check", "pull", "push"]);
  sync.stop();
}
{
  const { clock, fake, sync } = setup({ push: [{ status: "behind" }, { status: "behind" }, { status: "behind" }, { status: "pushed" }] });
  sync.start();
  await clock.settle();
  assert.equal(fake.count("push"), 3);
  assert.equal(sync.state.mode, "idle");
  // The push that did not land waits a check interval, so a busy branch is no tight loop.
  await clock.advance(PACE.checkMs - 1);
  assert.equal(fake.count("push"), 3);
  await clock.advance(1);
  assert.equal(fake.count("push"), 4);
  sync.stop();
}
{
  const { clock, fake, sync } = setup({ push: [{ status: "moved" }, { status: "pushed" }] });
  sync.start();
  await clock.settle();
  assert.equal(fake.count("push"), 1);
  await clock.advance(PACE.checkMs);
  assert.equal(fake.count("push"), 2);
  sync.stop();
}

// 7. A conflict pauses, stays paused without checking, then resumes.
{
  const conflict = { conflict: { paths: ["a.tex"] }, pending: [] };
  const { clock, fake, sync } = setup({
    check: { status: "behind" },
    pull: [{ status: "conflict" }, { status: "pulled" }],
    status: [DEFAULTS.status, conflict, conflict, DEFAULTS.status],
  });
  sync.start();
  await clock.advance(0);
  assert.equal(sync.state.mode, "paused");
  assert.equal(sync.state.kind, "conflict");
  assert.equal(fake.count("check"), 1);
  await clock.to(PACE.checkMs);
  assert.equal(sync.state.mode, "paused");
  assert.equal(fake.count("status"), 2);
  assert.equal(fake.count("check"), 1);
  assert.equal(fake.count("pull"), 1);
  await clock.to(2 * PACE.checkMs);
  assert.equal(fake.count("check"), 1);
  await clock.to(3 * PACE.checkMs);
  assert.equal(fake.count("check"), 2);
  assert.notEqual(sync.state.mode, "paused");
  sync.stop();
}

// 8. A network refusal backs off, and a success resets the interval.
{
  const down = refused("network", "offline");
  const { clock, fake, sync } = setup({ check: [down, down, { status: "current" }] });
  sync.start();
  await clock.advance(0);
  assert.equal(sync.state.mode, "waiting");
  assert.equal(sync.state.reason, "offline");
  assert.equal(sync.state.nextAt, T0 + 60_000);
  await clock.to(60_000);
  assert.equal(fake.count("check"), 2);
  assert.equal(sync.state.mode, "waiting");
  assert.equal(sync.state.nextAt, T0 + 60_000 + 120_000);
  await clock.to(179_000);
  assert.equal(fake.count("check"), 2);
  await clock.to(180_000);
  assert.equal(fake.count("check"), 3);
  assert.equal(sync.state.mode, "idle");
  assert.equal(sync.state.nextAt, T0 + 180_000 + PACE.checkMs);
  sync.stop();
}

// 9. An auth refusal pauses.
{
  const { clock, sync } = setup({ check: refused("auth", "denied") });
  sync.start();
  await clock.advance(0);
  assert.equal(sync.state.mode, "paused");
  assert.equal(sync.state.kind, "auth");
  assert.equal(sync.state.reason, "Git could not sign in.");
  sync.stop();
}

// 10. After stop nothing runs.
{
  const { clock, fake, sync } = setup();
  sync.start();
  await clock.advance(0);
  sync.stop();
  const before = fake.calls.length;
  sync.edited();
  sync.poke();
  await clock.advance(3_600_000);
  assert.equal(fake.calls.length, before);
  assert.equal(sync.state.mode, "off");
  assert.equal(clock.pending(), 0);
}

// 11. A poke during a round makes exactly one more round.
{
  let release;
  const gate = new Promise((resolve) => (release = resolve));
  const { clock, fake, sync } = setup({ check: (n) => (n === 0 ? gate.then(() => ({ status: "current" })) : { status: "current" }) });
  sync.start();
  await clock.advance(0);
  assert.equal(fake.count("status"), 1);
  sync.poke();
  sync.poke();
  await clock.advance(0);
  assert.equal(fake.count("status"), 1);
  release();
  await clock.advance(0);
  assert.equal(fake.count("status"), 2);
  assert.equal(fake.maxInFlight(), 1);
  sync.stop();
}

// 12. onChange sees the mode go from running to idle.
{
  const seen = [];
  const { clock, sync } = setup({}, { onChange: (s) => seen.push(s.mode) });
  sync.start();
  await clock.advance(0);
  assert.ok(seen.indexOf("running") >= 0);
  assert.ok(seen.indexOf("idle") > seen.indexOf("running"));
  sync.stop();
}

// 13. Stopping while a check is pending ends the round: no pull and no push follow.
{
  let release;
  const gate = new Promise((resolve) => (release = resolve));
  const { clock, fake, sync } = setup({ check: () => gate.then(() => ({ status: "behind" })) });
  sync.start();
  await clock.advance(0);
  sync.stop();
  release();
  await clock.advance(0);
  assert.equal(fake.count("pull"), 0);
  assert.equal(fake.count("push"), 0);
  assert.equal(sync.state.mode, "off");
  assert.equal(clock.pending(), 0);
}

// 14. A restart while the old round is pending runs one round of its own, never alongside the old one.
{
  let release;
  const gate = new Promise((resolve) => (release = resolve));
  const { clock, fake, sync } = setup({ check: (n) => (n === 0 ? gate.then(() => ({ status: "behind" })) : { status: "current" }) });
  sync.start();
  await clock.advance(0);
  sync.stop();
  sync.start();
  release();
  await clock.advance(0);
  assert.equal(fake.count("pull"), 0);
  assert.equal(fake.count("status"), 2);
  assert.equal(sync.state.mode, "idle");
  assert.equal(fake.maxInFlight(), 1);
  sync.stop();
}

// 15. An interrupted pull that only a fresh pull can settle makes the round pull, then carry on.
{
  const { clock, fake, sync } = setup({
    status: { conflict: null, pending: ["op"] },
    recover: { pending: ["op"], needsPull: true, conflict: null },
  });
  sync.start();
  await clock.advance(0);
  assert.deepEqual(fake.names(), ["status", "recover", "pull", "check", "push"]);
  assert.equal(sync.state.mode, "idle");
  assert.equal(sync.state.lastPullAt, T0);
  sync.stop();
}

// 16. Leftover entries that a pull cannot settle keep the scheduler waiting, with no pull.
{
  const { clock, fake, sync } = setup({
    status: { conflict: null, pending: ["op"] },
    recover: { pending: ["op"], needsPull: false, conflict: null },
  });
  sync.start();
  await clock.advance(0);
  assert.equal(sync.state.mode, "waiting");
  assert.equal(fake.count("pull"), 0);
  sync.stop();
}

// 17. A push that answers a conflict pauses and keeps the edits pending, so
// the push runs again once the conflict is resolved, with no new edit.
{
  const conflict = { conflict: { paths: ["a.tex"] }, pending: [] };
  const { clock, fake, sync } = setup({
    push: [{ status: "conflict" }, { status: "pushed" }],
    status: [DEFAULTS.status, conflict, DEFAULTS.status],
  });
  sync.start();
  await clock.advance(0);
  assert.equal(sync.state.mode, "paused");
  assert.equal(sync.state.kind, "conflict");
  await clock.to(PACE.checkMs);
  assert.equal(sync.state.mode, "paused");
  assert.equal(fake.count("push"), 1);
  await clock.to(2 * PACE.checkMs);
  assert.equal(fake.count("push"), 2);
  assert.equal(sync.state.mode, "idle");
  sync.stop();
}

console.log("gitsync auto ok");
