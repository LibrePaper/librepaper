// The sign-in help for each kind of remote and platform.

import assert from "node:assert/strict";

import { signInHelp } from "../../src/lib/gitsync/sign-in-help.js";

// scp-style GitHub on a Mac: the key goes into the keychain.
const scpGithub = signInHelp("git@github.com:owner/repo.git", "mac");
assert.equal(scpGithub.title, "Git could not sign in to github.com over SSH.");
assert.deepEqual(scpGithub.steps[0], { text: "Add your SSH key to the agent.", command: "ssh-add --apple-use-keychain ~/.ssh/id_ed25519" });
assert.deepEqual(scpGithub.steps[1], {
  text: "Check that github.com accepts the key. Answer yes if SSH asks to trust the host.",
  command: "ssh -T git@github.com",
});
assert.deepEqual(scpGithub.steps[2], { text: "Then try again." });

// ssh:// GitLab on Linux: a plain ssh-add.
const sshGitlab = signInHelp("ssh://git@gitlab.com/group/repo.git", "linux");
assert.equal(sshGitlab.title, "Git could not sign in to gitlab.com over SSH.");
assert.deepEqual(sshGitlab.steps[0], { text: "Add your SSH key to the agent.", command: "ssh-add ~/.ssh/id_ed25519" });
assert.equal(sshGitlab.steps[1].command, "ssh -T git@gitlab.com");

// HTTPS GitHub on Windows: the GitHub CLI, and no ls-remote.
const githubWindows = signInHelp("https://github.com/owner/repo.git", "windows");
assert.deepEqual(githubWindows, {
  title: "Git could not sign in to github.com.",
  steps: [
    { text: "Install the GitHub CLI.", href: "https://cli.github.com" },
    { text: "Sign in once in a terminal.", command: "gh auth login" },
    { text: "Let Git use that sign-in.", command: "gh auth setup-git" },
    { text: "Then try again." },
  ],
});
assert.ok(!JSON.stringify(githubWindows).includes("ls-remote"));

// HTTPS GitLab on a Mac: the keychain helper, then ls-remote with the remote's name.
const gitlabMac = signInHelp("https://gitlab.com/group/repo.git", "mac");
assert.equal(gitlabMac.title, "Git could not sign in to gitlab.com.");
assert.deepEqual(gitlabMac.steps[0], {
  text: "Save credentials in the macOS keychain.",
  command: "git config --global credential.helper osxkeychain",
});
assert.deepEqual(gitlabMac.steps[1], {
  text: "Sign in once from a terminal. Use an access token from gitlab.com as the password.",
  command: "git ls-remote origin",
});
assert.deepEqual(gitlabMac.steps[2], { text: "Then try again." });

// HTTPS on another host on Linux: a link to Git Credential Manager.
const codebergLinux = signInHelp("https://codeberg.org/owner/repo.git", "linux");
assert.equal(codebergLinux.title, "Git could not sign in to codeberg.org.");
assert.deepEqual(codebergLinux.steps[0], {
  text: "Install Git Credential Manager, which stores sign-ins in the system keyring.",
  href: "https://github.com/git-ecosystem/git-credential-manager",
});
assert.equal(codebergLinux.steps[1].command, "git ls-remote origin");

// A URL with shell syntax in it gives a probe with the remote's name only.
const hostile = signInHelp("https://example.com/o/r$(id).git", "linux");
assert.ok(!hostile.steps[1].command.includes("$("));

// A value that is neither SSH nor HTTPS gets the generic steps.
assert.deepEqual(signInHelp("not a remote", "linux"), {
  title: "Git could not sign in to the remote.",
  steps: [{ text: "Set up a credential helper or an SSH key for this remote, then try again.", href: "/docs/git.html#credentials" }],
});

console.log("gitsync sign-in help ok");
