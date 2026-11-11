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

/// Reads the Git panel's one field: a remote address or a folder on this computer.
/** @param {string} raw @returns {{ kind: "remote" | "local", value: string, problem: string }} */
export function repositoryInput(raw) {
  const value = String(raw ?? "").trim();
  if (!value) return { kind: "remote", value, problem: m.git_url_empty_input() };
  if (/^file:\/\//i.test(value)) return { kind: "local", value, problem: m.git_url_file_scheme() };
  if (value.startsWith("/") || value.startsWith("~") || value.startsWith("\\\\") || /^[A-Za-z]:[\\/]/.test(value)) {
    if (value.length > 4096) return { kind: "local", value, problem: m.git_url_path_too_long() };
    if (/[\u0000-\u001f\u007f]/.test(value)) return { kind: "local", value, problem: m.git_url_path_control() };
    return { kind: "local", value, problem: "" };
  }
  if (/^\.\.?(?:[\\/]|$)/.test(value)) return { kind: "local", value, problem: m.git_url_relative_path() };
  if (value.includes("://") || value.includes("@")) return { kind: "remote", value, problem: remoteUrlProblem(value) };
  return { kind: "remote", value, problem: m.git_url_unrecognized() };
}

/// Names the repository the desktop app is connected to, for the Git panel: a
/// folder's name and path, or a remote's path, host and, when the host has one,
/// a web page. Never throws; an address it cannot read is its own name.
/** @typedef {{ name: string, detail: string, full: string, href: string, icon: "github" | "git-fork" | "folder", local: boolean }} RepositoryIdentity */
/** @param {any} status @returns {RepositoryIdentity} */
export function repositoryIdentity(status) {
  const local = Boolean(status?.local);
  const text = String(local ? (status?.path ?? "") : (status?.url ?? ""));
  try {
    return local ? localIdentity(text) : remoteIdentity(text);
  } catch {
    return { name: text, detail: "", full: text, href: "", icon: /** @type {RepositoryIdentity["icon"]} */ (local ? "folder" : "git-fork"), local };
  }
}

/** @param {string} path @returns {RepositoryIdentity} */
function localIdentity(path) {
  const segments = path.split(/[\\/]+/).filter(Boolean);
  return { name: segments.length ? segments[segments.length - 1] : path, detail: path, full: path, href: "", icon: "folder", local: true };
}

/** @param {string} url @returns {RepositoryIdentity} */
function remoteIdentity(url) {
  let host = "";
  let webHost = "";
  let path = "";
  let https = false;
  if (url.includes("://")) {
    const parsed = new URL(url);
    https = parsed.protocol === "https:";
    host = parsed.hostname.toLowerCase();
    webHost = parsed.host.toLowerCase();
    path = parsed.pathname;
  } else {
    // scp-like: user@host:path, where the user is optional here.
    const rest = url.slice(url.indexOf("@") + 1);
    const colon = rest.indexOf(":");
    if (colon < 0) throw new Error("not an address");
    host = rest.slice(0, colon).toLowerCase();
    webHost = host;
    path = rest.slice(colon + 1);
  }
  const clean = path.replace(/\/+$/, "").replace(/\.git$/, "").replace(/^\/+/, "");
  // An SSH alias such as github-work names no web page, so only a dotted host
  // gets a link; https always names its page.
  let href = "";
  if (clean && https) href = `https://${webHost}/${clean}`;
  else if (clean && host.includes(".")) href = `https://${host}/${clean}`;
  const github = host === "github.com" || host.endsWith(".github.com");
  return { name: clean || url, detail: host, full: url, href, icon: github ? "github" : "git-fork", local: false };
}
