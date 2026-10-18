// Turns text and addresses into forms that are safe to show in a report.
//
// This is the second line of defence. The report is built from fields we
// choose, so most of it never needs scrubbing. These helpers cover the fields
// that carry free text, such as an error message, and the routes of requests.

const QUOTED = /"[^"]*"|'[^']*'|`[^`]*`|“[^”]*”|‘[^’]*’/g;
const URL_TEXT = /https?:\/\/[^\s)"'<>]+/g;
const EMAIL = /[A-Za-z0-9._%+-]+@[A-Za-z0-9-]+(?:\.[A-Za-z0-9-]+)+/g;
const PREFIXED = /\blp[at]?_[A-Za-z0-9_-]*/g;
const HEX_RUN = /[0-9a-fA-F]{16,}/g;
const BASE64_RUN = /[A-Za-z0-9_-]{24,}/g;
const UNIX_PATH = /\/(?:home|Users)\/[^\s"'`)]*/g;
const WINDOWS_PATH = /\b[A-Za-z]:\\[^\s"'`)]*/g;

const SLUG_AFTER = new Set(["docs", "documents", "ws"]);
// The words the server's own routes are made of. Any other segment is a slug,
// an id, a folder or a file name chosen by somebody, and is replaced.
const ROUTE_WORDS = new Set([
  "api", "docs", "ws", "assets", "documents", "account", "auth", "backup", "config", "device",
  "erase", "list", "me", "projects", "storage", "templates", "trash", "agent", "agent-token",
  "archive", "assistant", "candidates", "capabilities", "chat", "checkpoint", "comments", "delete",
  "favorite", "fork", "history", "opened", "project", "purge", "quarto", "rename", "replies",
  "restore", "share", "shared", "snapshot", "source", "state", "template", "tools", "transfer",
  "trim", "untrash", "shell", "signin",
]);

const mapPath = (pathname) => {
  const segments = pathname.split("/");
  const underAssets = segments[1] === "assets";
  return segments.map((segment, index) => {
    if (!segment) return segment;
    if (SLUG_AFTER.has(segments[index - 1])) return ":slug";
    // Bundle files are named by their content hash, which says nothing private.
    if (underAssets && index === segments.length - 1) return segment;
    if (ROUTE_WORDS.has(segment)) return segment;
    return segment.includes(".") ? ":file" : ":id";
  }).join("/");
};

/// The route pattern of a URL, with no query or fragment and no real slugs,
/// ids or file names. A URL on another origin keeps its host in front.
export function routePattern(url) {
  const here = globalThis.location?.origin || "http://localhost";
  let parsed;
  try {
    parsed = new URL(String(url?.href ?? url), here);
  } catch {
    return "";
  }
  const path = mapPath(parsed.pathname);
  return parsed.origin === here ? path : `${parsed.host}${path}`;
}

const describeUrl = (text) => {
  try {
    const parsed = new URL(text);
    return `${parsed.origin}${mapPath(parsed.pathname)}`;
  } catch {
    return "[redacted]";
  }
};

/// Text that is safe to put in a report. Quoted text, URL details, tokens, email
/// addresses and absolute paths are replaced, whitespace is collapsed and the
/// result is cut to `max` characters.
export function scrubMessage(text, max = 120) {
  let out = String(text ?? "").replace(QUOTED, '"..."');
  // URLs are set aside so the token rules below cannot mangle a host name.
  const urls = [];
  out = out.replace(URL_TEXT, (found) => {
    urls.push(describeUrl(found));
    return `\u0000${urls.length - 1}\u0000`;
  });
  out = out
    .replace(EMAIL, "[redacted]")
    .replace(PREFIXED, "[redacted]")
    .replace(HEX_RUN, "[redacted]")
    .replace(BASE64_RUN, "[redacted]")
    .replace(UNIX_PATH, "[path]")
    .replace(WINDOWS_PATH, "[path]")
    .replace(/\u0000(\d+)\u0000/g, (_, index) => urls[Number(index)])
    .replace(/\s+/g, " ")
    .trim();
  return out.length > max ? `${out.slice(0, max)}...` : out;
}
