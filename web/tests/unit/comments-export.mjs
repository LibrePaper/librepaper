import test from "node:test";
import assert from "node:assert/strict";
import { commentsMarkdown, loadAllComments } from "../../src/lib/reader/comments-export.js";

const exportedAt = new Date(2026, 9, 24, 14, 5);
const base = { title: "Paper", url: "https://example.org/docs/paper", exportedAt };
const note = (id, extra = {}) => ({
  id, creator: "Ada", created: "2026-10-01T10:00:00Z", body: `body ${id}`, resolved: false,
  replies: [], quote: "", whole: false, orphaned: false, position: null, ...extra,
});

test("header counts the comments and names the source", () => {
  const text = commentsMarkdown({ ...base, comments: [note("a", { quote: "x" })] });
  assert.ok(text.startsWith("# Comments on Paper\n\nExported 2026-10-24 14:05 from https://example.org/docs/paper. 1 comment.\n"));
});

test("whole-document comments come first, with no quote", () => {
  const text = commentsMarkdown({ ...base, comments: [
    note("p", { quote: "passage", position: 3 }),
    note("w", { whole: true, quote: "ignored" }),
  ] });
  assert.ok(text.indexOf("## About the whole document") < text.indexOf("## Comments on passages"));
  assert.ok(!text.includes("ignored"));
  assert.match(text, /### Ada, 2026-10-01\n\nbody w/);
  assert.match(text, /### 1\. Ada, 2026-10-01\n\n> passage\n\nbody p/);
});

test("empty sections are omitted", () => {
  const onlyPassage = commentsMarkdown({ ...base, comments: [note("p", { quote: "q" })] });
  assert.ok(!onlyPassage.includes("About the whole document"));
  const onlyWhole = commentsMarkdown({ ...base, comments: [note("w", { whole: true })] });
  assert.ok(!onlyWhole.includes("Comments on passages"));
});

test("resolved and orphaned markers", () => {
  const text = commentsMarkdown({ ...base, comments: [note("p", { quote: "q", resolved: true, orphaned: true })] });
  assert.match(text, /### 1\. Ada, 2026-10-01 \(Resolved\) \(Passage no longer in the document\)/);
  const plain = commentsMarkdown({ ...base, comments: [note("p", { quote: "q" })] });
  assert.ok(!plain.includes("(Resolved)") && !plain.includes("no longer"));
});

test("replies nest in a blockquote and keep every line", () => {
  const text = commentsMarkdown({ ...base, comments: [note("p", {
    quote: "q",
    body: "line one\n\nline two",
    replies: [{ id: "r", creator: "", created: "2026-10-02T09:00:00Z", body: "first\nsecond\n\nthird" }],
  })] });
  assert.ok(text.includes("line one\n\nline two"));
  assert.ok(text.includes("> **Anonymous**, 2026-10-02: first\n> second\n>\n> third"));
});

test("quotes are literal: line starts are escaped", () => {
  const text = commentsMarkdown({ ...base, comments: [note("p", { quote: "# not a heading\n> not a quote\n- not a list\n1. not a list\nplain" })] });
  assert.ok(text.includes("> \\# not a heading\n> \\> not a quote\n> \\- not a list\n> \\1. not a list\n> plain"));
});

test("highlights without words or replies are left out", () => {
  const text = commentsMarkdown({ ...base, comments: [
    note("h", { body: "", quote: "marked" }),
    note("c", { quote: "kept" }),
    note("hr", { body: "", quote: "marked with reply", replies: [{ id: "r", creator: "Bo", created: "2026-10-02T00:00:00Z", body: "hm" }] }),
  ] });
  assert.ok(!text.includes("> marked\n"));
  assert.ok(text.includes("> kept") && text.includes("marked with reply"));
  assert.match(text, /2 comments\./);
});

test("passages follow document order, then creation time", () => {
  const text = commentsMarkdown({ ...base, comments: [
    note("late", { quote: "q", created: "2026-10-05T00:00:00Z" }),
    note("second", { quote: "q", position: 50 }),
    note("early", { quote: "q", created: "2026-10-02T00:00:00Z" }),
    note("first", { quote: "q", position: 5 }),
  ] });
  const order = ["body first", "body second", "body early", "body late"].map((s) => text.indexOf(s));
  assert.deepEqual([...order].sort((a, b) => a - b), order);
});

test("loadAllComments walks both cursors to the end", async () => {
  const asked = [];
  const threads = await loadAllComments({
    fetchPage: async (cursor) => {
      asked.push(`page:${cursor}`);
      return cursor === null
        ? { comments: [{ id: "1", replies: [{ id: "r1" }], reply_cursor: "rc1" }], next_cursor: "p2", complete: false }
        : { comments: [{ id: "2", replies: [] }, { id: "1", replies: [] }], next_cursor: null, complete: true };
    },
    fetchReplies: async (comment, cursor) => {
      asked.push(`replies:${comment.id}:${cursor}`);
      return cursor === "rc1"
        ? { replies: [{ id: "r2" }], next_cursor: "rc2", complete: false }
        : { replies: [{ id: "r3" }, { id: "r2" }], next_cursor: null, complete: true };
    },
  });
  assert.deepEqual(threads.map((t) => t.id), ["1", "2"]);
  assert.deepEqual(threads[0].replies.map((r) => r.id), ["r1", "r2", "r3"]);
  assert.deepEqual(asked, ["page:null", "page:p2", "replies:1:rc1", "replies:1:rc2"]);
});
