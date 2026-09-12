// Where a passage went, against a history the test writes.
//
// The search is a bisection over the checkpoints between the one a comment was
// made on and the one on the screen, and what makes it correct is that "found"
// only goes one way. What is checked here is that it lands on the first moment
// the passage is missing rather than on any moment it is missing, that it says
// nothing when there is nothing to say, and that it costs a handful of
// renders rather than one per checkpoint -- which is the whole reason it is a
// bisection and not a walk.

import { replacementAt, sourceTextAt, htmlAt, renderTree, wentAt } from "../../src/lib/passages.js";
import { hunks } from "../../src/lib/history.js";

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

// Every historical format uses contemporary HTML, never its stored PDF.
{
  let compiled = 0;
  let gathered = 0;
  let pdfRead = 0;
  const text = await htmlAt(
    "slug",
    "typst-checkpoint",
    {},
    {
      history: { checkpoint: async () => ({ main: "main.typ", texts: { "main.typ": "#page" }, files: {} }) },
      renderers: {
        formatOf: () => "typst",
        producesPdf: () => true,
        render: async (_tree, _title, options) => { check("history explicitly requests HTML", options.format === "html"); compiled++; return { html: "<p>contemporary page</p>" }; },
      },
      figures: { gather: async () => { gathered++; return { assets: {}, urls: {} }; } },
      fetch: async () => ({ ok: true, arrayBuffer: async () => Uint8Array.of(1, 2, 3).buffer }),
      pdfText: async (bytes) => { pdfRead += bytes.byteLength; return "stored Typst page"; },
    },
  );
  check("historical Typst passages use contemporary HTML", text === "<p>contemporary page</p>");
  check("historical Typst passage lookup compiles the captured tree", compiled === 1 && gathered === 1);
  check("historical Typst passage lookup never consumes PDF artifacts", pdfRead === 0);
}

// A failed historical page lookup is unknown only for that attempt. A later
// request retries it, and separate documents with the same checkpoint SHA
// do not share generated output.
{
  let available = false;
  let fetches = 0;
  const services = {
    history: { checkpoint: async (slug) => ({ main: "main.typ", texts: { "main.typ": slug }, files: {} }) },
    figures: { gather: async () => ({ assets: {}, urls: {} }) },
    renderers: { render: async () => {
      fetches++;
      if (!available) throw new Error("HTML unavailable");
      return { html: "recovered HTML" };
    } },
  };
  let missing = false;
  try { await htmlAt("retry-doc", "same-sha", {}, services); } catch { missing = true; }
  available = true;
  const recovered = await htmlAt("retry-doc", "same-sha", {}, services);
  const otherDocument = await htmlAt("other-doc", "same-sha", {}, services);
  check("a failed historical HTML render is retried", missing && recovered === "recovered HTML");
  check("historical HTML cache keys include the document", otherDocument === "recovered HTML" && fetches === 3);
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
    const keyed = await sourceTextAt("doc-a", sha, "main.md", { "X-LibrePaper-Key": "one" });
    const keyedAgain = await sourceTextAt("doc-a", sha, "main.md", { "X-LibrePaper-Key": "one" });
    const otherKey = await sourceTextAt("doc-a", sha, "main.md", { "X-LibrePaper-Key": "two" });
    check("failed source checkpoint fetches are retried", failed && a === "A");
    check("source checkpoint cache keys include the document", b === "B" && documentFetches === 3);
    check("source checkpoint requests revalidate each link context", keyed === "A" && keyedAgain === "A" && otherKey === "A" && fetches === 6);
  } finally {
    globalThis.fetch = oldFetch;
  }
}

// Historical page output is transient. Each completed request releases its
// generated HTML, so revisiting a checkpoint performs a fresh render.
{
  let renderedFetches = 0;
  let renders = 0;
  const services = {
    history: {
      checkpoint: async () => {
        renderedFetches += 1;
        return { main: "main.typ", texts: { "main.typ": "#page" }, files: {} };
      },
    },
    renderers: { render: async () => { renders += 1; return { html: "page" }; } },
    figures: { gather: async () => ({ assets: {}, urls: {} }) },
  };
  for (let at = 0; at < 65; at += 1) {
    await htmlAt(`bounded-${at}`, "same-sha", {}, services);
  }
  await htmlAt("bounded-0", "same-sha", {}, services);
  check("historical page output is not retained", renderedFetches === 66 && renders === 66);
}

// Renderer configuration is part of the historical render identity. A
// release/module change must not reuse the old page, and each page must carry
// the exact snapshot whose identity was used for its cache entry.
{
  let identity = "renderer-a";
  let renders = 0;
  const seen = [];
  const services = {
    history: { checkpoint: async () => ({ sha: "config-sha", main: "main.md", texts: { "main.md": "source" }, files: {} }) },
    figures: { gather: async () => ({ assets: {}, urls: {} }) },
    renderers: {
      htmlConfiguration: async () => ({ identity, modules: { markdown: identity } }),
      render: async (_tree, _title, options) => {
        renders += 1;
        seen.push(options.configuration);
        return { html: `<p>${options.configuration.modules.markdown}</p>` };
      },
    },
  };
  const first = await htmlAt("config-doc", "config-sha", {}, services);
  identity = "renderer-b";
  const second = await htmlAt("config-doc", "config-sha", {}, services);
  const repeated = await renderTree("config-doc", { sha: "config-sha", main: "main.md", texts: { "main.md": "source" }, files: {} }, {}, services);
  check("renderer configuration snapshots are used by the render", first === "<p>renderer-a</p>" && second === "<p>renderer-b</p>");
  check("renderer configuration is applied to each transient render", renders === 3 && repeated.rendererIdentity === "renderer-b");
  check("transient renders retain the configuration they name", seen[0]?.modules.markdown === "renderer-a" && seen[1]?.modules.markdown === "renderer-b" && seen[2]?.modules.markdown === "renderer-b");
}

// Only captured, integrity-checked asset bytes become projection evidence.
// The result carries path/digest/URL metadata without exposing the bytes.
{
  const digest = "c".repeat(64);
  const result = await renderTree("asset-doc", {
    sha: "asset-sha", main: "main.md", texts: { "main.md": "![figure](fig.png)" },
    digests: { "fig.png": digest }, files: { "fig.png": { kind: "asset", sha: digest } },
  }, {}, {
    figures: { gather: async () => ({ assets: { "fig.png": Uint8Array.of(4) }, urls: { "fig.png": "blob:captured#librepaper-asset=" + digest } }) },
    renderers: { render: async () => ({ html: "<p>figure</p>" }) },
  });
  check("render results carry captured asset evidence", result.assetEvidence?.paths?.["fig.png"]?.digest === digest);
  check("asset evidence does not carry bytes", !Object.prototype.hasOwnProperty.call(result.assetEvidence.paths["fig.png"], "bytes"));
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
