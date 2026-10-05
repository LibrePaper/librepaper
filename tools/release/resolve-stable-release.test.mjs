import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath, pathToFileURL } from "node:url";

const repository = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");
const resolver = path.join(repository, "tools/release/resolve-stable-release.sh");

function git(directory, ...args) {
  return execFileSync("git", args, {
    cwd: directory,
    encoding: "utf8",
    stdio: ["ignore", "pipe", "pipe"],
  }).trim();
}

function fixture(t) {
  const root = mkdtempSync(path.join(os.tmpdir(), "librepaper-release-resolution-"));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const source = path.join(root, "source");
  const remote = path.join(root, "origin.git");
  const checkout = path.join(root, "workflow-checkout");
  mkdirSync(source);
  git(source, "init", "--initial-branch=main");
  git(source, "config", "user.name", "Fixture");
  git(source, "config", "user.email", "fixture@example.invalid");

  writeFileSync(path.join(source, "release.txt"), "old release\n");
  git(source, "add", "release.txt");
  git(source, "commit", "-m", "old release");
  const releasedSha = git(source, "rev-parse", "HEAD");
  git(source, "tag", "v1.2.3");

  writeFileSync(path.join(source, "release.txt"), "trusted updater revision\n");
  git(source, "commit", "-am", "trusted workflow revision");
  const workflowSha = git(source, "rev-parse", "HEAD");
  git(source, "tag", "v1.3.0-rc.1");
  git(root, "clone", "--bare", source, remote);
  git(root, "clone", "--no-tags", "--depth", "1", pathToFileURL(remote).href, checkout);

  return { checkout, releasedSha, root, workflowSha };
}

function resolve(fixtureData, env) {
  const output = path.join(fixtureData.root, "github-output");
  writeFileSync(output, "");
  execFileSync("bash", [resolver], {
    cwd: fixtureData.checkout,
    env: {
      ...process.env,
      GITHUB_OUTPUT: output,
      DISPATCH_TAG: "",
      RELEASE_SHA: "",
      RUN_HEAD_BRANCH: "",
      ...env,
    },
    stdio: "pipe",
  });
  return readFileSync(output, "utf8");
}

test("stable dispatch fetches the tag without moving the trusted checkout", (t) => {
  const data = fixture(t);
  assert.notEqual(data.releasedSha, data.workflowSha);
  assert.equal(git(data.checkout, "rev-parse", "--is-shallow-repository"), "true");
  assert.equal(git(data.checkout, "rev-parse", "HEAD"), data.workflowSha);
  assert.throws(() => git(data.checkout, "cat-file", "-e", `${data.releasedSha}^{commit}`));

  assert.equal(resolve(data, { DISPATCH_TAG: "v1.2.3" }), `tag=v1.2.3\nsha=${data.releasedSha}\nproceed=true\n`);
  assert.equal(git(data.checkout, "rev-parse", "HEAD"), data.workflowSha);
  assert.equal(git(data.checkout, "rev-parse", "refs/tags/v1.2.3^{commit}"), data.releasedSha);
  assert.doesNotThrow(() => git(data.checkout, "cat-file", "-e", `${data.releasedSha}^{commit}`));
});

test("workflow-run resolution rejects a stable tag on a different commit", (t) => {
  const data = fixture(t);
  assert.equal(
    resolve(data, { RELEASE_SHA: data.workflowSha, RUN_HEAD_BRANCH: "v1.2.3" }),
    "proceed=false\n",
  );
});

test("successful release workflow resolves the immutable tagged commit SHA", (t) => {
  const data = fixture(t);
  assert.equal(
    resolve(data, { RELEASE_SHA: data.releasedSha, RUN_HEAD_BRANCH: "v1.2.3" }),
    `tag=v1.2.3\nsha=${data.releasedSha}\nproceed=true\n`,
  );
  assert.equal(git(data.checkout, "rev-parse", "HEAD"), data.workflowSha);
});

test("missing stable and prerelease tags are skipped", (t) => {
  const data = fixture(t);
  assert.equal(resolve(data, { DISPATCH_TAG: "v9.9.9" }), "proceed=false\n");
  assert.equal(resolve(data, { DISPATCH_TAG: "v1.3.0-rc.1" }), "proceed=false\n");
});
