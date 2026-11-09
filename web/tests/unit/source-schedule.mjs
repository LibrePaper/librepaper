// The preview's paint cadence and the preview target's pause, checked against
// `createSourceSchedule` directly. Timers and frames are fakes fired by hand,
// so each assertion says exactly which wait is pending and what fires it.
import assert from "node:assert/strict";
import { createSourceSchedule } from "../../src/lib/reader/source-schedule.js";

function fakeClock() {
  let nextId = 1;
  const timers = new Map();
  const frames = new Map();
  return {
    timers,
    frames,
    setTimer(run, ms) {
      const id = nextId++;
      timers.set(id, { run, ms });
      return id;
    },
    clearTimer(id) { timers.delete(id); },
    requestFrame(run) {
      const id = nextId++;
      frames.set(id, run);
      return id;
    },
    cancelFrame(id) { frames.delete(id); },
    fireFrames() {
      const pending = [...frames.values()];
      frames.clear();
      for (const run of pending) run();
    },
    fireTimers() {
      const pending = [...timers.values()];
      timers.clear();
      for (const { run } of pending) run();
    },
  };
}

// An editor in a fast format: every change in one frame shares one paint.
{
  const clock = fakeClock();
  let paints = 0;
  const schedule = createSourceSchedule({
    ...clock, editing: () => true, format: () => "typst", paint: () => paints++, hidden: () => false,
  });
  for (let i = 0; i < 5; i++) schedule.changed();
  assert.equal(clock.frames.size, 1, "several changes in one frame request one paint frame");
  assert.equal(clock.timers.size, 0, "a fast format does not wait on a timer");
  clock.fireFrames();
  assert.equal(paints, 1, "the frame paints once");
  schedule.changed();
  assert.equal(clock.frames.size, 1, "a change after the frame fired requests a new frame");
  clock.fireFrames();
  assert.equal(paints, 2);
}

// A hidden page does not run frames, so a zero timer stands in for one.
{
  const clock = fakeClock();
  let paints = 0;
  const schedule = createSourceSchedule({
    ...clock, editing: () => true, format: () => "typst", paint: () => paints++, hidden: () => true,
  });
  schedule.changed();
  schedule.changed();
  assert.equal(clock.frames.size, 0, "a hidden page requests no frame");
  assert.equal(clock.timers.size, 1, "one zero timer stands in for the frame");
  assert.equal([...clock.timers.values()][0].ms, 0);
  clock.fireTimers();
  assert.equal(paints, 1);
}

// A page that is not editing waits the passive pause, and each change
// restarts that wait.
{
  const clock = fakeClock();
  let paints = 0;
  const schedule = createSourceSchedule({
    ...clock, editing: () => false, format: () => "markdown", paint: () => paints++, passive: 1000,
  });
  schedule.changed();
  schedule.changed();
  assert.equal(clock.timers.size, 1, "a second change replaces the pending wait");
  assert.equal([...clock.timers.values()][0].ms, 1000, "the wait is the passive pause");
  assert.equal(clock.frames.size, 0);
  assert.equal(paints, 0, "nothing paints before the pause ends");
  clock.fireTimers();
  assert.equal(paints, 1);
}

// LaTeX while editing waits for latex.debounce(), not a frame.
{
  const clock = fakeClock();
  const schedule = createSourceSchedule({
    ...clock, editing: () => true, format: () => "latex", latexDebounce: () => 1500, passive: 1000,
  });
  schedule.changed();
  assert.equal(clock.frames.size, 0, "LaTeX does not paint on a frame");
  assert.equal([...clock.timers.values()][0].ms, 1500, "an editor waits latex.debounce()");
}

// LaTeX while not editing waits the longer of latex.debounce() and the passive
// pause.
{
  const clock = fakeClock();
  const shorter = createSourceSchedule({
    ...clock, editing: () => false, format: () => "latex", latexDebounce: () => 300, passive: 1000,
  });
  shorter.changed();
  assert.equal([...clock.timers.values()][0].ms, 1000, "the passive pause wins when it is longer");
  clock.timers.clear();
  const longer = createSourceSchedule({
    ...clock, editing: () => false, format: () => "latex", latexDebounce: () => 2500, passive: 1000,
  });
  longer.changed();
  assert.equal([...clock.timers.values()][0].ms, 2500, "latex.debounce() wins when it is longer");
}

// While a timer paint is pending, a fast-format change does not start a frame,
// so continuous typing cannot keep moving the timer's deadline forever.
{
  const clock = fakeClock();
  let editing = false;
  const schedule = createSourceSchedule({
    ...clock, editing: () => editing, format: () => "typst", hidden: () => false,
  });
  schedule.changed();
  editing = true;
  schedule.changed();
  schedule.changed();
  assert.equal(clock.frames.size, 0, "no frame while the debounce timer is pending");
  assert.equal(clock.timers.size, 1);
}

// cancel() stops a pending timer paint. It leaves a pending frame alone.
{
  const clock = fakeClock();
  let paints = 0;
  const schedule = createSourceSchedule({
    ...clock, editing: () => false, format: () => "markdown", paint: () => paints++,
  });
  schedule.changed();
  schedule.cancel();
  assert.equal(clock.timers.size, 0);
  clock.fireTimers();
  assert.equal(paints, 0, "a cancelled timer paint never runs");

  const framed = createSourceSchedule({
    ...clock, editing: () => true, format: () => "typst", paint: () => paints++, hidden: () => false,
  });
  framed.changed();
  framed.cancel();
  assert.equal(clock.frames.size, 1, "cancel does not cancel a pending frame");
}

// scheduleTarget() works the preview target out again once, after the pause,
// and a second call restarts the pause.
{
  const clock = fakeClock();
  let retargets = 0;
  const schedule = createSourceSchedule({
    ...clock, retarget: () => retargets++, targetPause: 500,
  });
  schedule.scheduleTarget();
  schedule.scheduleTarget();
  assert.equal(clock.timers.size, 1, "a second call replaces the first");
  assert.equal([...clock.timers.values()][0].ms, 500);
  assert.equal(retargets, 0);
  clock.fireTimers();
  assert.equal(retargets, 1, "retarget runs once after the pause");
}

// dispose() stops both the preview timer and the target timer.
{
  const clock = fakeClock();
  let paints = 0;
  let retargets = 0;
  const schedule = createSourceSchedule({
    ...clock, editing: () => false, format: () => "markdown", paint: () => paints++, retarget: () => retargets++,
  });
  schedule.changed();
  schedule.scheduleTarget();
  assert.equal(clock.timers.size, 2);
  schedule.dispose();
  assert.equal(clock.timers.size, 0, "dispose clears both timers");
  clock.fireTimers();
  assert.equal(paints, 0);
  assert.equal(retargets, 0);
}

// bump() advances the generation that renders compare against.
{
  const schedule = createSourceSchedule();
  assert.equal(schedule.generation, 0);
  schedule.bump();
  schedule.bump();
  schedule.bump();
  assert.equal(schedule.generation, 3);
}
