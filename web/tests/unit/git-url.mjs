// Which git addresses the page will send to the desktop app: https, ssh and
// scp-like, with no password in them.
import assert from "node:assert/strict";
import { remoteUrlProblem, repositoryInput } from "../../src/lib/git-url.js";

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

// The Git panel's one field: a remote address, or a folder on this computer.
const local = (value) => ({ kind: "local", value, problem: "" });
const remote = (value) => ({ kind: "remote", value, problem: "" });

// Folders on this computer are local, with no problem, spaces allowed.
assert.deepEqual(repositoryInput("C:\\Users\\me\\paper"), local("C:\\Users\\me\\paper"));
assert.deepEqual(repositoryInput("c:/Users/me/paper"), local("c:/Users/me/paper"));
assert.deepEqual(repositoryInput("\\\\server\\share\\paper"), local("\\\\server\\share\\paper"));
assert.deepEqual(repositoryInput("~/paper"), local("~/paper"));
assert.deepEqual(repositoryInput("/home/me/My Papers/paper"), local("/home/me/My Papers/paper"));
assert.deepEqual(repositoryInput("  /home/me/paper  "), local("/home/me/paper"));
assert.match(repositoryInput("/" + "a".repeat(4100)).problem, /too long/);
assert.equal(repositoryInput("/" + "a".repeat(4100)).kind, "local");
assert.match(repositoryInput("/home/me/\u0007paper").problem, /control character/);

// Addresses are remote, checked as remoteUrlProblem checks them.
assert.deepEqual(repositoryInput("git@github.com:me/x.git"), remote("git@github.com:me/x.git"));
assert.deepEqual(repositoryInput("https://github.com/me/x.git"), remote("https://github.com/me/x.git"));
assert.match(repositoryInput("https://you:secret@github.com/you/paper.git").problem, /password/);
assert.match(repositoryInput("https://example.org/a b").problem, /space/);

// Empty input asks for either form.
assert.equal(repositoryInput("   ").kind, "remote");
assert.equal(repositoryInput("   ").value, "");
assert.match(repositoryInput("   ").problem, /Enter a repository URL or a folder path/);

// A file:// address is refused: the folder is named by its plain path.
assert.equal(repositoryInput("file:///home/me/paper").kind, "local");
assert.match(repositoryInput("file:///home/me/paper").problem, /without file:\/\//);
assert.match(repositoryInput("FILE:///home/me/paper").problem, /without file:\/\//);

// A relative path is refused: the full path is needed.
for (const relative of ["./x", "../x", ".\\x", "..\\x", ".", ".."]) {
  assert.equal(repositoryInput(relative).kind, "local", relative);
  assert.match(repositoryInput(relative).problem, /full path to the folder/, relative);
}

// Anything else that is neither a path nor an address is unrecognized.
assert.deepEqual(repositoryInput("repo").kind, "remote");
assert.match(repositoryInput("repo").problem, /https:\/\/ or ssh:\/\//);
assert.match(repositoryInput("github.com/x/y").problem, /full path to a folder/);

console.log("git-url: the forms accepted, and a password or anything else refused");
