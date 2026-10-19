import assert from "node:assert/strict";
import {
  captureAttachment, capabilityAllows, composeTaskMessage, taskDiagnosticContext, diagnosticLabel, sourcePassage,
  normalizeCapabilities, resultIds, visibleResults, checkContextSize,
  TASK_CATALOG, TASK_CATEGORIES, TASK_KINDS, TASK_SCOPES, findTask, inferScope, searchTasks, taskPrompt, taskScopes,
  taskSections, CONTEXT_TASKS, taskForKind, taskBlocker,
  taskHint, recordTaskUse, frequentTasks, FREQUENT_LIMIT,
} from "../../src/lib/assistant.js";

const selection = {
  source: { path: "main.md", exact: "A repeated paragraph", prefix: "Before ", suffix: " after", position: 42 },
  exact: "A repeated paragraph", revision: "sha-1",
};
const attachment = captureAttachment(selection, "other.md", "sha-0");
assert.deepEqual(attachment, {
  path: "main.md", revision: "sha-1", anchored: true,
  selection: { path: "main.md", exact: "A repeated paragraph", prefix: "Before ", suffix: " after", position: 42 },
});

const task = { kind: "tighten", scope: "selection" };
const message = composeTaskMessage({ id: "m1", text: "Tighten it", task, attachment, path: "new.md", revision: "sha-2" });
assert.deepEqual(message.task, task);
assert.deepEqual(message.context.selection, attachment.selection);
assert.equal(message.context.file, "main.md");
assert.equal(message.context.revision, "sha-1");

const caps = normalizeCapabilities({ can_read: true, can_comment: true, can_suggest: true, can_edit: false });
assert.equal(capabilityAllows(caps, task, { attached: attachment, path: "main.md" }), true);
const rendered = captureAttachment({ exact: "Rendered words", prefix: "before ", suffix: " after", render_digest: "render-1" }, "open-file.md", "unrelated-source-revision");
assert.equal(rendered.anchored, false);
assert.equal(rendered.render_digest, "render-1");
assert.equal(capabilityAllows(caps, task, { attached: rendered }), true);
assert.equal(composeTaskMessage({ task, attachment: rendered }).context.render_digest, "render-1");
assert.equal(composeTaskMessage({ task, attachment: rendered, revision: "unrelated-source-revision" }).context.revision, undefined);

// A source passage is read from the open source text, and it needs no revision to be anchored.
const plain = sourcePassage("Alpha beta gamma", 6, 10, "main.md");
assert.deepEqual(plain, { path: "main.md", exact: "beta", prefix: "Alpha ", suffix: " gamma", position: 6 });
const wide = sourcePassage("x".repeat(40) + "word" + "y".repeat(40), 40, 44, "main.md");
assert.equal(wide.exact, "word");
assert.equal(wide.prefix.length, 32);
assert.equal(wide.suffix.length, 32);
assert.equal(wide.position, 40);
assert.equal(sourcePassage("abc", 1, 1, "main.md"), null);
assert.equal(sourcePassage("a   b", 1, 4, "main.md"), null);
assert.equal("path" in sourcePassage("Alpha beta", 6, 10), false);
// The 32 unit context edges fall inside emoji pairs, so each edge is trimmed to a whole character.
const emoji = "\u{1F600}";
const paired = "a".repeat(10) + emoji + "b".repeat(31) + "SEL" + "c".repeat(31) + emoji + "d".repeat(10);
const edged = sourcePassage(paired, 43, 46, "main.md");
assert.equal(edged.exact, "SEL");
assert.equal(edged.prefix.length, 31);
assert.equal(edged.suffix.length, 31);
assert.equal(edged.prefix.charCodeAt(0) >= 0xdc00 && edged.prefix.charCodeAt(0) <= 0xdfff, false);
assert.equal(edged.suffix.charCodeAt(edged.suffix.length - 1) >= 0xd800 && edged.suffix.charCodeAt(edged.suffix.length - 1) <= 0xdbff, false);
// A source selection is anchored without a revision, and its task carries none.
const sourced = captureAttachment({ source: plain, exact: "beta" }, "main.md", "");
assert.equal(sourced.anchored, true);
assert.equal(composeTaskMessage({ task, attachment: sourced }).context.revision, undefined);
assert.equal(capabilityAllows(caps, task, { attached: { selection: { exact: "" } } }), false);
assert.equal(capabilityAllows(caps, { kind: "rewrite", scope: "selection" }, { attached: { ...attachment, anchored: false }, path: "main.md" }), true);
assert.equal(capabilityAllows(caps, { kind: "explain", scope: "file" }, { path: "main.md" }), true);
assert.equal(capabilityAllows({}, { kind: "explain", scope: "file" }, { path: "main.md" }), false);

// A commenter must never be treated as able to suggest just because they can
// comment: a suggestion is an editor's track change, gated on the server's
// own `can_suggest` flag alone.
const commenter = normalizeCapabilities({ can_read: true, can_comment: true, can_edit: false });
assert.equal(commenter.can_suggest, false);
assert.equal(commenter.can_reply, false);
assert.equal(capabilityAllows(commenter, task, { attached: attachment, path: "main.md" }), false);

const diagnostic = taskDiagnosticContext({ severity: "error", path: "main.md", line: 9, column: 3, message: "Unknown command", excerpt: "#bad" }, "sha-9");
assert.equal(diagnostic.file, "main.md");
assert.equal(diagnostic.revision, "sha-9");
assert.equal(diagnostic.source, "#bad");
const explain = composeTaskMessage({ id: "m2", text: "Explain this error", task: { kind: "explain", scope: "file" }, diagnostic });
assert.equal(explain.context.diagnostic.message, "Unknown command");

const warnings = [{ severity: "warning", file: "librepaper.tex", line: 12,
  message: "Reference undefined", source: "\\ref{missing}", revision: "render-sha",
  provenance: { engine: "latex" } }];
const fixWarnings = composeTaskMessage({ text: "Fix the warnings", revision: "selection-sha", diagnostics: warnings });
assert.deepEqual(fixWarnings.context.diagnostics, warnings);
assert.equal(fixWarnings.context.diagnostics_omitted, 0);
assert.equal(composeTaskMessage({ text: "Hello" }).context.diagnostics.length, 0);
const crowded = composeTaskMessage({ text: "Fix warnings", task, attachment,
  diagnostics: [{ message: "x".repeat(20000) }, ...Array(500).fill(warnings[0])] });
assert.ok(checkContextSize({ task: crowded.task, context: crowded.context }).ok);
assert.deepEqual(crowded.context.selection, attachment.selection);
assert.ok(crowded.context.diagnostics.length > 0);
assert.equal(crowded.context.diagnostics.length + crowded.context.diagnostics_omitted, 501);

const thread = { id: "comment-1", body: "Please clarify this", replies: [{ body: "Could you expand?" }] };
const response = composeTaskMessage({ id: "m3", text: "I will clarify this.",
  task: { kind: "respond", scope: "selection" }, attachment, thread });
assert.deepEqual(response.context.thread, thread);
assert.equal(response.context.comment_id, "comment-1");
assert.equal(response.context.revision, undefined);

const reply = { context: { results: { changes: ["c1", "c2"] } } };
assert.deepEqual(resultIds(reply), { changes: ["c1", "c2"] });
assert.deepEqual(resultIds({ context: {} }), { changes: [] });
// Only the changes still pending in the document can be reviewed.
assert.deepEqual(visibleResults(reply, [{ id: "c1" }, { id: "other" }]), { changes: ["c1"] });
assert.deepEqual(visibleResults(reply, []), { changes: [] });

// Every launcher entry must stay inside the protocol's task vocabulary, and
// every entry must be reachable by its own id.
for (const entry of TASK_CATALOG) {
  assert.ok(TASK_KINDS.includes(entry.kind), `${entry.id} uses an unknown task kind`);
  assert.equal(findTask(entry.id), entry);
  assert.ok(taskScopes(entry).length, `${entry.id} has no usable scope`);
  assert.ok(taskPrompt(entry, "document").includes("the whole document"));
}
assert.equal(new Set(TASK_CATALOG.map((entry) => entry.id)).size, TASK_CATALOG.length);
assert.equal(findTask("nope"), null);

assert.deepEqual(taskScopes(findTask("tighten")), ["selection"]);
assert.deepEqual(taskScopes(findTask("outline")), ["file", "document"]);
assert.deepEqual(taskScopes(findTask("proofread")), ["selection", "file", "document"]);
assert.equal(taskPrompt(findTask("proofread"), "selection"), "Proofread the selected passage.");
assert.equal(taskPrompt(findTask("summarize"), "file"), "Summarize this file.");

// Scope follows what the reader already has: a passage, then a file.
assert.equal(inferScope(findTask("proofread"), { attached: attachment, path: "main.md" }), "selection");
assert.equal(inferScope(findTask("proofread"), { path: "main.md" }), "file");
assert.equal(inferScope(findTask("proofread"), {}), "document");
assert.equal(inferScope(findTask("outline"), { attached: attachment, path: "main.md" }), "file");
assert.equal(inferScope(findTask("tighten"), { path: "main.md" }), "selection");

// Search narrows like a command palette: a label prefix wins over a
// description hit, and every typed word has to match.
assert.deepEqual(searchTasks("proof").map((entry) => entry.id), ["proofread"]);
assert.deepEqual(searchTasks("").map((entry) => entry.id), TASK_CATALOG.map((entry) => entry.id));
assert.deepEqual(searchTasks("check cit").map((entry) => entry.id), ["check-citations"]);
assert.deepEqual(searchTasks("zzz"), []);

// Keywords carry the synonyms a user actually types.
assert.equal(searchTasks("grammar")[0].id, "proofread");
assert.equal(searchTasks("shorter")[0].id, "tighten");
assert.equal(searchTasks("citations")[0].id, "check-citations");
assert.equal(searchTasks("argument")[0].id, "review-argument");
assert.equal(searchTasks("summary")[0].id, "summarize");
// A typo still finds the entry, and a label prefix outranks a description hit.
assert.equal(searchTasks("proofraed")[0].id, "proofread");
assert.equal(searchTasks("review")[0].id, "review-argument");
assert.equal(searchTasks("contradictions")[0].id, "check-consistency");
assert.equal(searchTasks("spelling")[0].id, "proofread");
assert.equal(searchTasks("concise")[0].id, "tighten");
for (const entry of TASK_CATALOG) {
  assert.ok(Array.isArray(entry.keywords) && entry.keywords.length >= 3, `${entry.id} has too few keywords`);
}

// The Tasks browser shows every catalog entry, one section per category. An
// entry in several categories appears in each of their sections.
const sectionIds = (category) => taskSections().find((section) => section.category === category).entries.map((entry) => entry.id);
assert.deepEqual(taskSections().map((section) => section.category), TASK_CATEGORIES);
assert.ok(sectionIds("Writing").includes("explain"));
assert.ok(sectionIds("Review").includes("explain"));
assert.ok(sectionIds("Document").includes("summarize"));
assert.ok(sectionIds("Review").includes("summarize"));
assert.deepEqual(sectionIds("Document"), ["outline", "summarize"]);
const browsed = taskSections().flatMap((section) => section.entries.map((entry) => entry.id));
for (const entry of TASK_CATALOG) {
  assert.ok(browsed.includes(entry.id), `${entry.id} is not browsed`);
}
for (const section of taskSections()) {
  assert.ok(section.entries.length > 0, `${section.category} has no entries`);
  assert.ok(section.entries.every((entry) => entry.categories.includes(section.category)), `${section.category} holds a foreign entry`);
}
for (const entry of CONTEXT_TASKS) {
  assert.equal(browsed.includes(entry.id), false, `${entry.id} must not be browsed`);
}

// Context tasks sit outside the catalog and use the protocol's kinds.
for (const entry of CONTEXT_TASKS) {
  assert.ok(TASK_KINDS.includes(entry.kind), `${entry.id} uses an unknown task kind`);
  assert.equal(TASK_CATALOG.includes(entry), false);
  assert.equal(findTask(entry.id), entry);
}
assert.equal(findTask("fix-diagnostic").kind, "fix");
assert.equal(taskForKind("tighten").id, "tighten");
assert.equal(taskForKind("respond").id, "address-comment");
assert.equal(taskForKind("nope"), null);

// The blocker and the boolean gate agree across the whole matrix.
const commentCaps = normalizeCapabilities({ can_read: true, can_comment: true, can_edit: false });
const editorCaps = normalizeCapabilities({ can_read: true, can_comment: true, can_suggest: true, can_edit: false });
const contexts = [{}, { path: "main.md" }, { attached: attachment, path: "main.md" },
  { attached: { selection: { exact: "" } }, path: "" }, { attached: { ...attachment, anchored: false } }];
const kinds = [...TASK_KINDS, "bogus"];
const scopes = [...TASK_SCOPES, "bogus"];
let allowedCount = 0;
let blockedCount = 0;
for (const capsValue of [commentCaps, editorCaps]) {
  for (const kind of kinds) {
    for (const scope of scopes) {
      for (const context of contexts) {
        const candidate = { kind, scope };
        const allowed = capabilityAllows(capsValue, candidate, context);
        assert.equal(taskBlocker(capsValue, candidate, context) === "", allowed, `${kind}/${scope} disagrees`);
        if (allowed) allowedCount += 1;
        else blockedCount += 1;
      }
    }
  }
}
assert.ok(allowedCount > 0 && blockedCount > 0);
assert.equal(taskBlocker(commentCaps, null), "This task cannot run here.");
assert.equal(taskBlocker({}, { kind: "explain", scope: "file" }, { path: "main.md" }), "Connect an agent to check access for this task.");
assert.equal(taskBlocker({}, { kind: "proofread", scope: "file" }, { path: "main.md", deferAccess: true }), "");
// Held capabilities from a Comment link do not block a start that will grant Edit.
assert.equal(taskBlocker(commentCaps, { kind: "proofread", scope: "file" }, { path: "main.md", deferAccess: true }), "");
assert.equal(taskBlocker({}, { kind: "explain", scope: "file" }, { deferAccess: true }), "Open a file to use this task.");
assert.equal(taskBlocker(commentCaps, { kind: "tighten", scope: "selection" }, { attached: attachment }), "You need edit access to run this task.");
assert.equal(taskBlocker(commentCaps, { kind: "explain", scope: "file" }, {}), "Open a file to use this task.");
assert.equal(taskBlocker(editorCaps, { kind: "respond", scope: "selection" }, { attached: attachment }), "You need comment access to run this task.");

// The launcher names a diagnostic by severity and place. The engine's own
// message is never part of the label: it is what the Diagnostics panel shows.
assert.equal(diagnosticLabel({ severity: "error", file: "main.tex", line: 12, message: "html export is under active development and incomplete" }), "Error · main.tex:12");
assert.equal(diagnosticLabel({ severity: "warning", path: "chapter.typ" }), "Warning · chapter.typ");
assert.equal(diagnosticLabel({ message: "no place given" }, "main.md"), "Error · main.md");
assert.equal(diagnosticLabel({ severity: "warning" }), "Warning");

// taskHint names what the reader still needs, and never reports access.
assert.equal(taskHint(findTask("tighten"), {}).short, "Needs a source selection");
assert.equal(taskHint(findTask("tighten"), { attached: attachment, path: "main.md" }), null);
assert.equal(taskHint(findTask("outline"), {}), null);
assert.equal(taskHint(findTask("proofread"), {}), null);
assert.equal(taskHint(null), null);

// Usage counts uses by stable id, and a recorded use never mutates its input.
const once = recordTaskUse({}, "proofread", 10);
assert.deepEqual(once, { proofread: { count: 1, last: 10 } });
assert.deepEqual(recordTaskUse(once, "proofread", 20), { proofread: { count: 2, last: 20 } });
assert.deepEqual(once, { proofread: { count: 1, last: 10 } });
assert.deepEqual(recordTaskUse({}, "not-a-task", 10), {});
assert.deepEqual(recordTaskUse({ "not-a-task": { count: 1, last: 1 } }, "proofread", 10), { proofread: { count: 1, last: 10 } });
assert.deepEqual(recordTaskUse({ proofread: { count: "x", last: 1 }, tighten: null }, "outline", 5), { outline: { count: 1, last: 5 } });

assert.deepEqual(frequentTasks(recordTaskUse({}, "proofread", 1)), []);
const twice = recordTaskUse(recordTaskUse({}, "proofread", 1), "proofread", 2);
assert.deepEqual(frequentTasks(twice).map((entry) => entry.id), ["proofread"]);
const ranking = {
  tighten: { count: 3, last: 5 }, proofread: { count: 3, last: 9 },
  outline: { count: 2, last: 100 }, summarize: { count: 2, last: 50 },
};
assert.deepEqual(frequentTasks(ranking).map((entry) => entry.id), ["proofread", "tighten", "outline", "summarize"]);
const many = Object.fromEntries(["proofread", "tighten", "rewrite", "explain", "outline", "summarize"]
  .map((id, index) => [id, { count: 2, last: index }]));
assert.equal(FREQUENT_LIMIT, 5);
assert.equal(frequentTasks(many).length, FREQUENT_LIMIT);
assert.deepEqual(frequentTasks({ nope: { count: 9, last: 9 }, proofread: { count: "3", last: 1 } }), []);
assert.deepEqual(frequentTasks(null), []);

console.log("assistant: anchored tasks, capability gating, diagnostic context and review result mapping, and the task catalog passed");
