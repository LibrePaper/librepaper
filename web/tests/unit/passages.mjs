// Where a passage went, against a history the test writes.
//
// The search scans labels after the comment frontier in order because later
// edits can reintroduce a passage after it disappears.

import { replacementAt, sourceTextAt, tracedBy, wentAt, wentAtMany } from "../../src/lib/passages.js";
import { hunks } from "../../src/lib/history.js";
import { MOMENT } from "../../src/lib/moment.js";

let failures = 0;
function check(what, condition) {
  if (condition) return;
  failures += 1;
  console.error(`passages: ${what}`);
}

/// A comment with one stable source-file identity, anchored to the given
/// frontier (§8.2 dropped `checkpoint_id`; a comment carries its own
/// `source_sequence` and `frontier` now).
const comment = (frontier, created = "2026-09-05T09:10:30Z") => ({
  created,
  original_anchor: {
    kind: "source_text",
    source_sequence: 1,
    frontier,
    target: {
      file_id: "file-1",
      exact: "the passage of interest",
      prefix: "before ",
      suffix: " after",
    },
  },
});
const inSource = (frontier, created) => tracedBy(comment(frontier, created), new Map([["file-1", "chapter.typ"]]));

/// Label texts stand in for what `sourceTextAt` returns by file ID. The
/// sentinel `own` frontier always holds -- every fixture below reads it for
/// the comment's own recorded position before bisecting the manifest.
function sourceHistory(n, until) {
  const { labels, texts, looked } = (() => {
    const labels = Array.from({ length: n }, (_, at) => ({
      sha: String(at).padStart(64, "0"),
      at: `2026-09-05T09:${String(at).padStart(2, "0")}:00Z`,
      by: "vincent",
      why: "quiet",
      label: "",
    }));
    const looked = new Set();
    const texts = new Map(
      labels.map((point, at) => [
        point.sha,
        at <= until ? "before the passage of interest after" : "before after",
      ]),
    );
    texts.set(`${MOMENT}own`, "before the passage of interest after");
    return { labels, texts, looked };
  })();
  return {
    labels,
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

// Source label responses are scoped by document and failed requests are
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
    check("failed source label fetches are retried", failed && a === "A");
    check("source label cache keys include the document", b === "B" && documentFetches === 3);
    // A label already fetched under one authorization is reused by later
    // comments asking about that moment, and never reused under another. The
    // scope is part of the key, so a second link key fetches its own copy
    // rather than reading one it was never shown.
    check("a label is reused within one link context",
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
    check("source label cache is bounded", sourceFetches === 66);
  } finally {
    globalThis.fetch = oldFetch;
  }
}

{
  const oldFetch = globalThis.fetch;
  const fetched = new Map();
  const firstAt = Date.UTC(2026, 8, 5, 10, 0, 0);
  const labels = Array.from({ length: 90 }, (_, at) => ({
    sha: String(at).padStart(64, "0"),
    at: new Date(firstAt + at * 1000).toISOString(),
  }));
  globalThis.fetch = async (url) => {
    const sha = decodeURIComponent(new URL(String(url), "https://example.test").pathname.split("/history/").at(-1));
    fetched.set(sha, (fetched.get(sha) || 0) + 1);
    const index = Number(sha);
    const text = sha.startsWith("frontier:") || index < 70
      ? "before the passage of interest after" : "before after";
    return {
      ok: true,
      json: async () => ({
        files: { "main.md": { kind: "text", id: "file-1" } },
        texts: { "main.md": text },
      }),
    };
  };
  try {
    const traced = [0, 20, 40].map((at) => ({
      ...inSource(`parallel-history-${at}`, new Date(firstAt + (at - 1) * 1000).toISOString()),
      created: new Date(firstAt + (at - 1) * 1000).toISOString(),
    }));
    const answers = await wentAtMany("parallel-history", traced, labels);
    check("parallel comments share one request per label beyond the bounded cache window",
      answers.every((answer) => answer?.sha === labels[70].sha)
        && fetched.size === 75
        && [...fetched.values()].every((count) => count === 1));
  } finally {
    globalThis.fetch = oldFetch;
  }
}

/* ---------------------------------------------------------- source anchors */

{
  const { labels, atSource, looked } = sourceHistory(32, 20);
  const found = await wentAt("slug", inSource("own", "2026-09-05T08:00:00Z"), labels, {}, atSource);
  check("a source anchor finds the first moment the source no longer holds it", found?.sha === labels[21].sha);
  check(`the first loss scan stops at label 21 (looked at ${looked.size} including the newest-label probe)`, looked.size === 24);
}

{
  let reads = 0;
  const traced = inSource("stale-frontier", "2026-09-05T09:00:00Z");
  const result = await wentAtMany("slug", [traced], sourceHistory(90, 80).labels, {}, async () => {
    reads++;
    return "before the passage of interest after";
  }, () => false);
  check("a stale passage batch stops after its current projection", reads === 1 && result[0] === null);
}

/* ------------------------------------------------------------ the answer */

{
  const { labels, atSource, looked } = sourceHistory(32, 20);
  const found = await wentAt("slug", inSource("own", labels[10].at), labels, {}, atSource);
  check("the comment's own frontier starts the source search after its timestamp", found?.sha === labels[21].sha);
  check("source history stops at the first missing label", looked.size === 14);
}

{
  const { labels, atSource } = sourceHistory(8, 7);
  check("a surviving source passage has no loss point",
    await wentAt("slug", inSource("own", "2026-09-05T08:00:00Z"), labels, {}, atSource) === null);
  check("an empty history has no loss point",
    await wentAt("slug", inSource("own", "2026-09-05T08:00:00Z"), [], {}, atSource) === null);
  let emptyReads = 0;
  check("a comment with no recorded frontier never becomes a history request",
    await wentAt("slug", inSource(""), labels, {}, async () => { emptyReads += 1; }) === null
      && emptyReads === 0);
}

{
  const { labels } = sourceHistory(12, 11);
  const texts = new Map(labels.map((point, index) => [point.sha,
    index < 4 || (index >= 7 && index < 10)
      ? "before the passage of interest after" : "before after",
  ]));
  const frontier = inSource("comment-frontier-before-removal", "2026-09-05T09:07:30Z");
  const found = await wentAt("slug", frontier, labels, {}, async (_slug, sha) =>
    sha === `${MOMENT}comment-frontier-before-removal` ? "before the passage of interest after" : texts.get(sha));
  check("a frontier ignores removals before the comment and finds the later loss", found?.sha === labels[10].sha);
}

{
  const { labels } = sourceHistory(12, 11);
  const texts = new Map(labels.map((point, index) => [point.sha,
    index === 8 || index === 10
      ? "before the passage of interest after" : "before after",
  ]));
  const frontier = inSource("comment-frontier", "2026-09-05T09:07:30Z");
  const found = await wentAt("slug", frontier, labels, {}, async (_slug, sha) =>
    sha === `${MOMENT}comment-frontier` ? "before the passage of interest after" : texts.get(sha));
  check("a frontier finds the first removal even when the passage later returns", found?.sha === labels[9].sha);
}

{
  const { labels } = sourceHistory(12, 11);
  const texts = new Map(labels.map((point, index) => [point.sha,
    index === 8 || index === 10 || index === 11
      ? "before the passage of interest after" : "before after",
  ]));
  const frontier = inSource("comment-frontier-current", "2026-09-05T09:07:30Z");
  const found = await wentAt("slug", frontier, labels, {}, async (_slug, sha) =>
    sha === `${MOMENT}comment-frontier-current` ? "before the passage of interest after" : texts.get(sha));
  check("a passage present again at the newest label has no current loss point", found === null);
}

check("a selected part of a rewritten word has no identifiable replacement",
  await replacementAt("infrared", "ultraviolet", { exact: "red" },
    async () => [{ at: 0, delete: 8, insert: "ultraviolet" }]) === null);

/* ------------------------------------------------------- the live route */

// `tracedBy` reads the anchor fields §8.2 actually left behind
// (`source_sequence`, `frontier`) rather than the dropped `checkpoint_id`.
{
  const traced = tracedBy(comment("AAAA/BBBB+CC==", "2026-09-05T09:10:30Z"), new Map([["file-1", "chapter.typ"]]));
  check("tracedBy reads the frontier the anchor actually carries", traced.frontier === "AAAA/BBBB+CC==");
}

// A comment's own frontier and a label answer through the same route and the
// same fetch, so a passage lookup on the live draft needs one branch, not
// two: `sourceTextAt` never calls a second module for `frontier:`-prefixed
// morments, and it encodes one so the base64 alphabet survives the URL.
{
  const oldFetch = globalThis.fetch;
  let requested = null;
  globalThis.fetch = async (url) => {
    requested = String(url);
    return { ok: true, json: async () => ({ files: {}, texts: { "chapter.typ": "the passage" } }) };
  };
  try {
    const sha = `${MOMENT}AAAA/BBBB+CC==`;
    await sourceTextAt("live-route-test", sha, "chapter.typ");
    check("a comment's own frontier is read through the history route",
      requested === `/api/documents/live-route-test/history/${encodeURIComponent(sha)}`);
    check("the frontier's base64 alphabet is escaped, not passed through raw",
      requested.includes("%2F") && requested.includes("%2B") && requested.includes("%3D"));
  } finally {
    globalThis.fetch = oldFetch;
  }
}

if (failures) process.exit(1);
console.log("passages: source identity survives renames and finds the loss point in a handful of labels");
