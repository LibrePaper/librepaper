// How a comment thread is written down: the runs a card groups its messages
// into, and the short time each run is stamped with.

import { named, threadRuns } from "../../src/lib/thread.js";
import { moment } from "../../src/lib/dates.js";

let failures = 0;
function check(what, condition) {
  if (condition) return;
  failures += 1;
  console.error(`thread: ${what}`);
}

const reply = (id, creator, body, created = "2026-09-17T11:08:00Z") => ({ id, creator, body, created });

/* ----------------------------------------------------------------- the runs */

{
  const runs = threadRuns({
    id: "c1",
    creator: "Vincent",
    created: "2026-09-17T11:04:00Z",
    body: "this is not ok",
    replies: [reply("r1", "Vincent", "Again, there is repetition here"), reply("r2", "Vincent", "A formula is a rule")],
  });
  check("a run of one author is one run", runs.length === 1);
  check("it carries every message", runs[0].posts.map((post) => post.body).join("|") === "this is not ok|Again, there is repetition here|A formula is a rule");
  check("it is stamped with the first message's time", runs[0].created === "2026-09-17T11:04:00Z");
}

{
  const runs = threadRuns({
    id: "c2",
    creator: "Vincent",
    body: "this is not ok",
    replies: [reply("r1", "Alice", "I agree with the second point"), reply("r2", "Vincent", "Fair enough")],
  });
  check("a new author opens a new run", runs.map((run) => run.author).join(",") === "Vincent,Alice,Vincent");
  check("and the author who comes back gets a third one", runs.every((run) => run.posts.length === 1));
}

{
  const runs = threadRuns({ id: "c3", creator: "", body: "", replies: [] });
  check("a note with no words still has a run, so its author is shown", runs.length === 1);
  check("and an empty name is written out", runs[0].author === "Anonymous");
  check("named() agrees about whitespace", named("  ") === "Anonymous" && named("Vincent") === "Vincent");
}

{
  const runs = threadRuns({ id: "c4", creator: "Vincent", body: "" });
  check("a comment with no replies array is a thread of one", runs.length === 1 && runs[0].posts.length === 1);
  check("every run is keyed by its first message", runs[0].id === "note-c4");
}

/* ----------------------------------------------------------------- the time */

{
  const now = new Date("2026-09-17T15:00:00Z");
  const today = new Date("2026-09-17T11:04:00Z");
  const pad = (part) => String(part).padStart(2, "0");
  check(
    "a message from today is written as the clock time, in the reader's zone",
    moment(today, now) === `${pad(today.getHours())}:${pad(today.getMinutes())}`,
  );
  check("an older message is written as its day", moment(new Date("2026-09-01T11:04:00Z"), now) === "2026-09-01");
  check("a missing time is written as nothing", moment("", now) === "" && moment("not a date", now) === "");
}

if (failures) {
  console.error(`thread: ${failures} check(s) failed`);
  process.exit(1);
}
console.log("thread: comments group by author and are stamped once per run");
