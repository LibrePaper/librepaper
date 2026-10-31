import { m } from "../paraglide/messages.js";
import { GIT_DOCS_URL } from "../site-links.js";

/// The steps to take when Git could not sign in to a remote. `remote` is the
/// remote as typed or as the connection stores it: a URL without credentials,
/// or scp-style `host:path`. `platform` is "mac", "windows" or "linux".
/// `name` is the remote's name, such as "origin". The commands use it in place
/// of the address, so the address never ends up in a command to run. It
/// defaults to "origin".
export function signInHelp(remote, platform, name = "origin") {
  const text = String(remote || "").trim();
  const target = parseRemote(text);
  if (!target) {
    return {
      title: m.sign_in_help_title_remote(),
      steps: [{ text: m.sign_in_help_setup_remote(), href: `${GIT_DOCS_URL}#credentials` }],
    };
  }
  if (target.transport === "ssh") return sshHelp(target.host, platform);
  if (target.host === "github.com") return githubHelp();
  return httpsHelp(target.host, name, platform);
}

/// The platform of this computer: "mac", "windows" or "linux".
export function currentPlatform() {
  if (typeof navigator === "undefined") return "linux";
  // userAgentData is Chromium only and missing from the DOM types.
  const data = /** @type {{ userAgentData?: { platform?: string } }} */ (/** @type {unknown} */ (navigator)).userAgentData;
  const hint = data?.platform || navigator.platform || navigator.userAgent || "";
  if (/mac|darwin|iphone|ipad/i.test(hint)) return "mac";
  if (/\bwin/i.test(hint)) return "windows";
  return "linux";
}

/// The transport and host of a remote, or null when it is neither SSH nor
/// HTTPS. A `ssh://` or scp-style remote is SSH. An `http://` or `https://`
/// remote is HTTPS.
function parseRemote(value) {
  const url = /^([a-z][a-z0-9+.-]*):\/\/(?:[^@/]*@)?([^/:]+)/i.exec(value);
  if (url) {
    const scheme = url[1].toLowerCase();
    const host = url[2].toLowerCase();
    if (scheme === "ssh") return { transport: "ssh", host };
    if (scheme === "https" || scheme === "http") return { transport: "https", host };
    return null;
  }
  const scp = /^(?:[^@/]+@)?([^/:]+):(?!\/\/)(.+)$/.exec(value);
  return scp ? { transport: "ssh", host: scp[1].toLowerCase() } : null;
}

function sshHelp(host, platform) {
  const add = platform === "mac"
    ? "ssh-add --apple-use-keychain ~/.ssh/id_ed25519"
    : "ssh-add ~/.ssh/id_ed25519";
  return {
    title: m.sign_in_help_ssh_title({ host }),
    steps: [
      { text: m.sign_in_help_ssh_add(), command: add },
      { text: m.sign_in_help_ssh_check({ host }), command: `ssh -T git@${host}` },
      { text: m.sign_in_help_retry() },
    ],
  };
}

function githubHelp() {
  return {
    title: m.sign_in_help_github_title(),
    steps: [
      { text: m.sign_in_help_github_install(), href: "https://cli.github.com" },
      { text: m.sign_in_help_github_login(), command: "gh auth login" },
      { text: m.sign_in_help_github_setup(), command: "gh auth setup-git" },
      { text: m.sign_in_help_retry() },
    ],
  };
}

function httpsHelp(host, name, platform) {
  const title = m.sign_in_help_https_title({ host });
  // The probe names the remote and never its address. A name is plain letters,
  // digits, dots, dashes and underscores; anything else falls back to origin, so
  // no shell syntax reaches the command.
  const probe = `git ls-remote ${/^[A-Za-z0-9._-]+$/.test(name) ? name : "origin"}`;
  if (platform === "windows") {
    return {
      title,
      steps: [
        { text: m.sign_in_help_windows_signin(), command: probe },
        { text: m.sign_in_help_retry() },
      ],
    };
  }
  if (platform === "mac") {
    return {
      title,
      steps: [
        { text: m.sign_in_help_mac_keychain(), command: "git config --global credential.helper osxkeychain" },
        { text: m.sign_in_help_token_signin({ host }), command: probe },
        { text: m.sign_in_help_retry() },
      ],
    };
  }
  return {
    title,
    steps: [
      { text: m.sign_in_help_linux_install(), href: "https://github.com/git-ecosystem/git-credential-manager" },
      { text: m.sign_in_help_token_signin({ host }), command: probe },
      { text: m.sign_in_help_retry() },
    ],
  };
}
