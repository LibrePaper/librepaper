// Pure assistant-panel helpers. Keeping task and context composition here
// makes it possible to verify the wire contract without mounting Svelte.

export const TASK_KINDS = ["proofread", "tighten", "rewrite", "explain", "fix", "refine", "outline", "respond"];
export const TASK_SCOPES = ["selection", "file", "document"];
export const CONTEXT_LIMIT = 16 * 1024;

const text = (value) => (value == null ? "" : String(value));

export function normalizeAnchor(selection, fallbackPath = "") {
  const source = selection?.source && typeof selection.source === "object"
    ? selection.source
    : selection;
  if (!source || !text(source.exact).trim()) return null;
  const path = text(source.path || fallbackPath);
  return {
    ...(path ? { path } : {}),
    exact: text(source.exact),
    prefix: text(source.prefix),
    suffix: text(source.suffix),
    position: Number.isInteger(source.position) && source.position >= 0 ? source.position : null,
  };
}

export function captureAttachment(selection, path = "", revision = "") {
  const hasSource = Boolean(selection && Object.prototype.hasOwnProperty.call(selection, "source"));
  const source = hasSource && selection.source ? selection.source : selection;
  const anchor = normalizeAnchor(source, path);
  if (!anchor) return null;
  const renderDigest = text(selection?.render_digest || source?.render_digest);
  const capturedRevision = text(selection?.revision || source?.revision || (renderDigest ? "" : revision));
  return {
    path: anchor.path || text(path),
    selection: anchor,
    revision: capturedRevision,
    ...(renderDigest ? { render_digest: renderDigest } : {}),
    // The server resolves rendered quotations against its immutable view. A
    // browser selection therefore does not need to invent a source handle or
    // revision before an editing task can be offered. `anchored` remains a
    // display hint for source backed attachments, rather than an authority
    // claim about the selection.
    anchored: Boolean((hasSource ? selection.source : source?.path) && anchor.path && capturedRevision),
  };
}

// The wire vocabulary is fixed (see docs/protocol/chat.md), but a launcher
// entry is a task the user recognizes: a kind plus the request it phrases.
// Several entries may share a kind, which is what lets the catalog grow
// without touching the protocol.
export const TASK_CATALOG = [
  { id: "proofread", kind: "proofread", category: "Writing", label: "Proofread",
    description: "Correct spelling, grammar, and wording while preserving meaning.",
    prompt: "Proofread {target}." },
  { id: "tighten", kind: "tighten", category: "Writing", label: "Tighten",
    description: "Cut redundancy and shorten without losing substance.",
    scopes: ["selection"], prompt: "Tighten {target}." },
  { id: "rewrite", kind: "rewrite", category: "Writing", label: "Rewrite",
    description: "Rework the wording for clarity and flow.",
    scopes: ["selection"], prompt: "Rewrite {target}." },
  { id: "explain", kind: "explain", category: "Writing", label: "Explain",
    description: "Say what this passage claims and how it is put together.",
    prompt: "Explain {target}." },

  { id: "review-argument", kind: "explain", category: "Review", label: "Review the argument",
    description: "Name unsupported claims, gaps in reasoning, and weak transitions.",
    prompt: "Review the argument in {target}. Name unsupported claims, gaps in reasoning, and weak transitions." },
  { id: "check-consistency", kind: "explain", category: "Review", label: "Check consistency",
    description: "Look for terminology, notation, and definitions that disagree with each other.",
    prompt: "Check {target} for inconsistent terminology, notation, and definitions." },
  { id: "check-citations", kind: "explain", category: "Review", label: "Check citations",
    description: "Check that each citation resolves in the bibliography and supports its claim.",
    prompt: "Check the citations in {target}: each one should resolve in the bibliography and support the claim it is attached to." },
  { id: "suggest-improvements", kind: "proofread", category: "Review", label: "Suggest improvements",
    description: "Propose concrete anchored edits rather than commentary.",
    prompt: "Suggest concrete improvements to {target} as anchored edits." },

  { id: "outline", kind: "outline", category: "Document", label: "Outline",
    description: "List the headings and what each section does.",
    scopes: ["file", "document"], prompt: "Outline {target}: list the headings and say what each section does." },
  { id: "summarize", kind: "explain", category: "Document", label: "Summarize",
    description: "Summarize the argument in a few paragraphs.",
    scopes: ["file", "document"], prompt: "Summarize {target}." },
];

export const TASK_TARGETS = { selection: "the selected passage", file: "this file", document: "the whole document" };

export function findTask(id) {
  return TASK_CATALOG.find((entry) => entry.id === id) || null;
}

export function taskScopes(entry) {
  const allowed = entry?.scopes || TASK_SCOPES;
  return TASK_SCOPES.filter((scope) => allowed.includes(scope));
}

// Scope is inferred from what the reader already has in hand rather than
// asked for before the user has said what they want.
export function inferScope(entry, { attached = null, path = "" } = {}) {
  const allowed = taskScopes(entry);
  if (attached && allowed.includes("selection")) return "selection";
  if (path && allowed.includes("file")) return "file";
  return allowed.includes("document") ? "document" : allowed[0] || "document";
}

export function taskPrompt(entry, scope) {
  if (!entry) return "";
  return String(entry.prompt || `${entry.label} {target}.`)
    .replaceAll("{target}", TASK_TARGETS[scope] || TASK_TARGETS.document);
}

// A command-palette match: every word the user typed has to appear somewhere
// in the entry, and a hit on the label outranks a hit on the description.
export function searchTasks(query, catalog = TASK_CATALOG) {
  const words = String(query || "").toLowerCase().split(/\s+/).filter(Boolean);
  if (!words.length) return [...catalog];
  const scored = [];
  for (const entry of catalog) {
    const label = entry.label.toLowerCase();
    const haystack = `${label} ${entry.category} ${entry.description || ""} ${entry.kind}`.toLowerCase();
    if (!words.every((word) => haystack.includes(word))) continue;
    const rank = label.startsWith(words[0]) ? 0 : words.every((word) => label.includes(word)) ? 1 : 2;
    scored.push({ entry, rank });
  }
  return scored.sort((a, b) => a.rank - b.rank).map((item) => item.entry);
}

export function groupTasks(entries) {
  const groups = [];
  for (const entry of entries) {
    const group = groups.find((item) => item.category === entry.category);
    if (group) group.tasks.push(entry);
    else groups.push({ category: entry.category, tasks: [entry] });
  }
  return groups;
}

export function scopeLabel(scope, { path = "", attached = null } = {}) {
  if (scope === "selection") return attached?.path ? `Selected passage · ${attached.path}` : "Selected passage";
  if (scope === "file") return path ? `Current file · ${path}` : "Current file";
  return "Whole document";
}

// How a diagnostic is named in the launcher. The compiler's own message is
// the Diagnostics panel's business: here it would be an unbounded paragraph
// of engine prose sitting in a list of one-line tasks, so the launcher says
// only which diagnostic this is -- its severity and where it is.
export function diagnosticLabel(item, fallbackPath = "") {
  const severity = item?.severity === "warning" ? "Warning" : "Error";
  const path = text(item?.file || item?.path || fallbackPath);
  const line = Number(item?.line) > 0 ? `:${item.line}` : "";
  return path ? `${severity} · ${path}${line}` : severity;
}

// The server's capability flags are taken exactly as given. A commenter must
// never be upgraded into a suggester by inferring one right from another: a
// suggestion is an editor's track change, and only the server's own
// `can_suggest` flag says whether this reader may make one.
export function normalizeCapabilities(value) {
  const raw = value?.capabilities && typeof value.capabilities === "object"
    ? value.capabilities
    : (value && typeof value === "object" ? value : {});
  const known = ["can_read", "can_comment", "can_edit", "can_suggest", "can_reply"];
  const hasAny = known.some((key) => typeof raw[key] === "boolean");
  return {
    verified: value?.verified !== false && (value?.verified === true || hasAny),
    can_read: raw.can_read === true,
    can_comment: raw.can_comment === true,
    can_edit: raw.can_edit === true,
    can_suggest: raw.can_suggest === true,
    can_reply: raw.can_reply === true,
    raw,
  };
}

export function taskNeedsAnchor(task) {
  return task?.scope === "selection" && ["tighten", "rewrite", "refine"].includes(task?.kind);
}

export function capabilityAllows(capabilities, task, { attached = null, path = "" } = {}) {
  const caps = normalizeCapabilities(capabilities);
  if (!task || !TASK_KINDS.includes(task.kind) || !TASK_SCOPES.includes(task.scope)) return false;
  if (!caps.verified || !caps.can_read) return false;
  if (["tighten", "rewrite", "refine"].includes(task.kind) && task.scope !== "selection") return false;
  if (task.scope === "selection" && !attached) return false;
  if (taskNeedsAnchor(task) && !text(attached?.selection?.exact).trim()) return false;
  if (task.scope === "file" && !path) return false;
  if (["tighten", "rewrite", "proofread", "fix", "refine"].includes(task.kind) && !caps.can_suggest) return false;
  if (task.kind === "respond" && !caps.can_reply) return false;
  return true;
}

// The compact diagnostic entry a task's context carries, distinct from
// assistant-review.js's `diagnosticContext`, which reconstructs a source
// line from a rendered tree rather than compacting a wire diagnostic.
export function taskDiagnosticContext(diagnostic, revision = "") {
  if (!diagnostic) return null;
  const source = diagnostic.source || diagnostic.excerpt || diagnostic.line_text;
  return {
    ...(diagnostic.severity ? { severity: text(diagnostic.severity) } : {}),
    ...(diagnostic.file || diagnostic.path ? { file: text(diagnostic.file || diagnostic.path) } : {}),
    ...(Number.isFinite(diagnostic.line) ? { line: diagnostic.line } : {}),
    ...(Number.isFinite(diagnostic.column) ? { column: diagnostic.column } : {}),
    ...(diagnostic.message ? { message: text(diagnostic.message) } : {}),
    ...(source ? { source: text(source) } : {}),
    ...(diagnostic.provenance ? { provenance: diagnostic.provenance } : {}),
    ...(revision ? { revision: text(revision) } : {}),
  };
}

// A refinement keeps the original annotation as its identity. The runner
// uses this compact context to call the refine document operation rather than
// creating a second suggestion about the same words.
export function suggestionContext(suggestion) {
  if (!suggestion?.id) return null;
  return {
    id: text(suggestion.id),
    ...(suggestion.proposed !== undefined ? { proposed: text(suggestion.proposed) } : {}),
    ...(suggestion.revision ? { revision: text(suggestion.revision) } : {}),
    ...(suggestion.path ? { path: text(suggestion.path) } : {}),
    ...(suggestion.exact ? { exact: text(suggestion.exact) } : {}),
  };
}

export function composeTaskMessage({ id, text: body = "", task, attachment = null, selection = null, path = "", revision = "", diagnostic = null, diagnostics = [], suggestion = null, thread = null } = {}) {
  const context = {};
  const selected = task?.scope === "selection" || !task
    ? (attachment || (selection ? captureAttachment(selection, path, revision) : null))
    : null;
  const file = selected?.path || (task?.scope === "file" || !task ? path : "");
  if (file) context.file = file;
  if (selected?.selection) context.selection = selected.selection;
  const commentId = thread?.id || suggestion?.id;
  const currentRevision = selected?.revision || (selected?.render_digest ? "" : revision);
  if (commentId) context.comment_id = text(commentId);
  else {
    if (currentRevision) context.revision = currentRevision;
    if (selected?.render_digest) context.render_digest = selected.render_digest;
  }
  const error = taskDiagnosticContext(diagnostic, revision || diagnostic?.revision || "");
  if (error) context.diagnostic = error;
  const refinement = suggestionContext(suggestion);
  if (refinement) context.suggestion = refinement;
  if (thread && typeof thread === "object") context.thread = thread;
  // Browser diagnostics include LaTeX warnings that the native CLI cannot
  // reproduce. Keep each render's revision, independently of the selection.
  // Reserve space for an explicit omission count without crowding out the
  // user's selected passage, diagnostic, or comment thread.
  context.diagnostics = [];
  context.diagnostics_omitted = diagnostics.length;
  for (const item of diagnostics) {
    const entry = taskDiagnosticContext(item, item?.revision || "");
    if (!entry) continue;
    context.diagnostics.push(entry);
    if (!checkContextSize({ task, context }).ok) context.diagnostics.pop();
    else context.diagnostics_omitted -= 1;
  }
  return {
    type: "message",
    id: text(id),
    text: text(body).trim(),
    ...(task ? { task: { kind: task.kind, scope: task.scope } } : {}),
    context,
  };
}

export function contextSize(value) {
  return new TextEncoder().encode(JSON.stringify(value ?? {})).byteLength;
}

export function checkContextSize(value, limit = CONTEXT_LIMIT) {
  const bytes = contextSize(value);
  return { bytes, limit, ok: bytes <= limit };
}

export function resultIds(message) {
  const results = message?.context?.results;
  if (!results || typeof results !== "object") return { suggestions: [] };
  const suggestions = Array.isArray(results.suggestions)
    ? results.suggestions.map(text).filter(Boolean)
    : [];
  return { suggestions };
}

export function visibleResults(message, comments = []) {
  const result = resultIds(message);
  const editing = comments.filter((comment) => comment?.motivation === "editing");
  const visible = new Set(editing.map((comment) => text(comment?.id)));
  const suggestions = result.suggestions.filter((id) => visible.has(id));
  return { suggestions };
}
