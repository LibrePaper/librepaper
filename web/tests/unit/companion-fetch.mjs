import assert from "node:assert/strict";
import * as local from "../../src/lib/companion/client.js";
import { formOf } from "../helpers/form-body.mjs";

const BASE = "lp-local://localhost/";

const response = (body, status = 200, contentType = "application/json") => ({
  status,
  ok: status >= 200 && status < 300,
  headers: { get: (name) => name.toLowerCase() === "content-type" ? contentType : null },
  json: async () => body,
  text: async () => JSON.stringify(body),
  clone() { return response(body, status, contentType); },
});

function setup(fetch, { inDesktop = true } = {}) {
  if (inDesktop) globalThis.window = { __LIBREPAPER_DESKTOP__: Object.freeze({ base: BASE, version: "test" }) };
  else delete globalThis.window;
  local._testing.reset();
  local._testing.inject({ fetch });
  local.configure({ project: "paper", origin: "https://papers.example" });
}

const health = (protocol = [2]) => response({ service: "librepaper-local", protocol, version: "0.0.42", instance: "companion" });

// desktop() reads the page global and is null in an ordinary browser.
{
  delete globalThis.window;
  assert.equal(local.desktop(), null);
  globalThis.window = { __LIBREPAPER_DESKTOP__: { base: BASE, version: "test" } };
  assert.deepEqual(local.desktop(), { base: BASE, version: "test" });
  delete globalThis.window;
}

// Outside the desktop app the state is browser and a probe stays there.
{
  setup(async () => { throw new Error("no request may be sent"); }, { inDesktop: false });
  assert.equal(local.status().state, "browser");
  assert.equal((await local.probe()).state, "browser");
  assert.equal(local.canManage(), false);
  await assert.rejects(local.settings(), (error) => error.name === "Unreachable");
}

// Inside the desktop app the status starts as checking and a probe makes it ready.
{
  const requests = [];
  setup(async (url, init = {}) => {
    requests.push({ url, init });
    if (url === `${BASE}librepaper/local/health`) return health();
    if (url === `${BASE}librepaper/local/capabilities`) return response({ manage: true, builders: {} });
    if (url === `${BASE}companion/api/settings`) return response({ setting: true });
    if (url === `${BASE}librepaper/local/jobs`) return response({ id: "job-1" });
    if (url === `${BASE}librepaper/local/jobs/job-1`) return response({ status: "done", exit: 0, outputs: {} });
    throw new Error(`Unexpected request: ${url}`);
  });
  assert.equal(local.status().state, "checking");
  const ready = await local.probe();
  assert.equal(ready.state, "ready");
  assert.equal(ready.version, "0.0.42");
  assert.equal(local.canManage(), true);
  assert.deepEqual(await local.manage("/settings"), { setting: true });
  const built = await local.runBuild({ tree: { main: "paper.qmd", texts: { "paper.qmd": "# Paper" } }, builder: "quarto" });
  assert.equal(built.ok, false, "the stub omits Quarto outputs after exercising job submission and polling");
  const jobPost = requests.find(({ url, init }) => url.endsWith("/librepaper/local/jobs") && init.method === "POST");
  assert.ok(jobPost, "job submission sends a POST");
  assert.match(String(jobPost.init.headers?.["Content-Type"] || ""), /^multipart\/form-data; boundary=/, "job submission preserves its multipart body");
  assert.notEqual((await formOf(jobPost.init)).get("job"), null, "job submission preserves its multipart body");
  assert.ok(requests.every(({ init }) => !init.headers || !("Authorization" in init.headers)), "the page sends no credential");
}

// A 502 from the shell, or a network error, is unavailable.
{
  setup(async () => response({ error: "companion unavailable" }, 502));
  assert.equal((await local.probe()).state, "unavailable");
  setup(async () => { throw new TypeError("Failed to fetch"); });
  assert.equal((await local.probe()).state, "unavailable");
  setup(async (url) => url.endsWith("/health") ? health() : response({ error: "companion unavailable" }, 502));
  assert.equal((await local.probe()).state, "unavailable", "a 502 on capabilities is unavailable too");
}

// A protocol that excludes this client's version is incompatible.
{
  setup(async () => health([1]));
  assert.equal((await local.probe()).state, "incompatible");
}

// A network failure or 502 during a job is Unreachable and is not replayed.
{
  for (const failure of [() => { throw new TypeError("Failed to fetch"); }, () => response({}, 502)]) {
    let posts = 0;
    setup(async (url, init = {}) => {
      if (url.endsWith("/librepaper/local/jobs") && init.method === "POST") {
        posts += 1;
        return failure();
      }
      throw new Error(`Unexpected request: ${url}`);
    });
    await assert.rejects(
      local.runBuild({ tree: { main: "paper.qmd", texts: { "paper.qmd": "# Paper" } }, builder: "quarto" }),
      (error) => error.name === "Unreachable",
    );
    assert.equal(posts, 1, "a failed job request is not replayed");
  }
}

// A 403 from a management route says it is not available for this site.
{
  setup(async () => response({ error: "forbidden" }, 403));
  await assert.rejects(local.manage("/state"), (error) => error.name === "Refused" && error.status === 403);
}

local._testing.reset();
delete globalThis.window;
console.log("companion-fetch: the desktop base, no credential, and the browser, ready, unavailable and incompatible states");
