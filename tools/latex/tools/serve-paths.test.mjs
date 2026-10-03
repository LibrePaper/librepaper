import assert from "node:assert/strict";
import { mkdtemp, mkdir, readFile, rm, symlink, writeFile } from "node:fs/promises";
import { createServer } from "node:http";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { fetchMirror, mirrorTarget, resolveLocalFile } from "./serve-paths.mjs";

test("local file resolution confines decoded paths and symlinks to the selected root", async () => {
  const parent = await mkdtemp(join(tmpdir(), "librepaper-latex-serve-"));
  const root = join(parent, "root");
  const sibling = join(parent, "root-sibling");
  await mkdir(root);
  await mkdir(sibling);
  await writeFile(join(root, "ok.txt"), "inside");
  await writeFile(join(sibling, "secret.txt"), "outside");
  await symlink(join(sibling, "secret.txt"), join(root, "escape.txt"));
  try {
    const path = resolveLocalFile(root, "ok.txt");
    assert.ok(path);
    assert.equal(await readFile(path, "utf8"), "inside");
    assert.equal(resolveLocalFile(root, "../root-sibling/secret.txt"), null);
    assert.equal(resolveLocalFile(root, "%2e%2e/root-sibling/secret.txt"), null);
    assert.equal(resolveLocalFile(root, "escape.txt"), null);
    assert.equal(resolveLocalFile(root, "%E0%A4%A"), null);
  } finally {
    await rm(parent, { recursive: true, force: true });
  }
});

test("remote mirror paths cannot change origin, escape base path, or use URL delimiters", () => {
  const base = "https://mirror.example/latex/release/";
  assert.equal(mirrorTarget(base, "engine.js").href, `${base}engine.js`);
  for (const suffix of ["../secret", "%2e%2e/secret", "%2f%2fevil.example/file", "//evil.example/file",
    "https:%2f%2fevil.example/file", "a%3fb", "a%23b", "a%5cb", "%00"]) {
    assert.equal(mirrorTarget(base, suffix), null, suffix);
  }
  assert.equal(mirrorTarget("https://user:pass@mirror.example/latex/", "engine.js"), null);
});

test("a remote mirror redirect is not followed to another origin", async () => {
  let redirectedRequests = 0;
  const destination = createServer((_request, response) => {
    redirectedRequests += 1;
    response.end("must not be fetched");
  });
  const source = createServer((_request, response) => {
    response.writeHead(302, { location: `http://127.0.0.1:${destination.address().port}/secret` });
    response.end();
  });
  destination.listen(0, "127.0.0.1");
  await new Promise((resolve, reject) => {
    destination.once("listening", resolve);
    destination.once("error", reject);
  });
  source.listen(0, "127.0.0.1");
  await new Promise((resolve, reject) => {
    source.once("listening", resolve);
    source.once("error", reject);
  });
  try {
    const target = mirrorTarget(`http://127.0.0.1:${source.address().port}/latex/`, "engine.js");
    assert.equal(await fetchMirror(target), null);
    assert.equal(redirectedRequests, 0);
  } finally {
    await Promise.all([source, destination].map((server) => new Promise((resolve, reject) => {
      server.close((error) => error ? reject(error) : resolve());
    })));
  }
});
