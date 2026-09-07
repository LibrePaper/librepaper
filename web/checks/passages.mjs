// Where a passage went, against a history the test writes.
//
// The search is a bisection over the checkpoints between the one a comment was
// made on and the one on the screen, and what makes it correct is that "found"
// only goes one way. What is checked here is that it lands on the first moment
// the passage is missing rather than on any moment it is missing, that it says
// nothing when there is nothing to say, and that it costs a handful of
// renders rather than one per checkpoint -- which is the whole reason it is a
// bisection and not a walk.

import { replacementAt, sourceTextAt, textAt, wentAt } from "../src/lib/passages.js";
import { hunks } from "../src/lib/history.js";

let failures = 0;
function check(what, condition) {
  if (condition) return;
  failures += 1;
  console.error(`passages: ${what}`);
}

/// A history of `n` checkpoints in which the passage is present up to `until`
/// and gone after it, with a counter of how many were actually looked at.
function history(n, until) {
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
  return {
    checkpoints,
    looked,
    // The same shape `passages.textAt` has, so the search under test is the
    // one that runs in a browser and only the fetching is stubbed.
    at: async (_slug, sha) => {
      looked.add(sha);
      return texts.get(sha);
    },
  };
}

const comment = (revision) => ({
  exact: "the passage of interest",
  prefix: "before ",
  suffix: " after",
  position: null,
  revision,
});

/// The same history, but read as source files rather than renderings -- the
/// texts at each checkpoint stand in for what `sourceTextAt` would return for
/// one path, and `at` is left to blow up if `wentAt` ever calls it, since a
/// comment with a source anchor has no business asking for a render.
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
    at: async () => {
      throw new Error("wentAt must not render for a comment with a source anchor");
    },
    atSource: async (_slug, sha, path) => {
      looked.add(sha);
      return path === "chapter.typ" ? texts.get(sha) : null;
    },
  };
}

const sourceComment = (revision) => ({
  revision,
  source: {
    path: "chapter.typ",
    exact: "the passage of interest",
    prefix: "before ",
    suffix: " after",
    position: null,
  },
});

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

// Historical Typst checkpoints already have a stored PDF. Passage lookup
// must read that artifact directly, just like LaTeX, without trying to warm a
// browser compiler or falling through to the HTML renderer.
{
  let compiled = 0;
  let gathered = 0;
  let pdfRead = 0;
  const text = await textAt(
    "slug",
    "typst-checkpoint",
    {},
    {
      history: { checkpoint: async () => ({ main: "main.typ", texts: { "main.typ": "#page" }, files: {} }) },
      renderers: {
        formatOf: () => "typst",
        producesPdf: () => true,
        render: async () => { compiled++; return { html: "<p>wrong path</p>" }; },
      },
      figures: { gather: async () => { gathered++; return { assets: {}, urls: {} }; } },
      fetch: async () => ({ ok: true, arrayBuffer: async () => Uint8Array.of(1, 2, 3).buffer }),
      pdfText: async (bytes) => { pdfRead += bytes.byteLength; return "stored Typst page"; },
    },
  );
  check("historical Typst passages read stored PDF text", text === "stored Typst page");
  check("historical Typst passage lookup does not compile or gather figures", compiled === 0 && gathered === 0);
  check("historical Typst passage lookup consumed the PDF artifact", pdfRead === 3);
}

// A missing rendering is unknown only for that attempt. Once the artifact is
// uploaded, the same history lookup must retry it rather than serving a
// permanently cached null; separate documents with the same checkpoint SHA
// also have independent rendering caches.
{
  let available = false;
  let fetches = 0;
  const services = {
    history: { checkpoint: async (slug) => ({ main: "main.typ", texts: { "main.typ": slug }, files: {} }) },
    renderers: { formatOf: () => "typst", producesPdf: () => true },
    fetch: async () => {
      fetches++;
      return available
        ? { ok: true, arrayBuffer: async () => Uint8Array.of(9).buffer }
        : { ok: false };
    },
    pdfText: async () => "recovered PDF text",
  };
  const missing = await textAt("retry-doc", "same-sha", {}, services);
  available = true;
  const recovered = await textAt("retry-doc", "same-sha", {}, services);
  const otherDocument = await textAt("other-doc", "same-sha", {}, services);
  check("a missing historical PDF is retried after it appears", missing === null && recovered === "recovered PDF text");
  check("historical PDF cache keys include the document", otherDocument === "recovered PDF text" && fetches === 3);
}

// Source checkpoint responses are scoped by document and failed requests are
// retryable. A shared SHA is possible across documents, and a rejected
// promise must not become a permanent access or availability failure.
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
    const keyed = await sourceTextAt("doc-a", sha, "main.md", { "X-Komodoc-Key": "one" });
    const keyedAgain = await sourceTextAt("doc-a", sha, "main.md", { "X-Komodoc-Key": "one" });
    const otherKey = await sourceTextAt("doc-a", sha, "main.md", { "X-Komodoc-Key": "two" });
    check("failed source checkpoint fetches are retried", failed && a === "A");
    check("source checkpoint cache keys include the document", b === "B" && documentFetches === 3);
    check("source checkpoint cache keys include link context", keyed === "A" && keyedAgain === "A" && otherKey === "A" && fetches === 5);
  } finally {
    globalThis.fetch = oldFetch;
  }
}

// Successful rendered and source entries are bounded. The first entry falls
// out after enough distinct documents have been visited and is fetched again.
{
  let renderedFetches = 0;
  const services = {
    history: {
      checkpoint: async () => {
        renderedFetches += 1;
        return { main: "main.typ", texts: { "main.typ": "#page" }, files: {} };
      },
    },
    renderers: { formatOf: () => "typst", producesPdf: () => true },
    fetch: async () => ({ ok: true, arrayBuffer: async () => Uint8Array.of(1).buffer }),
    pdfText: async () => "page",
  };
  for (let at = 0; at < 65; at += 1) {
    await textAt(`bounded-${at}`, "same-sha", {}, services);
  }
  await textAt("bounded-0", "same-sha", {}, services);
  check("rendered checkpoint cache is bounded", renderedFetches === 66);
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
  const { checkpoints, at, atSource, looked } = sourceHistory(32, 20);
  const found = await wentAt("slug", sourceComment(checkpoints[0].sha), checkpoints, {}, at, atSource);
  check("a source anchor finds the first moment the source no longer holds it", found?.sha === checkpoints[21].sha);
  check(`a bisection over the source, not a walk (looked at ${looked.size} of 32)`, looked.size <= 8);
}

/* ------------------------------------------------------------ the answer */

{
  const { checkpoints, at, looked } = history(32, 20);
  const found = await wentAt("slug", comment(checkpoints[0].sha), checkpoints, {}, at);
  check("the first moment the passage is missing", found?.sha === checkpoints[21].sha);
  check(
    `a bisection, not a walk (looked at ${looked.size} of 32)`,
    looked.size <= 8,
  );
}

{
  // A comment made after the passage had already been through some history
  // starts from its own checkpoint and finds the same moment.
  const { checkpoints, at } = history(32, 20);
  const found = await wentAt("slug", comment(checkpoints[10].sha), checkpoints, {}, at);
  check("the comment's own checkpoint is where the search starts", found?.sha === checkpoints[21].sha);
}

{
  // A comment whose checkpoint the manifest no longer has is read as made on
  // the oldest moment there is, which is the earliest thing that can be true.
  const { checkpoints, at } = history(32, 20);
  const found = await wentAt("slug", comment("nowhere"), checkpoints, {}, at);
  check("a forgotten checkpoint falls back to the oldest", found?.sha === checkpoints[21].sha);
}

/* ------------------------------------------------- when there is nothing */

{
  const { checkpoints, at } = history(8, 7);
  const found = await wentAt("slug", comment(checkpoints[0].sha), checkpoints, {}, at);
  check("a passage still in the document is not reported as gone", found === null);
}

{
  const { checkpoints, at } = history(8, -1);
  const found = await wentAt("slug", comment(checkpoints[0].sha), checkpoints, {}, at);
  check("a passage that was never there names no moment", found === null);
}

{
  // A checkpoint without a usable rendering is unknown. It must not be
  // treated as an empty page and reported as the moment a passage vanished.
  const { checkpoints, at } = history(8, 4);
  const unknown = async (_slug, sha) => (sha === checkpoints[3].sha ? null : at(_slug, sha));
  const found = await wentAt("slug", comment(checkpoints[0].sha), checkpoints, {}, unknown);
  check("an unavailable checkpoint is unknown rather than empty", found === null);
}

{
  const { checkpoints, at } = history(1, -1);
  const found = await wentAt("slug", comment(checkpoints[0].sha), checkpoints, {}, at);
  check("a history of one has nothing to say", found === null);
  check("an empty history has nothing to say", (await wentAt("slug", comment(""), [], {}, at)) === null);
}

check("a selected part of a rewritten word has no identifiable replacement",
  await replacementAt("infrared", "ultraviolet", { exact: "red" },
    async () => [{ at: 0, delete: 8, insert: "ultraviolet" }]) === null);

if (failures) process.exit(1);
console.log("passages: the first moment a passage is missing, found in a handful of renders");
