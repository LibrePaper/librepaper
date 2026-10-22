import assert from "node:assert/strict";
import test from "node:test";
import { gitHub, MovedError, AuthError } from "../../src/lib/forge/github.js";
import { startFakeGitHub } from "../helpers/fake-github.mjs";

const REPO = "ada/paper";
const binary = new Uint8Array([0, 1, 2, 255, 254]);

// Values computed with `git hash-object` on the same bytes.
const QMD = "e5c5c5583f49a34e86ce622b59363df99e09d4c6";
const CRLF = "422c2b7ab3b3c668038da977e4e93a5fc623169c";
const BIN = "bfa7018e30c510999bbe462316bdd3839a58aa56";
const X = "c1b0730e0133447badcfd47fd144e254807b06e1";

async function fixture() {
  const forge = await startFakeGitHub();
  forge.seed(REPO, {
    files: { "paper.qmd": "line one\nline two\n", "crlf.txt": "a\r\nb\r\n", "img/fig.bin": binary },
    message: "Start",
  });
  const client = gitHub({ token: "fake-token", api: forge.api });
  return { forge, client };
}

test("blob ids match git hash-object", async () => {
  const { forge, client } = await fixture();
  try {
    const head = forge.headOf(REPO, "main");
    const { entries } = await client.treeOf(REPO, head);
    const byPath = Object.fromEntries(entries.map((entry) => [entry.path, entry]));
    assert.equal(byPath["paper.qmd"].sha, QMD);
    assert.equal(byPath["crlf.txt"].sha, CRLF);
    assert.equal(byPath["img/fig.bin"].sha, BIN);
    assert.equal(byPath.img.type, "tree");
    assert.equal(await client.createBlob(REPO, new TextEncoder().encode("x")), X);
  } finally {
    await forge.stop();
  }
});

test("the client reads repositories, head, tree and blobs", async () => {
  const { forge, client } = await fixture();
  try {
    const repos = await client.repositories();
    assert.deepEqual(repos, [
      { id: forge.repo(REPO).id, name: REPO, defaultBranch: "main", private: true, installation: 1 },
    ]);
    assert.deepEqual(await client.branches(REPO), ["main"]);
    const head = await client.head(REPO, "main");
    assert.equal(head, forge.headOf(REPO, "main"));
    const { entries, truncated } = await client.treeOf(REPO, head);
    assert.equal(truncated, false);
    assert.deepEqual(entries.filter((e) => e.type === "blob").map((e) => e.path).sort(), ["crlf.txt", "img/fig.bin", "paper.qmd"]);
    assert.deepEqual([...(await client.blob(REPO, BIN))], [...binary]);
    assert.equal(new TextDecoder().decode(await client.blob(REPO, CRLF)), "a\r\nb\r\n");
    assert.equal(await client.textFile(REPO, "main", "paper.qmd"), "line one\nline two\n");
    assert.equal(await client.textFile(REPO, "main", "absent.txt"), null);
    assert.equal(await client.head(REPO, "main").then(() => "ok"), "ok");
    await assert.rejects(client.head(REPO, "nope"), (error) => error.status === 404);
    forge.truncateNextTree();
    assert.equal((await client.treeOf(REPO, head)).truncated, true);
    assert.equal((await client.treeOf(REPO, head)).truncated, false);
  } finally {
    await forge.stop();
  }
});

test("a push creates objects and moves the branch; a stale parent is MovedError", async () => {
  const { forge, client } = await fixture();
  try {
    const head = await client.head(REPO, "main");
    const { tree } = await client.treeOf(REPO, head);
    const blob = await client.createBlob(REPO, new TextEncoder().encode("new text\n"));
    const nested = await client.createTree(REPO, tree, [
      { path: "notes/deep/a.txt", sha: blob },
      { path: "crlf.txt", sha: null },
    ]);
    const first = await client.createCommit(REPO, { message: "One", tree: nested, parents: [head] });
    await client.moveBranch(REPO, "main", first);
    assert.equal(forge.headOf(REPO, "main"), first);
    assert.deepEqual(forge.list(REPO, "main"), ["img/fig.bin", "notes/deep/a.txt", "paper.qmd"]);
    assert.equal(forge.read(REPO, "main", "notes/deep/a.txt").toString(), "new text\n");
    assert.equal(forge.read(REPO, "main", "crlf.txt"), null);

    const stale = await client.createCommit(REPO, { message: "Two", tree: nested, parents: [head] });
    await assert.rejects(client.moveBranch(REPO, "main", stale), MovedError);
    assert.equal(forge.headOf(REPO, "main"), first);

    await assert.rejects(
      client.createTree(REPO, tree, [{ path: "x", sha: "0".repeat(40) }]),
      (error) => error.status === 422,
    );
    await client.createBranch(REPO, "feature/x", first);
    await assert.rejects(client.createBranch(REPO, "feature/x", first), (error) => error.status === 422);
    assert.equal(await client.head(REPO, "feature/x"), first);
  } finally {
    await forge.stop();
  }
});

test("compare reports ancestry", async () => {
  const { forge, client } = await fixture();
  try {
    const base = forge.headOf(REPO, "main");
    await client.createBranch(REPO, "side", base);
    assert.equal(await client.compare(REPO, "main", "side"), "identical");
    forge.commit(REPO, "side", { files: { "s.txt": "s" } });
    assert.equal(await client.compare(REPO, "main", "side"), "ahead");
    assert.equal(await client.compare(REPO, "side", "main"), "behind");
    forge.commit(REPO, "main", { files: { "m.txt": "m" } });
    assert.equal(await client.compare(REPO, "main", "side"), "diverged");
    const merged = forge.merge(REPO, { base: "main", head: "side", files: { "paper.qmd": "resolved\n" } });
    assert.equal(forge.headOf(REPO, "main"), merged);
    assert.equal(await client.compare(REPO, merged, "side"), "behind");
    assert.equal(forge.read(REPO, "main", "paper.qmd").toString(), "resolved\n");
    assert.equal(forge.read(REPO, "main", "s.txt"), null);
  } finally {
    await forge.stop();
  }
});

test("auth, OAuth, CORS and the request log", async () => {
  const { forge } = await fixture();
  try {
    assert.equal((await fetch(`${forge.api}/user/installations`)).status, 401);
    await assert.rejects(gitHub({ token: "wrong", api: forge.api }).repositories(), AuthError);

    const preflight = await fetch(`${forge.api}/repos/${REPO}/branches`, { method: "OPTIONS" });
    assert.equal(preflight.status, 204);
    assert.equal(preflight.headers.get("access-control-allow-origin"), "*");
    assert.match(preflight.headers.get("access-control-allow-headers"), /Authorization/);

    const redirect = await fetch(`${forge.web}/login/oauth/authorize?client_id=c&state=s1&redirect_uri=${encodeURIComponent("http://localhost:1/cb")}`, { redirect: "manual" });
    assert.equal(redirect.status, 302);
    assert.equal(redirect.headers.get("location"), "http://localhost:1/cb?code=fake-code&state=s1");

    const exchange = await fetch(`${forge.web}/login/oauth/access_token`, {
      method: "POST",
      headers: { "Content-Type": "application/json", Accept: "application/json" },
      body: JSON.stringify({ code: "fake-code" }),
    });
    const tokens = await exchange.json();
    assert.equal(tokens.access_token, "fake-token");
    assert.equal(tokens.refresh_token, "fake-refresh");
    const refreshed = await fetch(`${forge.web}/login/oauth/access_token`, {
      method: "POST",
      headers: { "Content-Type": "application/x-www-form-urlencoded" },
      body: "grant_type=refresh_token&refresh_token=fake-refresh",
    });
    assert.equal((await refreshed.json()).access_token, "fake-token");
    const bad = await fetch(`${forge.web}/login/oauth/access_token`, { method: "POST", body: "code=nope" });
    assert.equal(bad.status, 200);
    assert.deepEqual(await bad.json(), { error: "bad_verification_code" });

    assert.ok(forge.requests.some((r) => r.method === "GET" && r.path === "/user/installations" && r.authorization === null));
  } finally {
    await forge.stop();
  }
});

test("stop closes the port", async () => {
  const forge = await startFakeGitHub();
  const url = forge.url;
  await forge.stop();
  await assert.rejects(fetch(`${url}/user/installations`));
});
