// The GitHub client against a fake fetch: the token travels in a header and
// never in a URL, errors map to their classes, blobs decode, lists page.
import assert from "node:assert/strict";
import { AuthError, GitHubError, MovedError, gitHub } from "../../src/lib/forge/github.js";

const TOKEN = "ghu_secret";

function fakeFetch(handler) {
  const calls = [];
  const fetch = async (url, init = {}) => {
    calls.push({ url: String(url), init });
    const answer = await handler(String(url), init);
    return {
      ok: answer.status >= 200 && answer.status < 300,
      status: answer.status,
      json: async () => answer.body,
    };
  };
  return { fetch, calls };
}

// Headers on every call, and the token only in the Authorization header.
{
  const { fetch, calls } = fakeFetch(() => ({ status: 200, body: { object: { sha: "abc" } } }));
  const github = gitHub({ token: TOKEN, fetch });
  assert.equal(await github.head("o/r", "feature/x"), "abc");
  assert.equal(calls[0].url, "https://api.github.com/repos/o/r/git/ref/heads/feature/x");
  assert.equal(calls[0].init.headers.Authorization, `Bearer ${TOKEN}`);
  assert.equal(calls[0].init.headers.Accept, "application/vnd.github+json");
  assert.equal(calls[0].init.headers["X-GitHub-Api-Version"], "2022-11-28");
  assert.ok(!calls[0].url.includes(TOKEN));
}

// 422 on a branch move is MovedError; a body carries force: false.
{
  const { fetch, calls } = fakeFetch(() => ({ status: 422, body: { message: "not a fast forward" } }));
  const github = gitHub({ token: TOKEN, fetch });
  await assert.rejects(() => github.moveBranch("o/r", "main", "sha1"), MovedError);
  assert.equal(calls[0].init.method, "PATCH");
  assert.deepEqual(JSON.parse(calls[0].init.body), { sha: "sha1", force: false });
}

// 401 is AuthError, other failures GitHubError with the status.
{
  const github = gitHub({ token: TOKEN, fetch: fakeFetch(() => ({ status: 401, body: {} })).fetch });
  await assert.rejects(() => github.branches("o/r"), AuthError);
  const other = gitHub({ token: TOKEN, fetch: fakeFetch(() => ({ status: 500, body: { message: "boom" } })).fetch });
  await assert.rejects(() => other.head("o/r", "main"), (error) =>
    error instanceof GitHubError && error.status === 500 && error.message === "boom");
}

// A blob's base64 has line breaks and decodes to the original bytes.
{
  const bytes = Uint8Array.from({ length: 200 }, (_, i) => i);
  const encoded = btoa(String.fromCharCode(...bytes)).replace(/(.{60})/g, "$1\n");
  const github = gitHub({ token: TOKEN, fetch: fakeFetch(() => ({ status: 200, body: { content: encoded, encoding: "base64" } })).fetch });
  assert.deepEqual([...(await github.blob("o/r", "deadbeef"))], [...bytes]);
}

// textFile decodes UTF-8 and answers null for a missing file.
{
  const text = "café\n";
  const content = btoa(String.fromCharCode(...new TextEncoder().encode(text)));
  const { fetch, calls } = fakeFetch((url) =>
    url.includes("missing") ? { status: 404, body: { message: "Not Found" } } : { status: 200, body: { content } });
  const github = gitHub({ token: TOKEN, fetch });
  assert.equal(await github.textFile("o/r", "main", ".librepaperignore"), text);
  assert.equal(await github.textFile("o/r", "main", "missing"), null);
  assert.ok(calls[0].url.endsWith("/contents/.librepaperignore?ref=main"));
}

// Repositories page through installations and keep only those with push.
{
  const many = Array.from({ length: 100 }, (_, i) => ({ id: i, full_name: `o/r${i}`, default_branch: "main", permissions: { push: i % 2 === 0 } }));
  const { fetch, calls } = fakeFetch((url) => {
    if (url.includes("/user/installations?")) return { status: 200, body: { installations: [{ id: 7 }] } };
    if (url.includes("page=1")) return { status: 200, body: { repositories: many } };
    return { status: 200, body: { repositories: [{ id: 500, full_name: "o/last", default_branch: "dev", permissions: { push: true } }] } };
  });
  const repositories = await gitHub({ token: TOKEN, fetch }).repositories();
  assert.equal(repositories.length, 51);
  assert.deepEqual(repositories.at(-1), { id: 500, name: "o/last", defaultBranch: "dev", private: false, installation: 7 });
  assert.ok(calls.some((call) => call.url.includes("/user/installations/7/repositories?per_page=100&page=2")));
  assert.ok(calls.every((call) => !call.url.includes(TOKEN)));
}

// A tree deletion sends a null sha and the base tree.
{
  const { fetch, calls } = fakeFetch(() => ({ status: 201, body: { sha: "tree1" } }));
  const sha = await gitHub({ token: TOKEN, fetch }).createTree("o/r", "base", [{ path: "a.md", sha: null }]);
  assert.equal(sha, "tree1");
  assert.deepEqual(JSON.parse(calls[0].init.body), {
    base_tree: "base",
    tree: [{ path: "a.md", mode: "100644", type: "blob", sha: null }],
  });
}

console.log("forge-github ok");
