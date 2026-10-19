import assert from "node:assert/strict";
import {
  captureAttachment, capabilityAllows, composeTaskMessage, taskDiagnosticContext, diagnosticLabel,
  normalizeCapabilities, resultIds, visibleResults, checkContextSize,
  TASK_CATALOG, TASK_CATEGORIES, TASK_KINDS, TASK_SCOPES, findTask, inferScope, searchTasks, taskPrompt, taskScopes,
  taskSections, CONTEXT_TASKS, taskForKind, taskBlocker,
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

// The Tasks browser shows every catalog entry, one section per category.
assert.deepEqual(taskSections().map((section) => section.category), TASK_CATEGORIES);
assert.deepEqual(taskSections().flatMap((section) => section.entries.map((entry) => entry.id)), TASK_CATALOG.map((entry) => entry.id));
assert.equal(taskSections("Document").length, 1);
assert.deepEqual(taskSections("Document")[0].entries.map((entry) => entry.id), ["outline", "summarize"]);
assert.deepEqual(taskSections("Nope"), []);
for (const section of taskSections()) {
  assert.ok(section.entries.length > 0, `${section.category} has no entries`);
  assert.ok(section.entries.every((entry) => entry.category === section.category), `${section.category} holds a foreign entry`);
}
const browsed = taskSections().flatMap((section) => section.entries.map((entry) => entry.id));
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

console.log("assistant: anchored tasks, capability gating, diagnostic context and review result mapping, and the task catalog passed");
