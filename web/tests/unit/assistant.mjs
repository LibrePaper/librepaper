import assert from "node:assert/strict";
import {
  captureAttachment, capabilityAllows, composeTaskMessage, taskDiagnosticContext, diagnosticLabel,
  normalizeCapabilities, resultIds, visibleResults, checkContextSize,
  TASK_CATALOG, TASK_KINDS, findTask, groupTasks, inferScope, searchTasks, taskPrompt, taskScopes,
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

const refinement = composeTaskMessage({ id: "m4", text: "Make the suggestion more concise.",
  task: { kind: "refine", scope: "selection" }, attachment,
  suggestion: { id: "s1", proposed: "A shorter proposal", revision: "sha-1" } });
assert.deepEqual(refinement.context.suggestion, { id: "s1", proposed: "A shorter proposal", revision: "sha-1" });

const reply = { context: { results: { suggestions: ["s1", "s2"] } } };
assert.deepEqual(resultIds(reply), { suggestions: ["s1", "s2"] });
const comments = [
  { id: "s1", motivation: "editing", resolved: false },
  { id: "s2", motivation: "editing", resolved: true },
  { id: "other", motivation: "commenting", resolved: false },
];
assert.deepEqual(visibleResults(reply, comments), { suggestions: ["s1", "s2"] });

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
assert.deepEqual(searchTasks("proof").map((entry) => entry.id), ["proofread", "suggest-improvements"]);
assert.deepEqual(searchTasks("").map((entry) => entry.id), TASK_CATALOG.map((entry) => entry.id));
assert.deepEqual(searchTasks("check cit").map((entry) => entry.id), ["check-citations"]);
assert.deepEqual(searchTasks("zzz"), []);
assert.deepEqual(groupTasks(searchTasks("")).map((group) => group.category), ["Writing", "Review", "Document"]);
assert.deepEqual(groupTasks([]), []);

// The launcher names a diagnostic by severity and place. The engine's own
// message is never part of the label: it is what the Diagnostics panel shows.
assert.equal(diagnosticLabel({ severity: "error", file: "main.tex", line: 12, message: "html export is under active development and incomplete" }), "Error · main.tex:12");
assert.equal(diagnosticLabel({ severity: "warning", path: "chapter.typ" }), "Warning · chapter.typ");
assert.equal(diagnosticLabel({ message: "no place given" }, "main.md"), "Error · main.md");
assert.equal(diagnosticLabel({ severity: "warning" }), "Warning");

console.log("assistant: anchored tasks, capability gating, diagnostic context and review result mapping, and the task catalog passed");
