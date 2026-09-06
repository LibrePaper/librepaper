// Where a passage went, against a history the test writes.
//
// The search is a bisection over the checkpoints between the one a comment was
// made on and the one on the screen, and what makes it correct is that "found"
// only goes one way. What is checked here is that it lands on the first moment
// the passage is missing rather than on any moment it is missing, that it says
// nothing when there is nothing to say, and that it costs a handful of
// renders rather than one per checkpoint -- which is the whole reason it is a
// bisection and not a walk.

import { wentAt } from "../src/lib/passages.js";

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

if (failures) process.exit(1);
console.log("passages: the first moment a passage is missing, found in a handful of renders");
