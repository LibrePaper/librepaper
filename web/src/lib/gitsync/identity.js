/// Hosts whose repository pages the panel can link to.
const WEB_HOSTS = new Set(["github.com", "gitlab.com", "codeberg.org", "bitbucket.org", "sr.ht", "git.sr.ht"]);
/// A remote written as a URL: `https://host/path`, `ssh://user@host/path` or `git://host/path`.
const URL_FORM = /^(?:https?|ssh|git):\/\/(?:[^@/]+@)?([^/:]+)(?::\d+)?\/(.+)$/;
/// A remote written in scp style: `host:path` or `user@host:path`.
const SCP_FORM = /^(?:[^@/]+@)?([^/:]+):(?!\/\/)(.+)$/;

/// The host and the repository path of a network remote, or null when the
/// label is neither form. The path has no leading or trailing slash and no
/// `.git` suffix.
function parseRemote(label) {
  const match = URL_FORM.exec(label) || SCP_FORM.exec(label);
  if (!match) return null;
  const host = match[1].toLowerCase();
  const path = match[2].replace(/^\/+|\/+$/g, "").replace(/\.git$/, "");
  return path ? { host, path } : null;
}

/// How the panel names a remote. `label` is the connection's remote_label:
/// a URL without credentials, an scp-style `host:owner/repo.git`, or a local
/// folder's name; `kind` is "local" or "network". `href` is a page of the
/// repository on a known host, else null.
export function remoteIdentity(label, kind) {
  if (kind === "local") return { name: label || "Local repository", href: null, full: label };
  const parsed = parseRemote(label);
  if (!parsed) return { name: label, href: null, full: label };
  const href = WEB_HOSTS.has(parsed.host) ? `https://${parsed.host}/${parsed.path}` : null;
  return { name: parsed.path, href, full: label };
}
