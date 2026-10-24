// Every comment thread of a project, written as one Markdown file.
//
// The server pages comments on purpose and offers no call that returns them
// all, so the export walks the same two cursors the sidebar does, to the end.
// The formatter below is pure: it is handed threads that are already loaded.

import { named } from "../thread.js";

/// Reads every page of comments and every page of each comment's replies.
/// `fetchPage(cursor)` answers one annotation page and
/// `fetchReplies(comment, cursor)` one reply page, each with `next_cursor` and
/// `complete`. A row met twice is kept once, since the traversal is not a
/// snapshot and a row can arrive on two pages.
export async function loadAllComments({ fetchPage, fetchReplies }) {
  const threads = [];
  const seen = new Set();
  let cursor = null;
  do {
    const page = await fetchPage(cursor);
    for (const comment of page.comments || []) {
      if (seen.has(String(comment.id))) continue;
      seen.add(String(comment.id));
      threads.push({ ...comment, replies: [...(comment.replies || [])] });
    }
    const next = page.complete ? null : page.next_cursor ?? null;
    if (next !== null && next === cursor) throw new Error("The comments could not be read to the end.");
    cursor = next;
  } while (cursor !== null);

  for (const thread of threads) {
    const held = new Set(thread.replies.map((reply) => String(reply.id)));
    let replyCursor = thread.reply_cursor ?? null;
    while (replyCursor !== null) {
      const page = await fetchReplies(thread, replyCursor);
      for (const reply of page.replies || []) {
        if (held.has(String(reply.id))) continue;
        held.add(String(reply.id));
        thread.replies.push(reply);
      }
      const next = page.complete ? null : page.next_cursor ?? null;
      if (next !== null && next === replyCursor) throw new Error("The replies could not be read to the end.");
      replyCursor = next;
    }
  }
  return threads;
}

const pad = (value) => String(value).padStart(2, "0");

function day(created) {
  const date = new Date(created);
  return Number.isNaN(date.getTime()) ? String(created ?? "").slice(0, 10) : date.toISOString().slice(0, 10);
}

function stamp(date) {
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())} ${pad(date.getHours())}:${pad(date.getMinutes())}`;
}

const lines = (text) => String(text ?? "").replace(/\r\n?/g, "\n").split("\n");

/// Prefixes every line, leaving no trailing space on a blank one.
const quoted = (rows) => rows.map((row) => (row === "" ? ">" : `> ${row}`)).join("\n");

/// A quoted passage is literal text, so a line that would read as a heading,
/// a quotation or a list item is made plain with a backslash.
function literal(row) {
  return row.replace(/^(\s*)(#|>|[-+*](?=\s)|\d+[.)](?=\s))/, "$1\\$2");
}

function reply(item) {
  const [first, ...rest] = lines(item.body);
  return quoted([`**${named(item.creator)}**, ${day(item.created)}: ${first}`, ...rest]);
}

function heading(thread, number) {
  const marks = [thread.resolved ? " (Resolved)" : "", thread.orphaned ? " (Passage no longer in the document)" : ""].join("");
  const who = `${named(thread.creator)}, ${day(thread.created)}`;
  return `### ${number ? `${number}. ` : ""}${who}${marks}`;
}

function section(thread, number) {
  const parts = [heading(thread, number)];
  if (!thread.whole && thread.quote) parts.push(quoted(lines(thread.quote).map(literal)));
  if (String(thread.body ?? "").trim()) parts.push(String(thread.body).replace(/\r\n?/g, "\n").trim());
  for (const item of thread.replies || []) parts.push(reply(item));
  return parts.join("\n\n");
}

const byCreated = (a, b) => String(a.created).localeCompare(String(b.created)) || String(a.id).localeCompare(String(b.id));

/// Whole-document comments first, then passages in document order where the
/// position is known and by creation time after them.
function inOrder(threads) {
  const placed = threads.filter((thread) => Number.isInteger(thread.position));
  const unplaced = threads.filter((thread) => !Number.isInteger(thread.position));
  placed.sort((a, b) => a.position - b.position || byCreated(a, b));
  unplaced.sort(byCreated);
  return [...placed, ...unplaced];
}

/// `comments` are loaded threads: `creator`, `created`, `body`, `resolved`,
/// `replies`, and what the reader knows of the passage: `quote`, `whole`,
/// `orphaned` and `position`. A highlight with no words and no replies is not
/// a comment and is left out.
export function commentsMarkdown({ title, url, exportedAt, comments }) {
  const kept = comments.filter((thread) => String(thread.body ?? "").trim() || (thread.replies || []).length);
  const whole = kept.filter((thread) => thread.whole).sort(byCreated);
  const passages = inOrder(kept.filter((thread) => !thread.whole));
  const count = kept.length;
  const out = [
    `# Comments on ${title}`,
    `Exported ${stamp(exportedAt)} from ${url}. ${count} ${count === 1 ? "comment" : "comments"}.`,
  ];
  if (whole.length) out.push("## About the whole document", ...whole.map((thread) => section(thread, 0)));
  if (passages.length) out.push("## Comments on passages", ...passages.map((thread, index) => section(thread, index + 1)));
  return `${out.join("\n\n")}\n`;
}
