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
  check("nor does a box on a figure",
    verbsFor({ region: { image_index: 0 } }).map((verb) => verb.id).join(",") === "comment");
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
  check("a box is a comment", motivationFor("region") === "commenting");
}

/* ---------------------------------------------------------------- the modes */

{
  check("only the two gestures with nothing to select are modes",
    MODES.map((item) => item.id).join(",") === "point,region");
  check("the two modes do not share an icon", MODES[0].icon !== MODES[1].icon);
  check("arming from nothing arms it", nextMode("", "point") === "point");
  check("arming the other swaps", nextMode("point", "region") === "region");
  // A mode changes what a click or a drag in the document does, and `region`
  // stops text being selectable at all; the control that armed it has to be
  // able to put it away.
  check("choosing the armed mode puts it away", nextMode("region", "region") === "");
}

if (failures) {
  console.error(`annotating: ${failures} check(s) failed`);
  process.exit(1);
}
console.log("annotating: modes are armed, verbs act on a selection, and both store as W3C motivations");
