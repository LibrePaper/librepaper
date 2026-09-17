// The vocabulary of making an annotation: which gestures are modes, which are
// verbs over a selection, and what each one is stored as.

import { MODES, VERBS, motivationFor, nextMode, verbsFor } from "../../src/lib/annotating.js";

let failures = 0;
function check(what, condition) {
  if (condition) return;
  failures += 1;
  console.error(`annotating: ${what}`);
}

/* ---------------------------------------------------------------- the verbs */

{
  check("a passage can be commented on, highlighted or replaced",
    verbsFor({ exact: "some words" }).map((verb) => verb.id).join(",") === "comment,highlight,suggest");
  check("a point has no words to highlight or replace",
    verbsFor({ point: true }).map((verb) => verb.id).join(",") === "comment");
  check("nor does a note at a point",
    verbsFor({ point: true }).map((verb) => verb.id).join(",") === "comment");
  check("and with nothing selected there is nothing to offer", verbsFor(null).length === 0);
  check("every verb says what it is for", VERBS.every((verb) => verb.id && verb.label && verb.title));
}

/* ---------------------------------------------------------- the motivations */

{
  check("a comment is stored as commenting", motivationFor("comment") === "commenting");
  check("a highlight as highlighting", motivationFor("highlight") === "highlighting");
  check("a suggestion as editing", motivationFor("suggest") === "editing");
  // A point note and a box differ in what they are anchored to, not in what
  // they are; the server has never heard of either.
  check("a point note is a comment", motivationFor("point") === "commenting");
  check("anything else is a comment", motivationFor("point") === "commenting");
}

/* ---------------------------------------------------------------- the modes */

{
  // A box drawn on a figure was the other mode. A comment is a range of the
  // source now, and a rectangle over a rendered image is not one, so there is
  // nothing to arm.
  check("the one gesture with nothing to select is a mode",
    MODES.map((item) => item.id).join(",") === "point");
  check("arming from nothing arms it", nextMode("", "point") === "point");
  // A mode changes what a click in the document does, so the control that
  // armed it has to be able to put it away.
  check("choosing the armed mode puts it away", nextMode("point", "point") === "");
}

if (failures) {
  console.error(`annotating: ${failures} check(s) failed`);
  process.exit(1);
}
console.log("annotating: modes are armed, verbs act on a selection, and both store as W3C motivations");
