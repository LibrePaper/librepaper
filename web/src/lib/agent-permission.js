// Pure helpers for the inline permission card in the Chat tab. The agent
// speaks in tool names -- its own, or an MCP server's -- and this turns those
// into the plain words a reader sees, without inventing anything the agent
// did not actually say.

// The LibrePaper MCP surface, named for what each tool actually does rather
// than its wire name. Kept here, not in the server, because this is a display
// concern only: the request itself still carries the real tool name.
const LIBREPAPER_TOOLS = {
  document_read: "Read the current document",
  document_propose: "Suggest changes to the document",
  document_apply: "Edit the document",
  document_comment: "Comment on the document",
  document_result: "Report the result",
};

const MCP_TOKEN = /\bmcp__([a-z0-9_-]+)__([a-z0-9_-]+)\b/gi;
const BARE_TOOL = new RegExp(`\\b(${Object.keys(LIBREPAPER_TOOLS).join("|")})\\b`, "g");

/// Rewrite any `mcp__<server>__<tool>` token, and any bare LibrePaper tool
/// name, into plain words. Anything else in the string is left alone: this
/// is a targeted substitution, not a paraphrase of the whole message.
export function humanizeTool(text) {
  if (!text) return "";
  return String(text)
    .replace(MCP_TOKEN, (whole, server, tool) => {
      const known = LIBREPAPER_TOOLS[tool.toLowerCase()];
      if (known) return known;
      return `Use ${tool.replace(/_/g, " ")} (${server})`;
    })
    .replace(BARE_TOOL, (tool) => LIBREPAPER_TOOLS[tool] || tool);
}

/// The one line the permission card states: what the agent wants to do, in
/// its own words once humanized, or a neutral fallback when it said nothing.
export function permissionAction(input) {
  const humanized = humanizeTool(input?.message || "").trim();
  return humanized || "Allow the agent to continue";
}

const KIND_ORDER = ["allow_once", "allow_always", "reject_once", "reject_always"];
const KIND_LABELS = {
  allow_once: "Allow",
  allow_always: "Always allow",
  reject_once: "Reject",
  reject_always: "Always reject",
};

/// The agent names its own options; this only orders and labels them. A kind
/// outside the known four keeps the agent's own label rather than guessing
/// one, and sorts after the known kinds.
export function permissionButtons(options) {
  const list = Array.isArray(options) ? options : [];
  return list
    .map((option, index) => ({
      id: option?.id ?? `option-${index}`,
      kind: String(option?.kind || "other").toLowerCase(),
      label: option?.label || "",
    }))
    .sort((a, b) => {
      const rank = (kind) => { const at = KIND_ORDER.indexOf(kind); return at === -1 ? KIND_ORDER.length : at; };
      return rank(a.kind) - rank(b.kind);
    })
    .map((option) => ({
      id: option.id,
      label: KIND_LABELS[option.kind] || option.label,
      primary: option.kind === "allow_once",
    }));
}
