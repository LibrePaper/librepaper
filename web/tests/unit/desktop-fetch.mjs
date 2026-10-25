import assert from "node:assert/strict";
import * as local from "../../src/lib/desktop/client.js";
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

// desktop() reads the page global and is null in an ordinary browser.
{
  delete globalThis.window;
  assert.equal(local.desktop(), null);
  globalThis.window = { __LIBREPAPER_DESKTOP__: { base: BASE, version: "test" } };
  assert.deepEqual(local.desktop(), { base: BASE, version: "test" });
  delete globalThis.window;
}

// Outside the desktop app a feature call says it needs the desktop app and sends nothing.
{
  setup(async () => { throw new Error("no request may be sent"); }, { inDesktop: false });
  const desktopOnly = (error) => error.name === "DesktopOnly" && error.message === local.DESKTOP_ONLY_MESSAGE;
  await assert.rejects(local.capabilities(), desktopOnly);
  await assert.rejects(local.agents(), desktopOnly);
  await assert.rejects(local.manage("/state"), desktopOnly);
  await assert.rejects(local.runRender({ tree: { main: "paper.qmd", texts: { "paper.qmd": "# Paper" } }, format: "quarto", output: "html" }), desktopOnly);
  assert.equal(local.DESKTOP_ONLY_MESSAGE, "This needs the LibrePaper desktop app.");
}

// Inside the desktop app the base is used, no credential is sent, and capabilities are fetched once.
{
  const requests = [];
  setup(async (url, init = {}) => {
    requests.push({ url, init });
    if (url === `${BASE}librepaper/local/capabilities`) return response({ renderers: { "quarto.html": { found: true } } });
    if (url === `${BASE}librepaper/local/capabilities/rescan`) return response({ renderers: { "quarto.html": { found: false } } });
    if (url === `${BASE}companion/api/settings`) return response({ setting: true });
    if (url === `${BASE}librepaper/local/jobs`) return response({ id: "job-1" });
    if (url === `${BASE}librepaper/local/jobs/job-1`) return response({ status: "done", exit: 0, outputs: {} });
    throw new Error(`Unexpected request: ${url}`);
  });
  assert.equal((await local.capabilities()).renderers["quarto.html"].found, true);
  assert.equal(requests.filter(({ url }) => url.endsWith("/capabilities")).length, 1, "capabilities are fetched once per page load");
  assert.equal((await local.capabilities({ rescan: true })).renderers["quarto.html"].found, false, "a rescan replaces the answer");
  assert.deepEqual(await local.manage("/settings"), { setting: true });
  const built = await local.runRender({ tree: { main: "paper.qmd", texts: { "paper.qmd": "# Paper" } }, format: "quarto", output: "html" });
  assert.equal(built.ok, false, "the stub omits the result file after exercising job submission and polling");
  const jobPost = requests.find(({ url, init }) => url.endsWith("/librepaper/local/jobs") && init.method === "POST");
  assert.ok(jobPost, "job submission sends a POST");
  assert.match(String(jobPost.init.headers?.["Content-Type"] || ""), /^multipart\/form-data; boundary=/, "job submission preserves its multipart body");
  assert.notEqual((await formOf(jobPost.init)).get("job"), null, "job submission preserves its multipart body");
  assert.ok(requests.every(({ init }) => !init.headers || !("Authorization" in init.headers)), "the page sends no credential");
}

// A 502 from the shell, or a network error, is Unreachable.
{
  const unreachable = (error) => error.name === "Unreachable" && /could not reach its local service/.test(error.message);
  setup(async () => response({ error: "service unavailable" }, 502));
  await assert.rejects(local.capabilities(), unreachable);
  await assert.rejects(local.manage("/state"), unreachable);
  setup(async () => { throw new TypeError("Failed to fetch"); });
  await assert.rejects(local.agents(), unreachable);
}

// A 502 that names a reason is the local service answering a failed remote request: Refused, with its body.
{
  setup(async () => response({ reason: "auth", detail: "Permission denied", error: "Permission denied" }, 502));
  await assert.rejects(local.capabilities(), (error) => error.name === "Refused" && error.status === 502 && error.body.reason === "auth");
  setup(async () => response({ error: "service unavailable" }, 502));
  await assert.rejects(local.capabilities(), (error) => error.name === "Unreachable");
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
      local.runRender({ tree: { main: "paper.qmd", texts: { "paper.qmd": "# Paper" } }, format: "quarto", output: "html" }),
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
console.log("desktop-fetch: the desktop base, no credential, DesktopOnly in a browser, and Unreachable on a 502");
