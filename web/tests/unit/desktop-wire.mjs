// The page's wire contract, checked against the fixtures in
// crates/librepaper-companion/wire. See SPEC-wire.md 4.4, 9.1, 9.2 and 9.4.
import assert from "node:assert/strict";
import { readFile, readdir } from "node:fs/promises";
import { join, relative } from "node:path";
import { afterEach, test } from "node:test";
import { fileURLToPath } from "node:url";
import * as client from "../../src/lib/desktop/client.js";
import { compareVersions, major, PAGE } from "../../src/lib/desktop/negotiate.js";
import { formOf } from "../helpers/form-body.mjs";

const ROOT = join(fileURLToPath(new URL(".", import.meta.url)), "..", "..", "..");
const WIRE_VERSION_DIR = join(ROOT, "crates", "librepaper-companion", "wire", `v${major(PAGE)}`);
const SRC = join(ROOT, "web", "src");
const PARAGLIDE = join(SRC, "lib", "paraglide");
const PROTOCOL_HEADER = "x-librepaper-protocol";

const BASE = "lp-local://localhost/";
const CURRENT = { base: BASE, version: "3.1.0", protocol: { oldest: 3, current: "3.1.0" } };
const LEGACY = { base: BASE, version: "3.0.0" };
const TREE = { main: "chapters/paper.tex", texts: { "chapters/paper.tex": "\\documentclass{article}" } };

const originalWindow = Object.getOwnPropertyDescriptor(globalThis, "window");

afterEach(() => {
  client._testing.reset();
  if (originalWindow) Object.defineProperty(globalThis, "window", originalWindow);
  else delete globalThis.window;
});

function response(status, body) {
  return {
    ok: status >= 200 && status < 300,
    status,
    json: async () => body,
    text: async () => JSON.stringify(body),
    arrayBuffer: async () => new TextEncoder().encode(JSON.stringify(body)).buffer,
    clone() { return response(status, body); },
  };
}

// Installs a desktop global and a fake fetch, with no real waiting between polls.
function useDesktop(shell, fetch, wait = () => Promise.resolve()) {
  globalThis.window = { __LIBREPAPER_DESKTOP__: shell };
  client._testing.reset();
  client._testing.inject({ fetch, wait });
  client.configure({ project: "paper", origin: "https://app.test" });
}

function headerNames(init) {
  const headers = init?.headers;
  if (headers instanceof Headers) return [...headers.keys()];
  return Object.keys(headers ?? {}).map((name) => name.toLowerCase());
}

const fixture = async (name) => JSON.parse(await readFile(join(WIRE_VERSION_DIR, name), "utf8"));
const kindOf = (value) => (value === null ? "null" : Array.isArray(value) ? "array" : typeof value);

// Every key of `shape` is present in `value` with the same JSON type, recursively.
// Arrays are checked element by element against the shape's first element.
function missingKeys(value, shape, path = "$", out = []) {
  if (kindOf(value) !== kindOf(shape)) {
    out.push(`${path}: expected ${kindOf(shape)}, got ${kindOf(value)}`);
    return out;
  }
  if (Array.isArray(shape)) {
    if (shape.length) value.forEach((item, i) => missingKeys(item, shape[0], `${path}[${i}]`, out));
  } else if (shape !== null && typeof shape === "object") {
    for (const key of Object.keys(shape)) {
      if (!(key in value)) out.push(`${path}.${key}: missing`);
      else missingKeys(value[key], shape[key], `${path}.${key}`, out);
    }
  }
  return out;
}

// No key of `value` falls outside `shape`, and every key present has the shape's JSON type.
// An array element is checked against the shape's first element; an array whose
// fixture shows no element may not carry any.
function extraKeys(value, shape, path = "$", out = []) {
  if (kindOf(value) !== kindOf(shape)) {
    out.push(`${path}: expected ${kindOf(shape)}, got ${kindOf(value)}`);
    return out;
  }
  if (Array.isArray(value)) {
    if (!shape.length) {
      if (value.length) out.push(`${path}: the fixture shows no element`);
    } else {
      value.forEach((item, i) => extraKeys(item, shape[0], `${path}[${i}]`, out));
    }
  } else if (value !== null && typeof value === "object") {
    for (const key of Object.keys(value)) {
      if (!(key in shape)) out.push(`${path}.${key}: not in the fixture`);
      else extraKeys(value[key], shape[key], `${path}.${key}`, out);
    }
  }
  return out;
}

async function sourceFiles(dir) {
  const files = [];
  for (const entry of await readdir(dir, { withFileTypes: true })) {
    const full = join(dir, entry.name);
    if (entry.isDirectory()) {
      if (full !== PARAGLIDE) files.push(...await sourceFiles(full));
    } else if (/\.(js|svelte)$/.test(entry.name)) {
      files.push(full);
    }
  }
  return files;
}

test("PAGE equals the VERSION file of its major", async () => {
  const version = (await readFile(join(WIRE_VERSION_DIR, "VERSION"), "utf8")).trim();
  assert.equal(PAGE, version);
});

for (const [label, shell] of [["with", CURRENT], ["without", LEGACY]]) {
  test(`a major ${major(PAGE)} page sends no protocol header, ${label} protocol in the global`, async () => {
    const sent = [];
    useDesktop(shell, async (url, init = {}) => {
      sent.push({ url, names: headerNames(init) });
      return response(200, {});
    });
    await client.send("GET", "health");
    await client.send("GET", "capabilities");
    await client.manage("/status");
    await client.manage("/settings", { method: "PUT", body: { enabled: true } });
    assert.equal(sent.length, 4);
    for (const call of sent) {
      assert.equal(call.names.includes(PROTOCOL_HEADER), false, `${call.url} sends ${PROTOCOL_HEADER}`);
    }
  });
}

test("a job body has every key of the minimal job fixture and none beyond the full one", async () => {
  const minimal = await fixture("jobs.post.request.minimal.json");
  const full = await fixture("jobs.post.request.full.json");
  let body;
  useDesktop(CURRENT, async (url, init = {}) => {
    if (url.endsWith("/jobs") && init.method === "POST") {
      const form = await formOf(init);
      body = JSON.parse(await form.get("job").text());
      return response(202, { id: "job-1" });
    }
    if (url.endsWith("/jobs/job-1")) return response(200, { id: "job-1", status: "done", exit: 0, outputs: {} });
    throw new Error(`unexpected request ${url}`);
  });
  await client.runRender({ job: { snapshot: "s", generation: 1, deadlineSeconds: 60 }, tree: TREE, format: "latex", output: "pdf" });
  assert.ok(body, "the job was not submitted");
  assert.deepEqual([...missingKeys(body, minimal), ...extraKeys(body, full)], []);
});

test("renderer bodies match their fixtures, with the set fixture as both minimum and maximum", async () => {
  const set = await fixture("renderers.put.request.set.json");
  const reset = await fixture("renderers.put.request.reset.json");
  const sent = [];
  useDesktop(CURRENT, async (url, init = {}) => {
    sent.push(JSON.parse(init.body));
    return response(200, { ok: true });
  });
  await client.setRenderer("typst.pdf", { command: ["typst", "compile", "{input}", "{stem}.pdf"], result: "{stem}.pdf" });
  await client.resetRenderer("typst.pdf");
  assert.equal(sent.length, 2);
  assert.deepEqual([...missingKeys(sent[0], set), ...extraKeys(sent[0], set)], []);
  assert.deepEqual([...missingKeys(sent[1], reset), ...extraKeys(sent[1], reset)], []);
});

test("a job status the page does not know keeps polling until done", async () => {
  const statuses = ["reticulating", "queued-by-a-later-app", "done"];
  let polls = 0;
  useDesktop(CURRENT, async (url, init = {}) => {
    if (url.endsWith("/jobs") && init.method === "POST") return response(202, { id: "job-1" });
    if (url.endsWith("/jobs/job-1")) {
      const status = statuses[Math.min(polls, statuses.length - 1)];
      polls += 1;
      return response(200, { id: "job-1", status, exit: 0, outputs: {} });
    }
    throw new Error(`unexpected request ${url}`);
  });
  const result = await client.runRender({ job: {}, tree: TREE, format: "latex", output: "pdf" });
  assert.equal(polls, 3, "the page stopped polling at an unknown status");
  assert.equal(result.exit, 0);
});

test("every since in the page source is of the page's major and not above PAGE", async () => {
  const SINCE = /\bsince:\s*"(\d+\.\d+\.\d+)"/g;
  const problems = [];
  for (const file of await sourceFiles(SRC)) {
    const text = await readFile(file, "utf8");
    for (const match of text.matchAll(SINCE)) {
      const version = match[1];
      const line = text.slice(0, match.index).split("\n").length;
      const where = `${relative(ROOT, file)}:${line}`;
      if (major(version) !== major(PAGE)) problems.push(`${where}: since "${version}" is not major ${major(PAGE)}`);
      if (compareVersions(version, PAGE) > 0) problems.push(`${where}: since "${version}" is above PAGE ${PAGE}`);
    }
  }
  assert.deepEqual(problems, []);
});
