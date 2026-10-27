// What a git address may look like before the page sends it to the desktop
// app, which checks it again before it reaches a command line. The forms are
// https://host/path, ssh://user@host/path and scp-like user@host:path. An
// address that carries a password is refused here, so a secret typed into a
// field is never sent anywhere.

/// The reason `raw` cannot be used, or an empty string when it can.
/** @param {string} raw @returns {string} */
export function remoteUrlProblem(raw) {
  const url = String(raw ?? "").trim();
  if (!url) return "Enter an address.";
  if (url.length > 2048) return "That address is too long.";
  if (url.startsWith("-")) return "That address cannot start with a dash.";
  if (/[\s\u0000-\u001f\u007f]/.test(url)) return "That address has a space or a control character in it.";
  if (url.includes("://")) {
    let parsed;
    try {
      parsed = new URL(url);
    } catch {
      return "That is not a valid address.";
    }
    if (parsed.protocol !== "https:" && parsed.protocol !== "ssh:") return "Use an https:// or ssh:// address, or user@host:path.";
    if (parsed.password) return "That address contains a password. Use a credential helper or an SSH key instead.";
    if (!parsed.hostname || parsed.hostname.startsWith("-")) return "That address has no usable host.";
    return "";
  }
  const at = url.indexOf("@");
  if (at < 0) return "Use an https:// or ssh:// address, or user@host:path.";
  const user = url.slice(0, at);
  const rest = url.slice(at + 1);
  const colon = rest.indexOf(":");
  if (colon < 0) return "An SSH address needs the form user@host:path.";
  const host = rest.slice(0, colon);
  const path = rest.slice(colon + 1);
  if (!/^[A-Za-z0-9._-]+$/.test(user) || !/^[A-Za-z0-9.-]+$/.test(host) || host.startsWith("-") || !path) {
    return "An SSH address needs the form user@host:path.";
  }
  return "";
}
