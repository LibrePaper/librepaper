// How the panel names a remote.

import assert from "node:assert/strict";

import { remoteIdentity } from "../../src/lib/gitsync/identity.js";

// GitHub over https, with and without the .git suffix.
assert.deepEqual(remoteIdentity("https://github.com/owner/repo.git", "network"), {
  name: "owner/repo",
  href: "https://github.com/owner/repo",
  full: "https://github.com/owner/repo.git",
});
assert.deepEqual(remoteIdentity("https://github.com/owner/repo", "network"), {
  name: "owner/repo",
  href: "https://github.com/owner/repo",
  full: "https://github.com/owner/repo",
});

// GitHub in scp style, as the connection label writes it.
assert.deepEqual(remoteIdentity("github.com:owner/repo.git", "network"), {
  name: "owner/repo",
  href: "https://github.com/owner/repo",
  full: "github.com:owner/repo.git",
});

// GitLab over ssh with a subgroup: the name keeps every group.
assert.deepEqual(remoteIdentity("ssh://git@gitlab.com/group/subgroup/repo.git", "network"), {
  name: "group/subgroup/repo",
  href: "https://gitlab.com/group/subgroup/repo",
  full: "ssh://git@gitlab.com/group/subgroup/repo.git",
});

// A host that is not a known forge has a name and no link.
assert.deepEqual(remoteIdentity("https://git.example.org/team/paper.git", "network"), {
  name: "team/paper",
  href: null,
  full: "https://git.example.org/team/paper.git",
});

// A local folder is named by its label and never linked.
assert.deepEqual(remoteIdentity("shared-papers", "local"), {
  name: "shared-papers",
  href: null,
  full: "shared-papers",
});
assert.deepEqual(remoteIdentity("", "local"), {
  name: "Local repository",
  href: null,
  full: "",
});

// An empty network label names nothing and links nowhere.
assert.deepEqual(remoteIdentity("", "network"), {
  name: "",
  href: null,
  full: "",
});

console.log("gitsync identity ok");
