// The desktop client, against a fake desktop app.
//
// The desktop client never touches `fetch` or `localStorage` directly --
// everything ambient goes through its `_testing.inject`-able `deps` -- so this
// check drives the exact production module under Node with a small fake HTTP
// service and an in-memory store.

import { createHash } from "node:crypto";

import * as local from "../../src/lib/desktop/client.js";
import { formOf } from "../helpers/form-body.mjs";

// The client verifies every build output against the digest the job status
// announced, so the fake service has to announce real ones.
async function sha256hex(bytes) {
  return createHash("sha256").update(Buffer.from(bytes)).digest("hex");
}

let failures = 0;
function check(what, condition, detail = "") {
  if (condition) return;
  failures += 1;
  console.error(`latex-local: FAIL ${what}${detail ? ` -- ${detail}` : ""}`);
}
async function rejects(promise, name, what) {
  try {
    await promise;
    check(what, false, "resolved instead of rejecting");
    return null;
  } catch (error) {
    check(what, error?.name === name, `got name ${error?.name}, wanted ${name} (${error?.message})`);
    return error;
  }
}

/* --------------------------------------------------------- fakes */

function fakeStorage() {
  const data = new Map();
  return {
    getItem: (k) => (data.has(k) ? data.get(k) : null),
    setItem: (k, v) => data.set(k, String(v)),
    removeItem: (k) => data.delete(k),
    _data: data,
  };
}

function clock(start = 1_700_000_000_000) {
  let t = start;
  return { now: () => t, advance: (ms) => { t += ms; }, set: (v) => { t = v; } };
}

function jsonResponse(status, body, ok = status >= 200 && status < 300) {
  return {
    ok, status,
    json: async () => body,
    clone() { return jsonResponse(status, body, ok); },
    arrayBuffer: async () => new TextEncoder().encode(JSON.stringify(body)).buffer,
  };
}

function bytesResponse(status, bytes) {
  return {
    ok: status >= 200 && status < 300, status,
    json: async () => { throw new Error("not json"); },
    clone() { return bytesResponse(status, bytes); },
    arrayBuffer: async () => (bytes.buffer ? bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength) : bytes),
  };
}


function setup({ storage = fakeStorage(), now = clock(), fetchImpl } = {}) {
  globalThis.window = { __LIBREPAPER_DESKTOP__: { base: "lp-local://localhost/", version: "test" } };
  local._testing.reset();
  local._testing.inject({ storage, now: now.now, fetch: fetchImpl, wait: (ms) => Promise.resolve() });
  return { storage, now };
}

/* --------------------------------------------------------- multipart bodies */

async function testMultipartBuildBody() {
  const storage = fakeStorage();
  const nonUtf8 = new Uint8Array([0xff, 0xfe, 0x00, 0x80, 0x81, 0x41, 0x42]);
  const pdf = new TextEncoder().encode("%PDF-fake");
  const log = new TextEncoder().encode("typst log");
  let capturedInit = null;
  setup({ storage, fetchImpl: async (url, init) => {
    if (url.endsWith("/jobs") && init.method === "POST") { capturedInit = init; return jsonResponse(202, { id: "job1", status: "queued" }); }
    if (url.endsWith("/jobs/job1")) return jsonResponse(200, { id: "job1", status: "done", exit: 0, outputs: {
      artifact: { size: pdf.byteLength, sha256: await sha256hex(pdf) },
      log: { size: log.byteLength, sha256: await sha256hex(log) },
    } });
    if (url.endsWith("/files/artifact")) return bytesResponse(200, pdf);
    if (url.endsWith("/files/log")) return bytesResponse(200, log);
    throw new Error(`unexpected ${url}`);
  } });
  local.configure({ project: "proj1", origin: "https://app.example" });
  const result = await local.runRender({
    job: { snapshot: "snap1", generation: 1 },
    tree: {
      main: "main.typ",
      texts: { "main.typ": "= Title" },
      assets: { "assets/weird.bin": nonUtf8 },
    },
    format: "typst",
    output: "pdf",
  });
  check("runRender resolves ok on a clean done job", result.ok === true, JSON.stringify({ ok: result.ok, error: result.error }));
  check("the format is carried into provenance", result.provenance.format === "typst", JSON.stringify(result.provenance));
  check("the body is multipart", String(capturedInit?.headers?.["Content-Type"] || "").startsWith("multipart/form-data"));
  const capturedForm = await formOf(capturedInit);
  const jobPart = capturedForm.get("job");
  check("the job part is present", jobPart != null);
  const jobJson = JSON.parse(await jobPart.text());
  check("the job part parses to a protocol 3 render request", jobJson.protocol === 3 && jobJson.format === "typst" && jobJson.output === "pdf", JSON.stringify(jobJson));
  check("the entrypoint is the tree's main file", jobJson.entrypoint === "main.typ", jobJson.entrypoint);
  check("the hosted workspace is requested", jobJson.binding_id === "hosted", JSON.stringify(jobJson.binding_id));
  check("every input is named in the manifest", jobJson.manifest.some((m) => m.path === "main.typ") && jobJson.manifest.some((m) => m.path === "assets/weird.bin"), JSON.stringify(jobJson.manifest));
  const fileParts = capturedForm.getAll("file");
  check("every input became a file part", fileParts.length === 2, `got ${fileParts.length}`);
  const weirdPart = fileParts.find((f) => f.name === "assets/weird.bin");
  check("a nested path survives as the file part's name", weirdPart != null);
  const weirdBytes = new Uint8Array(await weirdPart.arrayBuffer());
  check(
    "non-UTF-8 bytes survive the multipart round trip exactly",
    weirdBytes.length === nonUtf8.length && weirdBytes.every((b, i) => b === nonUtf8[i]),
    `${[...weirdBytes]}`,
  );
}

// A build output that does not match the digest the job status announced is
// refused rather than handed to the reader as a page.
async function testOutputDigestIsVerified() {
  const storage = fakeStorage();
  const announced = new TextEncoder().encode("%PDF-real");
  const served = new TextEncoder().encode("%PDF-fake");
  setup({ storage, fetchImpl: async (url, init) => {
    if (url.endsWith("/jobs") && init.method === "POST") return jsonResponse(202, { id: "job2", status: "queued" });
    if (url.endsWith("/jobs/job2")) return jsonResponse(200, { id: "job2", status: "done", exit: 0, outputs: {
      artifact: { size: served.byteLength, sha256: await sha256hex(announced) },
    } });
    if (url.endsWith("/files/artifact")) return bytesResponse(200, served);
    throw new Error(`unexpected ${url}`);
  } });
  local.configure({ project: "proj1", origin: "https://app.example" });
  await rejects(
    local.runRender({ job: { snapshot: "s", generation: 1 }, tree: { main: "main.typ", texts: { "main.typ": "x" } }, format: "typst", output: "pdf" }),
    "Refused",
    "a mismatched output digest is refused",
  );
}

/* --------------------------------------------------------- polling / cancel */

async function testPollingIntervals() {
  const storage = fakeStorage();
  const { now } = setup({ storage, fetchImpl: null });
  local.configure({ project: "p", origin: "https://a" });
  const waits = [];
  local._testing.inject({
    wait: (ms) => { waits.push(ms); now.advance(ms); return Promise.resolve(); },
    fetch: (() => {
      let polls = 0;
      return async (url, init) => {
        if (url.endsWith("/jobs") && init.method === "POST") return jsonResponse(202, { id: "j1", status: "queued" });
        if (url.endsWith("/jobs/j1")) {
          polls += 1;
          if (polls < 25) return jsonResponse(200, { id: "j1", status: "running" });
          return jsonResponse(200, { id: "j1", status: "done", exit: 0, outputs: { artifact: { size: 1, sha256: await sha256hex(new Uint8Array([1])) } } });
        }
        if (url.endsWith("/files/artifact")) return bytesResponse(200, new Uint8Array([1]));
        throw new Error(`unexpected ${url}`);
      };
    })(),
  });
  await local.runRender({ job: { snapshot: "s", generation: 1 }, tree: { main: "main.typ", texts: { "main.typ": "x" } }, format: "typst", output: "pdf" });
  check("polling starts at 500ms", waits[0] === 500, `${waits[0]}`);
  check("polling switches to 1000ms after 10s elapsed", waits.some((w) => w === 1000), JSON.stringify(waits));
}

async function testCancelViaAbort() {
  const storage = fakeStorage();
  setup({ storage, fetchImpl: null });
  local.configure({ project: "p", origin: "https://a" });
  let canceled = false;
  const controller = new AbortController();
  local._testing.inject({
    wait: () => Promise.resolve(),
    fetch: async (url, init) => {
      if (url.endsWith("/jobs") && init.method === "POST") return jsonResponse(202, { id: "j2", status: "queued" });
      if (url.endsWith("/jobs/j2/cancel")) { canceled = true; return jsonResponse(200, {}); }
      if (url.endsWith("/jobs/j2")) { controller.abort(); return jsonResponse(200, { id: "j2", status: "running" }); }
      throw new Error(`unexpected ${url}`);
    },
  });
  await rejects(
    local.runRender({ job: { snapshot: "s", generation: 1 }, tree: { main: "main.typ", texts: { "main.typ": "x" } }, format: "typst", output: "pdf" }, { signal: controller.signal }),
    "Canceled",
    "an aborted signal cancels the job and rejects",
  );
  check("aborting posts a cancel request", canceled);
}

/* --------------------------------------------------------- rejection shapes */

async function testRefusedUnreachable() {
  const storage = fakeStorage();

  setup({ storage, fetchImpl: async () => jsonResponse(409, { error: "a newer generation is already queued" }) });
  local.configure({ project: "p", origin: "https://a" });
  const refused = await rejects(local.capabilities({}), "Refused", "a 409 with a JSON error rejects as Refused");
  check("the Refused message is the server's own", refused?.message === "a newer generation is already queued", refused?.message);

  setup({ storage, fetchImpl: async () => { throw new TypeError("network down"); } });
  local.configure({ project: "p", origin: "https://a" });
  await rejects(local.capabilities({}), "Unreachable", "a network failure rejects as Unreachable");

  setup({ storage, fetchImpl: async () => jsonResponse(502, {}) });
  local.configure({ project: "p", origin: "https://a" });
  await rejects(local.capabilities({}), "Unreachable", "a 502 from the shell rejects as Unreachable");
}

/* --------------------------------------------------------- run */

const tests = [
  testMultipartBuildBody,
  testOutputDigestIsVerified,
  testPollingIntervals,
  testCancelViaAbort,
  testRefusedUnreachable,
];

for (const test of tests) {
  await test();
}
local._testing.reset();
delete globalThis.window;

if (failures) {
  console.error(`latex-local: ${failures} check(s) failed`);
  process.exit(1);
}
console.log(`latex-local: ${tests.length} scenario(s) passed`);
