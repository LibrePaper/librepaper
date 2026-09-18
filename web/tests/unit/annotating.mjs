// The vocabulary of making an annotation: what can be done to a selection,
// and what each of those is stored as.

import { VERBS, motivationFor } from "../../src/lib/annotating.js";

let failures = 0;
function check(what, condition) {
  if (condition) return;
  failures += 1;
  console.error(`annotating: ${what}`);
}

/* ---------------------------------------------------------------- the verbs */

{
  check("a passage can be commented on, highlighted or replaced",
    VERBS.map((verb) => verb.id).join(",") === "comment,highlight,suggest");
  check("every verb says what it is for", VERBS.every((verb) => verb.id && verb.label && verb.title));
}

/* ---------------------------------------------------------- the motivations */

{
  check("a comment is stored as commenting", motivationFor("comment") === "commenting");
  check("a highlight as highlighting", motivationFor("highlight") === "highlighting");
  check("a suggestion as editing", motivationFor("suggest") === "editing");
  check("anything else is a comment", motivationFor("whatever") === "commenting");
}

if (failures) {
  console.error(`annotating: ${failures} check(s) failed`);
  process.exit(1);
}
console.log("annotating: every verb acts on a selection and stores as a W3C motivation");
