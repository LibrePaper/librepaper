// Where a passage went, against a history the test writes.
//
// The search is a bisection over the checkpoints between the one a comment was
// made on and the one on the screen, and what makes it correct is that "found"
// only goes one way. What is checked here is that it lands on the first moment
// the passage is missing rather than on any moment it is missing, that it says
// nothing when there is nothing to say, and that it costs a handful of
// checkpoint reads rather than one per checkpoint -- which is why it is a
// bisection and not a walk.

import { replacementAt, sourceTextAt, tracedBy, wentAt } from "../../src/lib/passages.js";
import { hunks } from "../../src/lib/history.js";

let failures = 0;
function check(what, condition) {
  if (condition) return;
  failures += 1;
  console.error(`passages: ${what}`);
}

/// A comment with one stable source-file identity.
const comment = (checkpoint) => ({
  created: "2026-09-05T09:10:30Z",
  original_anchor: {
    kind: "source_text",
    checkpoint_id: checkpoint,
    target: {
      file_id: "file-1",
      exact: "the passage of interest",
      prefix: "before ",
      suffix: " after",
    },
  },
});
const inSource = (checkpoint) => tracedBy(comment(checkpoint), new Map([["file-1", "chapter.typ"]]));

/// Checkpoint texts stand in for what `sourceTextAt` returns by file ID.
function sourceHistory(n, until) {
  const { checkpoints, texts, looked } = (() => {
    const checkpoints = Array.from({ length: n }, (_, at) => ({
      sha: String(at).padStart(64, "0"),
      at: `2026-09-05T09:${String(at).padStart(2, "0")}:00Z`,
      by: "vincent",
      why: "quiet",
      label: "",
    }));
    const looked = new Set();
    const texts = new Map(
      checkpoints.map((point, at) => [
        point.sha,
        at <= until ? "before the passage of interest after" : "before after",
      ]),
    );
    return { checkpoints, texts, looked };
  })();
  return {
    checkpoints,
    looked,
    atSource: async (_slug, sha, file) => {
      looked.add(sha);
      return file?.file_id === "file-1" ? texts.get(sha) : null;
    },
  };
}

/* ------------------------------------------------------------ replacements */

{
  const oldText = "The 🦎 estimator is unbiased under the model.";
  const newText = "The 🦎 estimator is consistent under the model.";
  const selector = {
    exact: "estimator is unbiased",
    prefix: "The 🦎 ",
    suffix: " under",
    position: 7,
  };
  const edits = [{ at: 7, delete: 21, insert: "estimator is consistent" }];
  const replacement = await replacementAt(oldText, newText, selector, async () => edits);
  check("replacement lookup uses a quoted UTF-16 range", replacement === "estimator is consistent");
  const displayed = hunks(oldText, newText, edits, 2);
  check("hunks preserve inserted text and context", displayed[0].current === "estimator is consistent");
  check("hunks expose the replacement kind", displayed[0].kind === "replace");
}

// Source checkpoint responses are scoped by document and failed requests are
// retryable. A shared SHA is possible across documents, and a rejected
// promise must not become a permanent access or availability failure.
{
  const oldFetch = globalThis.fetch;
  globalThis.fetch = async (url) => {
    const renamed = String(url).includes("new-name");
    const path = renamed ? "new.md" : "old.md";
    return { ok: true, json: async () => ({
      files: { [path]: { kind: "text", id: "file-1" } },
      texts: { [path]: "the same passage" },
    }) };
  };
  try {
    const old = await sourceTextAt("rename-test", "old-name", { file_id: "file-1" });
    const renamed = await sourceTextAt("rename-test", "new-name", { file_id: "file-1" });
    check("historical source lookup follows file identity across a rename",
      old === "the same passage" && renamed === old);
  } finally {
    globalThis.fetch = oldFetch;
  }
}

{
  const oldFetch = globalThis.fetch;
  const sha = "b".repeat(64);
  let fetches = 0;
  globalThis.fetch = async (url) => {
    fetches += 1;
    if (fetches === 1) return { ok: false, json: async () => ({}) };
    return {
      ok: true,
      json: async () => ({ texts: { "main.md": String(url).includes("doc-b") ? "B" : "A" } }),
    };
  };
  try {
    let failed = false;
    try {
      await sourceTextAt("doc-a", sha, "main.md");
    } catch {
      failed = true;
    }
    const a = await sourceTextAt("doc-a", sha, "main.md");
    const b = await sourceTextAt("doc-b", sha, "main.md");
    const documentFetches = fetches;
    const keyed = await sourceTextAt("doc-a", sha, "main.md", { "X-LibrePaper-Key": "one" });
    const keyedAgain = await sourceTextAt("doc-a", sha, "main.md", { "X-LibrePaper-Key": "one" });
    const reusedFetches = fetches;
    const otherKey = await sourceTextAt("doc-a", sha, "main.md", { "X-LibrePaper-Key": "two" });
    check("failed source checkpoint fetches are retried", failed && a === "A");
    check("source checkpoint cache keys include the document", b === "B" && documentFetches === 3);
    // A checkpoint already fetched under one authorization is reused under
    // that same authorization -- a bisection asks about the same handful of
    // moments for every comment it walks -- and never under another. The
    // scope is part of the key, so a second link key fetches its own copy
    // rather than reading one it was never shown.
    check("a checkpoint is reused within one link context",
      keyed === "A" && keyedAgain === "A" && reusedFetches === 4);
    check("another link context never reads the first one's tree",
      otherKey === "A" && fetches === 5);
  } finally {
    globalThis.fetch = oldFetch;
  }
}

{
  const oldFetch = globalThis.fetch;
  let sourceFetches = 0;
  globalThis.fetch = async () => {
    sourceFetches += 1;
    return { ok: true, json: async () => ({ texts: { "main.md": "source" } }) };
  };
  try {
    for (let at = 0; at < 65; at += 1) {
      await sourceTextAt(`point-bounded-${at}`, "same-sha", "main.md");
    }
    await sourceTextAt("point-bounded-0", "same-sha", "main.md");
    check("source checkpoint cache is bounded", sourceFetches === 66);
  } finally {
    globalThis.fetch = oldFetch;
  }
}

/* ---------------------------------------------------------- source anchors */

{
  const { checkpoints, atSource, looked } = sourceHistory(32, 20);
  const found = await wentAt("slug", inSource(checkpoints[0].sha), checkpoints, {}, atSource);
  check("a source anchor finds the first moment the source no longer holds it", found?.sha === checkpoints[21].sha);
  check(`a bisection over the source, not a walk (looked at ${looked.size} of 32)`, looked.size <= 8);
}

/* ------------------------------------------------------------ the answer */

{
  const { checkpoints, atSource, looked } = sourceHistory(32, 20);
  const found = await wentAt("slug", inSource(checkpoints[10].sha), checkpoints, {}, atSource);
  check("the comment's own checkpoint starts the source search", found?.sha === checkpoints[21].sha);
  check("source history is bisected", looked.size <= 8);
}

{
  const { checkpoints, atSource } = sourceHistory(8, 7);
  check("a surviving source passage has no loss point",
    await wentAt("slug", inSource(checkpoints[0].sha), checkpoints, {}, atSource) === null);
  check("an empty history has no loss point",
    await wentAt("slug", inSource(""), [], {}, atSource) === null);
  let emptyReads = 0;
  check("an empty checkpoint never becomes a history request",
    await wentAt("slug", inSource(""), checkpoints, {}, async () => { emptyReads += 1; }) === null
      && emptyReads === 0);
}

{
  const { checkpoints } = sourceHistory(12, 11);
  const texts = new Map(checkpoints.map((point, index) => [
    point.sha,
    index < 4 || (index >= 7 && index < 10)
      ? "before the passage of interest after" : "before after",
  ]));
  const frontier = inSource("frontier:comment");
  frontier.created = "2026-09-05T09:07:30Z";
  const found = await wentAt("slug", frontier, checkpoints, {}, async (_slug, sha) =>
    sha === "frontier:comment" ? "before the passage of interest after" : texts.get(sha));
  check("a frontier ignores removals that happened before the comment", found?.sha === checkpoints[10].sha);
}

check("a selected part of a rewritten word has no identifiable replacement",
  await replacementAt("infrared", "ultraviolet", { exact: "red" },
    async () => [{ at: 0, delete: 8, insert: "ultraviolet" }]) === null);

if (failures) process.exit(1);
console.log("passages: source identity survives renames and finds the loss point in a handful of checkpoints");
