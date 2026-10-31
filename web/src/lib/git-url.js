import { m } from "./paraglide/messages.js";

// What a git address may look like before the page sends it to the desktop
// app, which checks it again before it reaches a command line. The forms are
// https://host/path, ssh://user@host/path and scp-like user@host:path. An
// address that carries a password is refused here, so a secret typed into a
// field is never sent anywhere.

/// The reason `raw` cannot be used, or an empty string when it can.
/** @param {string} raw @returns {string} */
export function remoteUrlProblem(raw) {
  const url = String(raw ?? "").trim();
  if (!url) return m.git_url_empty();
  if (url.length > 2048) return m.git_url_too_long();
  if (url.startsWith("-")) return m.git_url_starts_dash();
  if (/[\s\u0000-\u001f\u007f]/.test(url)) return m.git_url_space_or_control();
  if (url.includes("://")) {
    let parsed;
    try {
      parsed = new URL(url);
    } catch {
      return m.git_url_invalid();
    }
    if (parsed.protocol !== "https:" && parsed.protocol !== "ssh:") return m.git_url_use_scheme();
    if (parsed.password) return m.git_url_password();
    if (!parsed.hostname || parsed.hostname.startsWith("-")) return m.git_url_no_host();
    return "";
  }
  const at = url.indexOf("@");
  if (at < 0) return m.git_url_use_scheme();
  const user = url.slice(0, at);
  const rest = url.slice(at + 1);
  const colon = rest.indexOf(":");
  if (colon < 0) return m.git_url_ssh_form();
  const host = rest.slice(0, colon);
  const path = rest.slice(colon + 1);
  if (!/^[A-Za-z0-9._-]+$/.test(user) || !/^[A-Za-z0-9.-]+$/.test(host) || host.startsWith("-") || !path) {
    return m.git_url_ssh_form();
  }
  return "";
}
