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
  const capturedRevision = text(selection?.revision || source?.revision || revision);
  return {
    path: anchor.path || text(path),
    selection: anchor,
    revision: capturedRevision,
    // A rendered quotation is useful context, but only a source selector
    // captured by the reader is safe for an edit suggestion.
    anchored: Boolean((hasSource ? selection.source : source?.path) && anchor.path && capturedRevision),
  };
}

export function scopeLabel(scope, { path = "", attached = null } = {}) {
  if (scope === "selection") return attached?.path ? `Selected passage · ${attached.path}` : "Selected passage";
  if (scope === "file") return path ? `Current file · ${path}` : "Current file";
  return "Whole document";
}

export function normalizeCapabilities(value) {
  const raw = value?.capabilities && typeof value.capabilities === "object"
    ? value.capabilities
    : (value && typeof value === "object" ? value : {});
  const known = ["can_read", "can_comment", "can_edit", "can_suggest", "can_reply"];
  const hasAny = known.some((key) => typeof raw[key] === "boolean");
  return {
    verified: value?.verified !== false && (value?.verified === true || hasAny),
    can_read: raw.can_read === true,
    can_comment: raw.can_comment === true || raw.can_suggest === true,
    can_edit: raw.can_edit === true,
    can_suggest: raw.can_suggest === true || raw.can_comment === true,
    can_reply: raw.can_reply === true || raw.can_comment === true,
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
  if (taskNeedsAnchor(task) && (!attached?.anchored || !attached?.revision)) return false;
  if (task.scope === "file" && !path) return false;
  if (["tighten", "rewrite", "proofread", "fix", "refine"].includes(task.kind) && !caps.can_suggest) return false;
  if (task.kind === "respond" && !caps.can_reply) return false;
  return true;
}

export function diagnosticContext(diagnostic, revision = "") {
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

export function composeTaskMessage({ id, text: body = "", task, attachment = null, selection = null, path = "", revision = "", diagnostic = null, suggestion = null, thread = null } = {}) {
  const context = {};
  const selected = task?.scope === "selection" || !task
    ? (attachment || (selection ? captureAttachment(selection, path, revision) : null))
    : null;
  const file = selected?.path || (task?.scope === "file" || !task ? path : "");
  if (file) context.file = file;
  if (selected?.selection) context.selection = selected.selection;
  const currentRevision = selected?.revision || revision;
  if (currentRevision) context.revision = currentRevision;
  const error = diagnosticContext(diagnostic, revision || diagnostic?.revision || "");
  if (error) context.diagnostic = error;
  const refinement = suggestionContext(suggestion);
  if (refinement) context.suggestion = refinement;
  if (thread && typeof thread === "object") context.thread = thread;
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
  if (!results || typeof results !== "object") return { suggestions: [], pass: null };
  const suggestions = Array.isArray(results.suggestions)
    ? results.suggestions.map(text).filter(Boolean)
    : [];
  return { suggestions, pass: results.pass ? text(results.pass) : null };
}

export function visibleResults(message, comments = []) {
  const result = resultIds(message);
  const editing = comments.filter((comment) => comment?.motivation === "editing");
  const visible = new Set(editing.map((comment) => text(comment?.id)));
  const suggestions = result.suggestions.filter((id) => visible.has(id));
  const pass = result.pass && editing.some((comment) => comment.pass === result.pass) ? result.pass : null;
  return { suggestions, pass };
}

export function groupPass(comments = [], pass) {
  const items = comments.filter((comment) => pass && comment?.pass === pass && comment?.motivation === "editing");
  const pending = items.filter((comment) => !comment.resolved);
  return { pass, total: items.length, pending: pending.length, suggestions: items.map((item) => item.id) };
}
