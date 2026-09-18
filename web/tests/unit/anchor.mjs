// Placing comments on the page, and taking the server's word for where their
// passages are in the source.
//
// Two different jobs, and the split between them is the point. `anchorOne`
// matches a rendered quotation against rendered text, which is display and
// may be wrong without anything being lost. `placeSources` does no matching
// at all: the server resolved every comment against the document's own
// history, and this only turns the file id in that answer into a path.

import { anchorOne, placeSources, shownSelector } from "../../src/lib/anchor.js";

let failures = 0;
function check(what, condition) {
  if (condition) return;
  failures += 1;
  console.error(`anchor: ${what}`);
}

const source = (path, exact, extra = {}) => ({ path, exact, prefix: "", suffix: "", position: null, ...extra });

// Every annotation is about words somebody selected, so a selector with none
// is not a place in the document -- it is nothing to look for.
{
  check("a selector with no words anchors nowhere",
    anchorOne("before after", source("chapter.typ", "")) === null);
}

/* -------------------------------------------------------------- placeSources */

{
  const paths = new Map([["file-1", "chapter.typ"]]);
  const session = { paths };
  const sourceText = (status, range) => ({
    original_anchor: { kind: "source_text", checkpoint_id: "abc", target: { file_id: "file-1", exact: "the passage" } },
    attachment: { status, resolved_range_utf16: range },
  });
  const comments = [
    sourceText("exact", [7, 18]),
    sourceText("modified", [9, 21]),
    sourceText("deleted", null),
    sourceText("ambiguous", [7, 18]),
    { original_anchor: { kind: "document", checkpoint_id: "abc" } },
  ];
  const { anchored, orphaned } = placeSources(session, comments);
  check("a passage the server placed is placed here", anchored === 2);
  check("and it is placed exactly where the server said", comments[0].sourceStart === 7 && comments[0].sourceEnd === 18);
  check("an edited passage is still placed", comments[1].sourceStart === 9);
  check("a deleted passage is not placed anywhere", comments[2].sourceStart === null);
  check("an ambiguous passage is not placed either", comments[3].sourceStart === null);
  check("both unplaced passages are counted as such", orphaned === 2);
  check("a comment on the document as a whole has no passage to place", comments[4].sourceStart === null);
}

{
  // A file id this browser does not know -- the document has moved on, or
  // this is a reader who was never sent the source -- places nothing rather
  // than placing it at an offset into some other file.
  const comments = [{
    original_anchor: { kind: "source_text", checkpoint_id: "abc", target: { file_id: "file-gone", exact: "x" } },
    attachment: { status: "exact", resolved_range_utf16: [0, 1] },
  }];
  placeSources({ paths: new Map() }, comments);
  check("an unknown file id places nothing", comments[0].sourcePath === null);
}

/* ------------------------------------------------------------ shownSelector */

{
  const shown = shownSelector({
    presentation: {
      rendered_exact: "the passage",
      rendered_prefix: "before ",
      rendered_suffix: " after",
      rendered_position_utf16: 7,
    },
  });
  check("the display selector is the rendered quotation", shown.exact === "the passage" && shown.prefix === "before ");
  check("the recorded position comes with it, to break ties between repeats",
    shown.position === 7);
  const bare = shownSelector({});
  check("a comment with no presentation at all selects nothing",
    bare.exact === "" && bare.position === null);
}

{
  const oldPublication = { exact: "passage", position: 0, requireUnique: true };
  check("old-publication positions cannot break an ambiguous quote tie",
    anchorOne("passage and passage", oldPublication) === null);
  check("an old publication follows a uniquely moved quote",
    anchorOne("new introduction passage", oldPublication)?.start === 17);
  check("distinct rendered context can select a repeated old-publication quote",
    anchorOne("left passage right; other passage end", { ...oldPublication, prefix: "other ", suffix: " end" })?.start === 26);
  check("a missing old-publication quote remains unmatched",
    anchorOne("a replacement paragraph", oldPublication) === null);
}

if (failures) process.exit(1);
console.log("anchor: rendered quotations are matched, source ranges are taken as given");
