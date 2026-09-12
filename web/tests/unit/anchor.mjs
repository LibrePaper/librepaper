// anchorSource, and anchorAllSources over it.
//
// A source selector is anchored the same way a rendered one is -- exact
// match, then the flattened fallback -- but it also has to find its file
// first, since the path a comment remembers can have been renamed since. The
// three things worth checking are that the easy case still works, that a
// rename is followed when the passage is unmistakably in exactly one other
// file, and that it is refused rather than guessed at when two files could
// both be the answer.

import { anchorOne, anchorSource, anchorAllSources } from "../../src/lib/anchor.js";

let failures = 0;
function check(what, condition) {
  if (condition) return;
  failures += 1;
  console.error(`anchor: ${what}`);
}

const source = (path, exact, extra = {}) => ({ path, exact, prefix: "", suffix: "", position: null, ...extra });

{
  const tree = { texts: { "chapter.typ": "before the passage of interest after" } };
  const found = anchorSource(tree, source("chapter.typ", "the passage of interest"));
  check("the path it names is tried first", found?.path === "chapter.typ");
  check("and the offsets are the passage's own", found?.start === 7 && found?.end === 30);
}

{
  // The file has been renamed since the comment was made -- its old path is
  // gone, but the text is, unmistakably, in the one file that remains.
  const tree = { texts: { "renamed.typ": "before the passage of interest after" } };
  const found = anchorSource(tree, source("chapter.typ", "the passage of interest"));
  check("a rename is followed when only one file could be meant", found?.path === "renamed.typ");
}

{
  // Two files could both be the answer once the name is gone: guessing which
  // one is worse than saying nothing.
  const tree = {
    texts: {
      "one.typ": "before the passage of interest after",
      "two.typ": "before the passage of interest after",
    },
  };
  const found = anchorSource(tree, source("gone.typ", "the passage of interest"));
  check("ambiguous across files after a rename is refused", found === null);
}

{
  // The exact text has reflowed a little -- a line break where there used to
  // be a space -- so only the whitespace-flattened fallback finds it.
  const tree = { texts: { "chapter.typ": "before the\npassage of interest after" } };
  const found = anchorSource(tree, source("chapter.typ", "the passage of interest"));
  check("the flattened fallback still finds a reflowed passage", found?.path === "chapter.typ");
}

{
  const found = anchorSource({ texts: {} }, source("chapter.typ", "the passage of interest"));
  check("a passage in no file at all is refused", found === null);
}

{
  const found = anchorSource({ texts: { "chapter.typ": "nothing like it here" } }, source("chapter.typ", "the passage of interest"));
  check("a passage nowhere in its own file, with no other file to try, is refused", found === null);
}

{
  const point = { path: "chapter.typ", exact: "", prefix: "before", suffix: " after", position: 6, point: true };
  const found = anchorOne("before after", point);
  check("a point selector keeps its nonnegative position", found?.start === 6 && found?.end === 6);
  const stale = anchorOne("short", { ...point, position: 999 });
  check("a stale point outside the document is refused", stale === null);
  const shifted = anchorOne("before new after", point);
  check("a point whose surrounding context was replaced is refused", shifted === null);
  check("a point follows an insertion before its context", anchorOne("New paragraph. before after", point)?.start === 21);
  check("a point follows deletion before its context", anchorOne("before after", { ...point, position: 999 })?.start === 6);
  check("a null point position is refused", anchorOne("before after", { ...point, position: null }) === null);
  check("a string point position is refused", anchorOne("before after", { ...point, position: "6" }) === null);
  check("a point can sit at document start", anchorOne("before after", { point:true, exact:"", position:0, suffix:"before after" })?.start === 0);
  check("a point can sit at document end", anchorOne("before after", { point:true, exact:"", position:12, prefix:"before after" })?.start === 12);
}

/* --------------------------------------------------------- anchorAllSources */

{
  const tree = { texts: { "chapter.typ": "before the passage of interest after" } };
  const comments = [
    { id: 1, source: source("chapter.typ", "the passage of interest") },
    { id: 2, source: source("chapter.typ", "nowhere to be found") },
    { id: 3 }, // no source anchor: left alone, and not counted
  ];
  const { anchored, orphaned } = anchorAllSources(tree, comments);
  check("counts only the comments that have a source", anchored + orphaned === 2);
  check("one anchored", anchored === 1 && comments[0].sourcePath === "chapter.typ" && typeof comments[0].sourceStart === "number");
  check("one orphaned", orphaned === 1 && comments[1].sourcePath === null && comments[1].sourceStart === null && comments[1].sourceEnd === null);
  check("the sourceless comment is untouched", !("sourcePath" in comments[2]));
}

if (failures) process.exit(1);
console.log("anchor: source selectors anchor to the right file, or to none");
