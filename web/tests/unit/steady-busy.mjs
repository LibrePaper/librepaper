// The preview's busy flag, at reading speed: what appears, when, and for how
// long. Time is injected, so none of this waits.
import assert from "node:assert/strict";
import { createSteadyBusy } from "../../src/lib/reader/steady-busy.js";

function clock() {
  let at = 0;
  const timers = new Map();
  let next = 1;
  return {
    now: () => at,
    setTimer: (fn, delay) => { timers.set(next, { fn, at: at + delay }); return next++; },
    clearTimer: (id) => { timers.delete(id); },
    /// Moves time forward, firing whatever was due on the way.
    advance(ms) {
      const until = at + ms;
      for (;;) {
        const due = [...timers.entries()].filter(([, timer]) => timer.at <= until).sort((a, b) => a[1].at - b[1].at)[0];
        if (!due) break;
        timers.delete(due[0]);
        at = due[1].at;
        due[1].fn();
      }
      at = until;
    },
    get pending() { return timers.size; },
  };
}

function watcher() {
  const time = clock();
  const shown = [];
  const steady = createSteadyBusy({
    appearAfter: 250, holdFor: 600, onchange: (value) => shown.push(value),
    setTimer: time.setTimer, clearTimer: time.clearTimer, now: time.now,
  });
  return { time, shown, steady };
}

// A render that finishes before the indicator would have appeared says
// nothing at all. This is the Typst and Markdown case, and the flicker the
// hold exists to stop: the whole control mounts into the header row and out
// of it again with this flag.
{
  const { time, shown, steady } = watcher();
  for (let keystroke = 0; keystroke < 20; keystroke += 1) {
    steady.set(true);
    time.advance(40);
    steady.set(false);
    time.advance(60);
  }
  assert.deepEqual(shown, [], "a render nobody could read about is not reported");
  assert.equal(steady.shown, false);
}

// A render that runs long enough is reported, and what was reported stays up
// long enough to be read even when the render ends immediately afterwards.
{
  const { time, shown, steady } = watcher();
  steady.set(true);
  time.advance(249);
  assert.deepEqual(shown, [], "not yet");
  time.advance(1);
  assert.deepEqual(shown, [true], "a render this slow is worth saying");
  steady.set(false);
  assert.deepEqual(shown, [true], "and is not taken away the instant it ends");
  time.advance(599);
  assert.deepEqual(shown, [true]);
  time.advance(1);
  assert.deepEqual(shown, [true, false]);
  assert.equal(time.pending, 0, "nothing is left ticking");
}

// A burst of renders behind an indicator that is already up leaves it up: it
// is one continuous "the preview is working", not one per compile.
{
  const { time, shown, steady } = watcher();
  steady.set(true);
  time.advance(300);
  assert.deepEqual(shown, [true]);
  for (let render = 0; render < 5; render += 1) {
    steady.set(false);
    time.advance(20);
    steady.set(true);
    time.advance(20);
  }
  assert.deepEqual(shown, [true], "the indicator does not blink between renders");
  steady.set(false);
  time.advance(600);
  assert.deepEqual(shown, [true, false]);
}

// Leaving the document drops a held indicator at once: there is nothing left
// for it to describe.
{
  const { time, shown, steady } = watcher();
  steady.set(true);
  time.advance(300);
  steady.stop();
  assert.deepEqual(shown, [true, false]);
  assert.equal(time.pending, 0);
  time.advance(5000);
  assert.deepEqual(shown, [true, false], "and nothing fires after it");
}

console.log("steady-busy: fast renders say nothing, slow ones stay readable");
