// Which git addresses the page will send to the desktop app: https, ssh and
// scp-like, with no password in them.
import assert from "node:assert/strict";
import { remoteUrlProblem } from "../../src/lib/git-url.js";

for (const good of [
  "https://github.com/you/paper.git",
  "  https://gitlab.com/group/sub/paper.git  ",
  "https://token@github.com/you/paper.git",
  "ssh://git@github.com/you/paper.git",
  "ssh://git@example.org:2222/you/paper.git",
  "git@github.com:you/paper.git",
  "deploy.user@git.example.org:srv/paper",
]) {
  assert.equal(remoteUrlProblem(good), "", `${good} is an address`);
}

// A password is refused before anything is sent.
assert.match(remoteUrlProblem("https://you:secret@github.com/you/paper.git"), /password/);
assert.match(remoteUrlProblem("ssh://git:secret@example.org/paper.git"), /password/);

// So is everything that is not one of the three forms.
assert.match(remoteUrlProblem(""), /Enter an address/);
assert.match(remoteUrlProblem("   "), /Enter an address/);
assert.match(remoteUrlProblem("http://example.org/paper.git"), /https:\/\/ or ssh:\/\//);
assert.match(remoteUrlProblem("git://example.org/paper.git"), /https:\/\/ or ssh:\/\//);
assert.match(remoteUrlProblem("file:///home/you/paper"), /https:\/\/ or ssh:\/\//);
assert.match(remoteUrlProblem("/home/you/paper"), /https:\/\/ or ssh:\/\//);
assert.match(remoteUrlProblem("github.com:you/paper.git"), /https:\/\/ or ssh:\/\//);
assert.match(remoteUrlProblem("git@github.com"), /user@host:path/);
assert.match(remoteUrlProblem("git@:paper"), /user@host:path/);
assert.match(remoteUrlProblem("git@github.com:"), /user@host:path/);
assert.match(remoteUrlProblem("-oProxyCommand=x"), /dash/);
assert.match(remoteUrlProblem("https://example.org/a b"), /space/);
assert.match(remoteUrlProblem("https://"), /valid address/);
assert.match(remoteUrlProblem(`https://example.org/${"a".repeat(2100)}`), /too long/);

console.log("git-url: the forms accepted, and a password or anything else refused");
