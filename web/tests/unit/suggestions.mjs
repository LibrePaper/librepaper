// Suggestions (track changes), the browser half: the card's diff runs, the
// modal's prefill rule, and the accept/error state transitions, as pure
// functions.

import {
  applyDecision,
  applyProposal,
  beginDeciding,
  clearDeciding,
  locateInText,
  prefillFor,
  runsFor,
} from "../../src/lib/suggestions.js";

let failures = 0;
function check(what, condition) {
  if (condition) return;
  failures += 1;
  console.error(`suggestions: ${what}`);
}

/* ------------------------------------------------------------- the prefill */

{
  check(
    "the source anchor's own quotation wins when there is one",
    prefillFor({ exact: "rendered words", source: { exact: "source words" } }) === "source words",
  );
  check(
    "the rendered quotation is the fallback with no source anchor",
    prefillFor({ exact: "rendered words", source: null }) === "rendered words",
  );
  check("no pending selection prefills empty", prefillFor(null) === "");
}

/* ------------------------------------------------------------- the diff runs */

{
  // A pure stand-in for `history.wordDiff`'s edits: replacing "quick" with
  // "slow" in "the quick fox".
  const old = "the quick fox";
  const edits = [{ at: 4, delete: 5, insert: "slow" }];
  const runs = runsFor(old, edits);
  check(
    "an edit in the middle keeps the words on either side as same-runs",
    runs[0].kind === "same" && runs[0].text === "the " &&
      runs[runs.length - 1].kind === "same" && runs[runs.length - 1].text === " fox",
  );
  check(
    "the replaced word is a del-run immediately followed by an ins-run",
    runs.some((run, i) => run.kind === "del" && run.text === "quick" && runs[i + 1]?.kind === "ins" && runs[i + 1].text === "slow"),
  );
}

{
  // An empty proposal deletes the passage. `wordDiff` against "" comes back
  // as one edit deleting everything, so no special case is needed here.
  const old = "the whole passage";
  const edits = [{ at: 0, delete: old.length, insert: "" }];
  const runs = runsFor(old, edits);
  check(
    "an empty proposal is a single del-run over the whole quotation",
    runs.length === 1 && runs[0].kind === "del" && runs[0].text === old,
  );
}

{
  check("no edits at all is a single same-run", runsFor("unchanged", []).length === 1 && runsFor("unchanged", [])[0].kind === "same");
}

/* ------------------------------------------------------- locating the anchor */

{
  const text = "one two three";
  check("a unique quotation is found where it is", (() => {
    const at = locateInText(text, { exact: "two" });
    return at && text.slice(at.start, at.end) === "two";
  })());
  check("a missing quotation is not found", locateInText(text, { exact: "four" }) === null);
}

{
  // "cat" occurs twice; the prefix/suffix context picks the second one.
  const text = "a cat sat, a cat ran";
  const at = locateInText(text, { exact: "cat", prefix: "a ", suffix: " ran" });
  check("ties are broken by how well the context matches", at && text.slice(at.end, at.end + 4) === " ran");
}

{
  const text = "a cat sat, a cat sat";
  const at = locateInText(text, { exact: "cat", prefix: "", suffix: "", position: 18 });
  check("an equal context match falls back to distance from position", at && at.start === 13);
}

/* ---------------------------------------------------- applying the proposal */

{
  const text = "the quick fox jumps";
  const replaced = applyProposal(text, { exact: "quick" }, "slow");
  check("the proposal replaces the located quotation", replaced === "the slow fox jumps");
}

{
  const text = "the quick fox jumps";
  check("an empty proposal deletes the quotation", applyProposal(text, { exact: "quick " }, "") === "the fox jumps");
}

{
  const text = "no match here";
  check("a quotation absent from the text leaves it unchanged", applyProposal(text, { exact: "gone" }, "x") === text);
}

/* --------------------------------------------------------- deciding a card */

{
  const comment = { id: "c1", resolved: false };
  beginDeciding(comment, "accept");
  check("beginning a decision marks the card busy, not resolved", comment.deciding === "accept" && comment.resolved === false);

  applyDecision(comment, { resolved_in: "4f2a91c", resolved_at: "2026-09-07T00:00:00Z" }, "accepted");
  check("a settled accept resolves the card with its outcome and checkpoint", comment.resolved === true && comment.outcome === "accepted" && comment.resolved_in === "4f2a91c" && !("deciding" in comment));
}

{
  const comment = { id: "c2", resolved: false };
  beginDeciding(comment, "reject");
  applyDecision(comment, { resolved_at: "2026-09-07T00:00:00Z" }, "rejected");
  check("a settled reject resolves the card as rejected", comment.resolved === true && comment.outcome === "rejected");
}

{
  const comment = { id: "c3", resolved: false, deciding: "accept" };
  clearDeciding(comment);
  check("an error clears the busy state without resolving the card", !("deciding" in comment) && comment.resolved === false);
}

if (failures) {
  console.error(`suggestions: ${failures} check(s) failed`);
  process.exit(1);
}
