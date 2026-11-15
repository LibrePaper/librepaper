// The preview's paint cadence and the preview target's pause, checked against
// `createSourceSchedule` directly. Timers and frames are fakes fired by hand,
// so each assertion says exactly which wait is pending and what fires it.
import assert from "node:assert/strict";
import { createSourceSchedule } from "../../src/lib/reader/source-schedule.js";

function fakeClock() {
  let nextId = 1;
  let time = 0;
  const timers = new Map();
  const frames = new Map();
  return {
    timers,
    frames,
    // The injected clock: time only moves when a test calls `advance()`.
    now: () => time,
    advance(ms) { time += ms; },
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
  clock.advance(150);
  schedule.changed();
  assert.equal(clock.frames.size, 1, "a change one interval after the paint requests a new frame");
  clock.fireFrames();
  assert.equal(paints, 2);
}

// A keystroke after a quiet spell paints on the next frame. The interval only
// holds back keystrokes that follow a paint closely.
{
  const clock = fakeClock();
  let paints = 0;
  const schedule = createSourceSchedule({
    ...clock, editing: () => true, format: () => "typst", paint: () => paints++, hidden: () => false,
  });
  schedule.changed();
  clock.fireFrames();
  clock.advance(1000);
  schedule.changed();
  assert.equal(clock.frames.size, 1, "after a quiet spell the next keystroke requests a frame at once");
  assert.equal(clock.timers.size, 0, "and waits on no cap timer");
  clock.fireFrames();
  assert.equal(paints, 2);
}

// Keystrokes within 150 ms of a paint wait for the cap timer, which asks for one
// frame when the interval has passed. The burst does not add timers or frames.
{
  const clock = fakeClock();
  let paints = 0;
  const schedule = createSourceSchedule({
    ...clock, editing: () => true, format: () => "typst", paint: () => paints++, hidden: () => false,
  });
  schedule.changed();
  clock.fireFrames();
  clock.advance(50);
  schedule.changed();
  assert.equal(clock.frames.size, 0, "a keystroke inside the interval requests no frame");
  assert.equal(clock.timers.size, 1, "it waits on one cap timer");
  assert.equal([...clock.timers.values()][0].ms, 100, "the timer waits out the rest of the 150 ms");
  clock.advance(30);
  for (let i = 0; i < 4; i++) schedule.changed();
  assert.equal(clock.timers.size, 1, "the rest of the burst keeps the one cap timer");
  assert.equal(clock.frames.size, 0, "and requests no frame of its own");
  clock.advance(70);
  clock.fireTimers();
  assert.equal(clock.frames.size, 1, "the cap timer requests exactly one frame");
  assert.equal(paints, 1, "nothing paints until that frame runs");
  clock.fireFrames();
  assert.equal(paints, 2, "the frame paints once");
}

// The burst's last keystroke is painted. The capped frame reads the source as
// it stands when it runs, so the text painted is the final one.
{
  const clock = fakeClock();
  let text = "";
  const painted = [];
  const schedule = createSourceSchedule({
    ...clock, editing: () => true, format: () => "typst", paint: () => painted.push(text), hidden: () => false,
  });
  text = "a";
  schedule.changed();
  clock.fireFrames();
  clock.advance(20);
  text = "ab";
  schedule.changed();
  text = "abc";
  schedule.changed();
  text = "abcd";
  schedule.changed();
  clock.advance(130);
  clock.fireTimers();
  clock.fireFrames();
  assert.deepEqual(painted, ["a", "abcd"], "the burst ends with one paint of its last text");
}

// A collaborator's edit while a capped repaint is pending neither adds a timer
// nor paints: the capped repaint renders the newer text.
{
  const clock = fakeClock();
  let paints = 0;
  const schedule = createSourceSchedule({
    ...clock, editing: () => true, format: () => "markdown", paint: () => paints++, hidden: () => false,
  });
  schedule.changed();
  clock.fireFrames();
  clock.advance(20);
  schedule.changed();
  schedule.changed({ remote: true });
  assert.equal(clock.timers.size, 1, "the remote edit adds no timer beside the cap timer");
  assert.equal(clock.frames.size, 0);
  clock.advance(130);
  clock.fireTimers();
  clock.fireFrames();
  assert.equal(paints, 2, "the capped repaint paints once, and the remote edit adds none");
}

// A change off the fast path drops a pending cap timer, as it drops a frame,
// and the passive wait takes its place.
{
  const clock = fakeClock();
  let editing = true;
  const schedule = createSourceSchedule({
    ...clock, editing: () => editing, format: () => "typst", paint: () => {}, hidden: () => false, passive: 1000,
  });
  schedule.changed();
  clock.fireFrames();
  clock.advance(20);
  schedule.changed();
  assert.equal(clock.timers.size, 1);
  editing = false;
  schedule.changed();
  assert.equal(clock.timers.size, 1, "the passive wait replaces the cap timer");
  assert.equal([...clock.timers.values()][0].ms, 1000);
  assert.equal(clock.frames.size, 0);
}

// cancel() and dispose() clear a pending cap timer, so no capped paint runs
// afterwards.
{
  const clock = fakeClock();
  let paints = 0;
  const schedule = createSourceSchedule({
    ...clock, editing: () => true, format: () => "typst", paint: () => paints++, hidden: () => false,
  });
  schedule.changed();
  clock.fireFrames();
  clock.advance(20);
  schedule.changed();
  assert.equal(clock.timers.size, 1);
  schedule.cancel();
  assert.equal(clock.timers.size, 0, "cancel clears the cap timer");
  clock.advance(130);
  clock.fireTimers();
  assert.equal(clock.frames.size, 0, "a cancelled cap timer requests no frame");

  const other = createSourceSchedule({
    ...clock, editing: () => true, format: () => "typst", paint: () => paints++, hidden: () => false,
  });
  other.changed();
  clock.fireFrames();
  clock.advance(20);
  other.changed();
  assert.equal(clock.timers.size, 1);
  other.dispose();
  assert.equal(clock.timers.size, 0, "dispose clears the cap timer");
  clock.advance(130);
  clock.fireTimers();
  assert.equal(clock.frames.size, 0, "a disposed cap timer requests no frame");
  assert.equal(paints, 2, "only the two paints that ran before the cancel and dispose painted");
}

// Continuous LaTeX: an editing change takes the frame path, not the debounce.
{
  const clock = fakeClock();
  let paints = 0;
  let on = true;
  const schedule = createSourceSchedule({
    ...clock, editing: () => true, format: () => "latex", latexDebounce: () => 2000,
    continuous: () => on, paint: () => paints++, hidden: () => false,
  });
  schedule.changed();
  schedule.changed();
  assert.equal(clock.frames.size, 1, "continuous LaTeX requests one paint frame");
  assert.equal(clock.timers.size, 0, "and does not wait on the debounce");
  clock.fireFrames();
  assert.equal(paints, 1);
  on = false;
  schedule.changed();
  assert.equal(clock.frames.size, 0, "without the preference LaTeX waits for a quiet source");
  assert.equal(clock.timers.size, 1);
  assert.equal([...clock.timers.values()][0].ms, 2000);
}

// A hidden page does not run frames, so a zero timer stands in for one, and
// that timer does not paint either: the render is held until the page shows.
{
  const clock = fakeClock();
  let paints = 0;
  let hiddenNow = true;
  const schedule = createSourceSchedule({
    ...clock, editing: () => true, format: () => "typst", paint: () => paints++, hidden: () => hiddenNow,
  });
  schedule.changed();
  schedule.changed();
  assert.equal(clock.frames.size, 0, "a hidden page requests no frame");
  assert.equal(clock.timers.size, 1, "one zero timer stands in for the frame");
  assert.equal([...clock.timers.values()][0].ms, 0);
  clock.fireTimers();
  assert.equal(paints, 0, "a hidden page does not paint");

  hiddenNow = false;
  schedule.resume();
  assert.equal(paints, 1, "resume draws the skipped render once");
  schedule.resume();
  assert.equal(paints, 1, "a second resume draws nothing");
}

// A collaborator's edit while this browser is editing waits a second, and
// further remote edits neither add a timer nor move the deadline.
{
  const clock = fakeClock();
  let paints = 0;
  const schedule = createSourceSchedule({
    ...clock, editing: () => true, format: () => "markdown", paint: () => paints++, hidden: () => false, remoteDelay: 1000,
  });
  schedule.changed({ remote: true });
  assert.equal(clock.frames.size, 0, "a remote change does not request a frame");
  assert.equal(clock.timers.size, 1);
  assert.equal([...clock.timers.values()][0].ms, 1000);
  const [pendingId] = clock.timers.keys();
  schedule.changed({ remote: true });
  schedule.changed({ remote: true });
  assert.deepEqual([...clock.timers.keys()], [pendingId], "further remote changes keep the one pending timer");
  assert.equal(paints, 0);
  clock.fireTimers();
  assert.equal(paints, 1, "the timer paints once");
  assert.equal(clock.timers.size, 0);
}

// A local edit while a remote timer is pending clears that timer and paints on
// the frame, so the remote wait cannot delay this browser's own preview.
{
  const clock = fakeClock();
  let paints = 0;
  const schedule = createSourceSchedule({
    ...clock, editing: () => true, format: () => "markdown", paint: () => paints++, hidden: () => false,
  });
  schedule.changed({ remote: true });
  schedule.changed();
  assert.equal(clock.timers.size, 0, "a local change clears the pending remote timer");
  assert.equal(clock.frames.size, 1, "and requests a frame");
  clock.fireFrames();
  assert.equal(paints, 1);
}

// A remote edit while a frame is already pending adds no timer: the frame will
// paint the text.
{
  const clock = fakeClock();
  const schedule = createSourceSchedule({
    ...clock, editing: () => true, format: () => "markdown", hidden: () => false,
  });
  schedule.changed();
  schedule.changed({ remote: true });
  assert.equal(clock.frames.size, 1);
  assert.equal(clock.timers.size, 0, "no remote timer while a frame is pending");
}

// Without continuous LaTeX, a remote edit waits the same debounce a local one
// does.
{
  const pendingFor = (remote) => {
    const clock = fakeClock();
    const schedule = createSourceSchedule({
      ...clock, editing: () => true, format: () => "latex", latexDebounce: () => 1500, continuous: () => false,
      passive: 1000, hidden: () => false,
    });
    schedule.changed({ remote });
    return { frames: clock.frames.size, waits: [...clock.timers.values()].map((t) => t.ms) };
  };
  assert.deepEqual(pendingFor(true), pendingFor(false), "remote LaTeX behaves exactly like local");
  assert.deepEqual(pendingFor(true), { frames: 0, waits: [1500] });
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

// A passive timer left from before editing began gives way to this browser's
// own edit: the timer is cleared and one frame paints, so continuous typing
// can neither keep moving a deadline nor stack frames.
{
  const clock = fakeClock();
  let editing = false;
  let paints = 0;
  const schedule = createSourceSchedule({
    ...clock, editing: () => editing, format: () => "typst", paint: () => paints++, hidden: () => false,
  });
  schedule.changed();
  editing = true;
  schedule.changed();
  schedule.changed();
  assert.equal(clock.timers.size, 0, "the passive timer is cleared");
  assert.equal(clock.frames.size, 1, "one frame for the burst");
  clock.fireFrames();
  assert.equal(paints, 1);
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
