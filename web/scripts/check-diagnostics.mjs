// The diagnostic timing rule, against a clock the test owns.
//
// The two cases that matter: a render that fails at 60 ms and
// succeeds at 200 ms never paints, and one that fails at 60 ms and is left
// alone paints at 400 ms -- four hundred from the keystroke, not from the
// render that noticed it.

import { diagnose } from "./render.js";
import { painter, DIAGNOSTIC_DELAY } from "../src/lib/diagnostics.js";

let failures = 0;
function check(what, condition) {
  if (condition) return;
  failures += 1;
  console.error(`diagnostics: ${what}`);
}

/// A session on a clock that only moves when the test says so.
function session() {
  let clock = 0;
  let timers = [];
  let painted = [];
  const paint = (list) => painted.push({ at: clock, list });
  const surface = painter({
    paint,
    now: () => clock,
    setTimer: (fn, after) => {
      const timer = { at: clock + after, fn, live: true };
      timers.push(timer);
      return timer;
    },
    clearTimer: (timer) => {
      if (timer) timer.live = false;
    },
  });
  return {
    surface,
    painted,
    /// Moves the clock, firing whatever comes due on the way.
    advance(to) {
      while (true) {
        const due = timers
          .filter((timer) => timer.live && timer.at <= to)
          .sort((a, b) => a.at - b.at)[0];
        if (!due) break;
        due.live = false;
        clock = due.at;
        due.fn();
      }
      clock = to;
    },
    now: () => clock,
  };
}

// A render that fails at 60 ms and succeeds at 200 ms never paints a
// diagnostic: the source went from red to clean faster than a reader could
// have looked at it.
{
  const run = session();
  run.surface.typed();
  run.advance(60);
  run.surface.rendered({ page: null, diagnostics: [{ severity: "error", message: "unclosed" }] });
  run.advance(200);
  run.surface.rendered({ page: "<p>ok</p>", diagnostics: [] });
  run.advance(1000);
  const errors = run.painted.filter((one) => one.list.length);
  check(
    `a failure at 60ms that succeeded at 200ms painted ${errors.length} diagnostic list(s)`,
    errors.length === 0,
  );
}

// One that fails at 60 ms and is left alone paints at 400 ms from the
// keystroke -- not at 460, which is what measuring from the render gives.
{
  const run = session();
  run.surface.typed();
  run.advance(60);
  run.surface.rendered({ page: null, diagnostics: [{ severity: "error", message: "unclosed" }] });
  run.advance(1000);
  const errors = run.painted.filter((one) => one.list.length);
  check(`a failure left alone painted ${errors.length} times`, errors.length === 1);
  check(
    `it painted at ${errors[0]?.at}ms, not at ${DIAGNOSTIC_DELAY}ms from the keystroke`,
    errors[0]?.at === DIAGNOSTIC_DELAY,
  );
}

// A slow render does not add its own length to the wait. One that takes 380 ms
// still paints at 400 from the keystroke, not at 780.
{
  const run = session();
  run.surface.typed();
  run.advance(380);
  run.surface.rendered({ page: null, diagnostics: [{ severity: "error", message: "unclosed" }] });
  run.advance(2000);
  const errors = run.painted.filter((one) => one.list.length);
  check(
    `a 380ms render painted at ${errors[0]?.at}ms rather than ${DIAGNOSTIC_DELAY}ms`,
    errors[0]?.at === DIAGNOSTIC_DELAY,
  );
}

// A render slower than the delay itself paints as soon as it lands: the source
// has already been quiet longer than the rule asks for.
{
  const run = session();
  run.surface.typed();
  run.advance(900);
  run.surface.rendered({ page: null, diagnostics: [{ severity: "error", message: "unclosed" }] });
  run.advance(1000);
  const errors = run.painted.filter((one) => one.list.length);
  check(
    `a render slower than the delay painted at ${errors[0]?.at}ms rather than 900ms`,
    errors[0]?.at === 900,
  );
}

// Typing again restarts the wait, so a burst of typing never paints mid-burst.
{
  const run = session();
  run.surface.typed();
  run.advance(60);
  run.surface.rendered({ page: null, diagnostics: [{ severity: "error", message: "unclosed" }] });
  run.advance(300);
  run.surface.typed(); // still typing at 300ms
  run.advance(360);
  run.surface.rendered({ page: null, diagnostics: [{ severity: "error", message: "unclosed" }] });
  run.advance(2000);
  const errors = run.painted.filter((one) => one.list.length);
  check(`a burst of typing painted ${errors.length} times`, errors.length === 1);
  check(
    `the second keystroke's wait ended at ${errors[0]?.at}ms rather than 700ms`,
    errors[0]?.at === 700,
  );
}

// A successful render clears at once, whatever the clock says.
{
  const run = session();
  run.surface.typed();
  run.advance(60);
  run.surface.rendered({ page: null, diagnostics: [{ severity: "error", message: "unclosed" }] });
  run.advance(500); // painted at 400
  run.surface.rendered({ page: "<p>ok</p>", diagnostics: [] });
  const last = run.painted[run.painted.length - 1];
  check(
    `a successful render cleared at ${last?.at}ms with ${last?.list.length} left`,
    last?.at === 500 && last?.list.length === 0,
  );
}

// A warning that comes back beside a page is painted on the slow schedule, not
// at once: a font name half typed should not flash a badge per keystroke.
{
  const run = session();
  run.surface.typed();
  run.advance(60);
  run.surface.rendered({
    page: "<p>ok</p>",
    diagnostics: [{ severity: "warning", message: "unknown font" }],
  });
  const straightAway = run.painted[run.painted.length - 1];
  check(
    "a warning was painted the moment its page landed",
    straightAway.list.length === 0,
  );
  run.advance(1000);
  const warnings = run.painted.filter((one) => one.list.length);
  check(
    `the warning painted at ${warnings[0]?.at}ms rather than ${DIAGNOSTIC_DELAY}ms`,
    warnings.length === 1 && warnings[0].at === DIAGNOSTIC_DELAY,
  );
}

// An error in a file the main one imports is an error in *that* file, and says
// so. The engine's `file` field used to admit "in the browser it is always
// empty"; a document is a directory now, the browser hands the compiler the
// whole of it, and a span in a chapter resolves to the chapter.
//
// This is the half of the rule the editor depends on to open the right file
// when somebody clicks the badge. The other half -- that an unplaceable
// diagnostic has line 0 whatever its file says -- is asserted beside it,
// because the two are easy to confuse and the editor treats them differently.
{
  const said = diagnose(
    "typst",
    {
      main: "main.typ",
      texts: {
        "main.typ": '#import "lib.typ": greet\n#greet()\n',
        "lib.typ": "#let greet() = { undefined_here }\n",
      },
    },
    "main.typ",
  );
  if (!said) {
    console.log("diagnostics: no typst module built; skipping the imported-file case");
  } else {
    const errors = said.diagnostics.filter((one) => one.severity === "error");
    check(
      `an error in an imported file names it: ${errors[0]?.file}:${errors[0]?.line}`,
      errors.length === 1 && errors[0].file === "lib.typ" && errors[0].line === 1,
    );
    const unplaceable = said.diagnostics.filter((one) => one.line === 0);
    check(
      "a diagnostic with no span has line 0, which is not the same as being in the main file",
      unplaceable.every((one) => one.file === ""),
    );
  }
}

if (failures) {
  console.error(`diagnostics: ${failures} timing rule(s) broken`);
  process.exit(1);
}
console.log("diagnostics: the wait is measured from the keystroke, and an error names its file");
