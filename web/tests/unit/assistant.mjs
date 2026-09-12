import assert from "node:assert/strict";
import {
  captureAttachment, capabilityAllows, composeTaskMessage, diagnosticContext,
  groupPass, normalizeCapabilities, resultIds, visibleResults, checkContextSize,
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

const caps = normalizeCapabilities({ can_read: true, can_comment: true, can_edit: false });
assert.equal(capabilityAllows(caps, task, { attached: attachment, path: "main.md" }), true);
assert.equal(capabilityAllows(caps, { kind: "rewrite", scope: "selection" }, { attached: { ...attachment, anchored: false }, path: "main.md" }), false);
assert.equal(capabilityAllows(caps, { kind: "explain", scope: "file" }, { path: "main.md" }), true);
assert.equal(capabilityAllows({}, { kind: "explain", scope: "file" }, { path: "main.md" }), false);

const diagnostic = diagnosticContext({ severity: "error", path: "main.md", line: 9, column: 3, message: "Unknown command", excerpt: "#bad" }, "sha-9");
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

const refinement = composeTaskMessage({ id: "m4", text: "Make the suggestion more concise.",
  task: { kind: "refine", scope: "selection" }, attachment,
  suggestion: { id: "s1", proposed: "A shorter proposal", revision: "sha-1" } });
assert.deepEqual(refinement.context.suggestion, { id: "s1", proposed: "A shorter proposal", revision: "sha-1" });

const reply = { context: { results: { suggestions: ["s1", "s2"], pass: "p1" } } };
assert.deepEqual(resultIds(reply), { suggestions: ["s1", "s2"], pass: "p1" });
const comments = [
  { id: "s1", motivation: "editing", pass: "p1", resolved: false },
  { id: "s2", motivation: "editing", pass: "p1", resolved: true },
  { id: "other", motivation: "commenting", pass: "p1", resolved: false },
];
assert.deepEqual(visibleResults(reply, comments), { suggestions: ["s1", "s2"], pass: "p1" });
assert.deepEqual(groupPass(comments, "p1"), { pass: "p1", total: 2, pending: 1, suggestions: ["s1", "s2"] });

console.log("assistant: anchored tasks, capability gating, diagnostic context and review result mapping passed");
