/// The steps to take when Git could not sign in to a remote. `remote` is the
/// remote as typed or as the connection stores it: a URL without credentials,
/// or scp-style `host:path`. `platform` is "mac", "windows" or "linux".
export function signInHelp(remote, platform) {
  const text = String(remote || "").trim();
  const target = parseRemote(text);
  if (!target) {
    return {
      title: "Git could not sign in to the remote.",
      steps: [{ text: "Set up a credential helper or an SSH key for this remote, then try again.", href: "/docs/git.html#credentials" }],
    };
  }
  if (target.transport === "ssh") return sshHelp(target.host, platform);
  if (target.host === "github.com") return githubHelp();
  return httpsHelp(target.host, text, platform);
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
    title: `Git could not sign in to ${host} over SSH.`,
    steps: [
      { text: "Add your SSH key to the agent.", command: add },
      { text: `Check that ${host} accepts the key. Answer yes if SSH asks to trust the host.`, command: `ssh -T git@${host}` },
      { text: "Then try again." },
    ],
  };
}

function githubHelp() {
  return {
    title: "Git could not sign in to github.com.",
    steps: [
      { text: "Install the GitHub CLI.", href: "https://cli.github.com" },
      { text: "Sign in once in a terminal.", command: "gh auth login" },
      { text: "Let Git use that sign-in.", command: "gh auth setup-git" },
      { text: "Then try again." },
    ],
  };
}

function httpsHelp(host, remote, platform) {
  const title = `Git could not sign in to ${host}.`;
  const probe = `git ls-remote ${remote}`;
  if (platform === "windows") {
    return {
      title,
      steps: [
        { text: "Git for Windows includes Git Credential Manager. Sign in once from a terminal.", command: probe },
        { text: "Then try again." },
      ],
    };
  }
  if (platform === "mac") {
    return {
      title,
      steps: [
        { text: "Save credentials in the macOS keychain.", command: "git config --global credential.helper osxkeychain" },
        { text: `Sign in once from a terminal. Use an access token from ${host} as the password.`, command: probe },
        { text: "Then try again." },
      ],
    };
  }
  return {
    title,
    steps: [
      { text: "Install Git Credential Manager, which stores sign-ins in the system keyring.", href: "https://github.com/git-ecosystem/git-credential-manager" },
      { text: `Sign in once from a terminal. Use an access token from ${host} as the password.`, command: probe },
      { text: "Then try again." },
    ],
  };
}
