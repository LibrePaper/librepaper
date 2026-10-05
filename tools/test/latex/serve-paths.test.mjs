import assert from "node:assert/strict";
import { mkdtemp, mkdir, readFile, rm, symlink, writeFile } from "node:fs/promises";
import { spawn } from "node:child_process";
import { createServer, get } from "node:http";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { fetchMirror, mirrorTarget, resolveLocalFile } from "./serve-paths.mjs";

const serveScript = fileURLToPath(new URL("./serve.mjs", import.meta.url));

async function startServe(mirror) {
  const child = spawn(process.execPath, [serveScript, "--port", "0", "--mirror", mirror]);
  let output = "";
  const address = await new Promise((resolve, reject) => {
    child.stdout.setEncoding("utf8").on("data", (chunk) => {
      output += chunk;
      const match = output.match(/http:\/\/([^/:]+):(\d+)\/mirror\//);
      if (match) resolve({ host: match[1], port: Number(match[2]) });
    });
    child.stderr.setEncoding("utf8").on("data", (chunk) => { output += chunk; });
    child.once("error", reject);
    child.once("exit", (code) => reject(new Error(`serve exited before listening (${code}): ${output}`)));
  });
  return {
    ...address,
    async close() {
      child.kill("SIGTERM");
      await new Promise((resolve) => child.once("exit", resolve));
    },
  };
}

function getPath(port, path) {
  return new Promise((resolve, reject) => {
    get({ hostname: "127.0.0.1", port, path }, (response) => {
      const chunks = [];
      response.on("data", (chunk) => chunks.push(chunk));
      response.on("end", () => resolve({ status: response.statusCode, body: Buffer.concat(chunks).toString("utf8") }));
    }).once("error", reject);
  });
}

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
    "https:%2f%2fevil.example/file", "a%3fb", "a%23b", "a%5cb", "%252e%252e%252fsecret", "%00"]) {
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

test("the running harness serves only real files inside its loopback mirror root", async () => {
  const parent = await mkdtemp(join(tmpdir(), "librepaper-latex-server-"));
  const root = join(parent, "mirror");
  const sibling = join(parent, "root-sibling");
  await mkdir(root);
  await mkdir(sibling);
  await writeFile(join(root, "inside.txt"), "inside");
  await writeFile(join(sibling, "secret.txt"), "outside");
  await symlink(join(sibling, "secret.txt"), join(root, "escape.txt"));
  const server = await startServe(root);
  try {
    assert.equal(server.host, "127.0.0.1");
    assert.deepEqual(await getPath(server.port, "/mirror/inside.txt"), { status: 200, body: "inside" });
    assert.equal((await getPath(server.port, "/mirror/escape.txt")).status, 404);
    assert.equal((await getPath(server.port, "/mirror/%2e%2e/root-sibling/secret.txt")).status, 404);
  } finally {
    await server.close();
    await rm(parent, { recursive: true, force: true });
  }
});

test("the running remote proxy rejects redirects without contacting their target", async () => {
  let redirectTargetRequests = 0;
  const target = createServer((_request, response) => {
    redirectTargetRequests += 1;
    response.end("private target");
  });
  const source = createServer((_request, response) => {
    response.writeHead(302, { location: `http://127.0.0.1:${target.address().port}/secret` });
    response.end();
  });
  for (const server of [target, source]) {
    server.listen(0, "127.0.0.1");
    await new Promise((resolve, reject) => {
      server.once("listening", resolve);
      server.once("error", reject);
    });
  }
  const harness = await startServe(`http://127.0.0.1:${source.address().port}/latex/`);
  try {
    assert.deepEqual(await getPath(harness.port, "/mirror/engine.js"), { status: 502, body: "mirror redirects are not followed" });
    assert.equal(redirectTargetRequests, 0);
  } finally {
    await harness.close();
    await Promise.all([source, target].map((server) => new Promise((resolve) => server.close(resolve))));
  }
});
