// Pure assistant-panel helpers. Keeping task and context composition here
// makes it possible to verify the wire contract without mounting Svelte.

export const TASK_KINDS = ["proofread", "tighten", "rewrite", "explain", "fix", "outline", "respond"];
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

// The wire vocabulary is fixed (see docs/dev/protocol/chat.md), but a launcher
// entry is a task the user recognizes: a kind plus the request it phrases.
// Several entries may share a kind, which is what lets the catalog grow
// without touching the protocol.
export const TASK_CATALOG = [
  { id: "proofread", kind: "proofread", category: "Writing", label: "Proofread",
    description: "Correct spelling, grammar, and wording while preserving meaning.",
    keywords: ["grammar", "spelling", "typos", "errors", "mistakes"],
    prompt: "Proofread {target}." },
  { id: "tighten", kind: "tighten", category: "Writing", label: "Tighten",
    description: "Cut redundancy and shorten without losing substance.",
    keywords: ["shorter", "concise", "shorten", "trim", "cut"],
    scopes: ["selection"], prompt: "Tighten {target}." },
  { id: "rewrite", kind: "rewrite", category: "Writing", label: "Rewrite",
    description: "Rework the wording for clarity and flow.",
    keywords: ["rephrase", "reword", "clarity", "flow"],
    scopes: ["selection"], prompt: "Rewrite {target}." },
  { id: "explain", kind: "explain", category: "Writing", label: "Explain",
    description: "Say what this passage claims and how it is put together.",
    keywords: ["meaning", "understand", "clarify", "unclear"],
    prompt: "Explain {target}." },

  { id: "review-argument", kind: "explain", category: "Review", label: "Review the argument",
    description: "Name unsupported claims, gaps in reasoning, and weak transitions.",
    keywords: ["logic", "reasoning", "claims", "critique", "support"],
    prompt: "Review the argument in {target}. Name unsupported claims, gaps in reasoning, and weak transitions." },
  { id: "check-consistency", kind: "explain", category: "Review", label: "Check consistency",
    description: "Find contradictions and inconsistent terminology, notation, or definitions.",
    keywords: ["terminology", "notation", "definitions", "consistent", "conflicts", "contradictions", "contradict"],
    prompt: "Check {target} for inconsistent terminology, notation, and definitions." },
  { id: "check-citations", kind: "explain", category: "Review", label: "Check citations",
    description: "Check that each citation resolves in the bibliography and supports its claim.",
    keywords: ["references", "bibliography", "sources", "cite", "refs"],
    prompt: "Check the citations in {target}: each one should resolve in the bibliography and support the claim it is attached to." },
  { id: "suggest-improvements", kind: "proofread", category: "Review", label: "Suggest improvements",
    description: "Propose concrete anchored edits rather than commentary.",
    keywords: ["edits", "feedback", "improve", "suggestions", "better"],
    prompt: "Suggest concrete improvements to {target} as anchored edits." },

  { id: "outline", kind: "outline", category: "Document", label: "Outline",
    description: "List the headings and what each section does.",
    keywords: ["headings", "structure", "sections", "layout", "toc"],
    scopes: ["file", "document"], prompt: "Outline {target}: list the headings and say what each section does." },
  { id: "summarize", kind: "explain", category: "Document", label: "Summarize",
    description: "Summarize the argument in a few paragraphs.",
    keywords: ["summary", "abstract", "overview", "tldr", "recap"],
    scopes: ["file", "document"], prompt: "Summarize {target}." },
];

// Context tasks are offered only when the reader is already looking at a
// comment or diagnostic. They are never searched or browsed.
export const CONTEXT_TASKS = [
  { id: "address-comment", kind: "respond", category: "Context", label: "Address comment",
    description: "Answer the comment, and change the passage it is about when that helps.",
    prompt: "Address this comment." },
  { id: "fix-diagnostic", kind: "fix", category: "Context", label: "Fix diagnostic",
    description: "Find what causes this compiler message and fix the source.",
    scopes: ["file", "document"], prompt: "Fix this diagnostic." },
];

export const TASK_CATEGORIES = [...new Set(TASK_CATALOG.map((entry) => entry.category))];

export const TASK_TARGETS = { selection: "the selected passage", file: "this file", document: "the whole document" };

export function findTask(id) {
  return TASK_CATALOG.find((entry) => entry.id === id)
    || CONTEXT_TASKS.find((entry) => entry.id === id)
    || null;
}

// The entry a kind is shown as. A catalog id wins, then the first catalog
// entry of that kind, then a context entry.
export function taskForKind(kind) {
  return TASK_CATALOG.find((entry) => entry.id === kind)
    || TASK_CATALOG.find((entry) => entry.kind === kind)
    || CONTEXT_TASKS.find((entry) => entry.kind === kind)
    || null;
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

function editDistance(a, b) {
  let previous = Array.from({ length: b.length + 1 }, (_, j) => j);
  for (let i = 1; i <= a.length; i += 1) {
    const current = [i];
    for (let j = 1; j <= b.length; j += 1) {
      const cost = a[i - 1] === b[j - 1] ? 0 : 1;
      current[j] = Math.min(previous[j] + 1, current[j - 1] + 1, previous[j - 1] + cost);
    }
    previous = current;
  }
  return previous[b.length];
}

// The best tier a single query word reaches in an entry, or null when the
// word does not match at all. Lower tiers are stronger matches.
function wordTier(word, entry, phrase) {
  const label = entry.label.toLowerCase();
  const labelWords = label.split(/\s+/);
  const keywords = entry.keywords || [];
  const context = `${entry.description || ""} ${entry.category || ""}`.toLowerCase();
  if (label.startsWith(phrase)) return 0;
  if (labelWords.some((item) => item.startsWith(word))) return 1;
  if (label.includes(word)) return 2;
  if (keywords.some((item) => item.startsWith(word) || item.includes(word))) return 3;
  if (context.includes(word)) return 4;
  const limit = word.length >= 7 ? 2 : word.length >= 4 ? 1 : 0;
  if (limit && [...labelWords, ...keywords].some((item) => (
    editDistance(word, item) <= limit || editDistance(word, item.slice(0, word.length)) <= limit
  ))) return 5;
  return null;
}

// A command-palette match: every word the user typed has to match somewhere
// in the entry. Matches are ranked by the sum of their word tiers, and typos
// still find an entry through the fuzzy tier.
export function searchTasks(query, catalog = TASK_CATALOG) {
  const words = String(query || "").toLowerCase().split(/\s+/).filter(Boolean);
  if (!words.length) return [...catalog];
  const phrase = words.join(" ");
  const scored = [];
  for (const entry of catalog) {
    const tiers = words.map((word) => wordTier(word, entry, phrase));
    if (tiers.some((tier) => tier === null)) continue;
    scored.push({ entry, score: tiers.reduce((sum, tier) => sum + tier, 0) });
  }
  return scored.sort((a, b) => a.score - b.score).map((item) => item.entry);
}

// The catalog as the Tasks browser shows it: one section per category, in
// the order categories first appear, each keeping the catalog's own order.
// A category narrows it to that one section.
export function taskSections(category = "", catalog = TASK_CATALOG) {
  const sections = [];
  for (const entry of catalog) {
    if (category && entry.category !== category) continue;
    let section = sections.find((item) => item.category === entry.category);
    if (!section) {
      section = { category: entry.category, entries: [] };
      sections.push(section);
    }
    section.entries.push(entry);
  }
  return sections;
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
  return task?.scope === "selection" && ["tighten", "rewrite"].includes(task?.kind);
}

// The reason a task cannot run here, or "" when it may run. The checks are
// the same ones capabilityAllows has always made, in the same order. With
// deferAccess, every access check waits for the agent start, which verifies
// the access the agent is given. The capabilities held before that describe
// another link.
export function taskBlocker(capabilities, task, { attached = null, path = "", deferAccess = false } = {}) {
  const caps = normalizeCapabilities(capabilities);
  const checkAccess = !deferAccess;
  if (!task || !TASK_KINDS.includes(task.kind) || !TASK_SCOPES.includes(task.scope)) return "This task cannot run here.";
  if (!deferAccess && !caps.verified) return "Connect an agent to check access for this task.";
  if (checkAccess && !caps.can_read) return "You need read access to run this task.";
  if (["tighten", "rewrite"].includes(task.kind) && task.scope !== "selection") return "This task works on a selected passage.";
  if (task.scope === "selection" && !attached) return "Select text in the editor to use this task.";
  if (taskNeedsAnchor(task) && !text(attached?.selection?.exact).trim()) return "Select the passage in the source to use this task.";
  if (task.scope === "file" && !path) return "Open a file to use this task.";
  if (checkAccess && ["tighten", "rewrite", "proofread", "fix"].includes(task.kind) && !caps.can_suggest) return "You need edit access to run this task.";
  if (checkAccess && task.kind === "respond" && !caps.can_reply) return "You need comment access to run this task.";
  return "";
}

export function capabilityAllows(capabilities, task, context = {}) {
  return taskBlocker(capabilities, task, context) === "";
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

/** @typedef {{kind: string, scope: string}} AssistantTask */
/** @typedef {{type: "message", id: string, text: string, task?: AssistantTask, context: Record<string, unknown>}} AssistantTaskMessage */
/** @param {{id?: string, text?: string, task?: AssistantTask, attachment?: object|null, selection?: object|null, path?: string, revision?: string, diagnostic?: object|null, diagnostics?: object[], thread?: object|null}} [options] @returns {AssistantTaskMessage} */
export function composeTaskMessage({ id, text: body = "", task, attachment = null, selection = null, path = "", revision = "", diagnostic = null, diagnostics = [], thread = null } = {}) {
  /** @type {Record<string, unknown> & {diagnostics: object[], diagnostics_omitted: number}} */
  const context = { diagnostics: [], diagnostics_omitted: 0 };
  const selected = task?.scope === "selection" || !task
    ? (attachment || (selection ? captureAttachment(selection, path, revision) : null))
    : null;
  const file = selected?.path || (task?.scope === "file" || !task ? path : "");
  if (file) context.file = file;
  if (selected?.selection) context.selection = selected.selection;
  const commentId = thread?.id;
  const currentRevision = selected?.revision || (selected?.render_digest ? "" : revision);
  if (commentId) context.comment_id = text(commentId);
  else {
    if (currentRevision) context.revision = currentRevision;
    if (selected?.render_digest) context.render_digest = selected.render_digest;
  }
  const error = taskDiagnosticContext(diagnostic, revision || diagnostic?.revision || "");
  if (error) context.diagnostic = error;
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
  if (!results || typeof results !== "object") return { changes: [] };
  const changes = Array.isArray(results.changes) ? results.changes.map(text).filter(Boolean) : [];
  return { changes };
}

// The ids of a task's tracked changes that are still pending in the document.
export function visibleResults(message, changes = []) {
  const visible = new Set(changes.map((change) => text(change?.id)));
  return { changes: resultIds(message).changes.filter((id) => visible.has(id)) };
}
