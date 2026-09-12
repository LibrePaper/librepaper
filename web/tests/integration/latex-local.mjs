// The local bridge client, against a fake local LibrePaper.
//
// `local.js` never touches `fetch` or `localStorage` directly -- everything
// ambient goes through its `_testing.inject`-able `deps` -- so this check
// drives the exact production module under Node with a small fake HTTP
// service and an in-memory store, and controls the wall clock so the
// negative-cache backoff does not actually take ten minutes to check.

import * as local from "../../src/lib/latex/local.js";

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

const HEALTH_OK = { service: "librepaper-local", protocol: [1], version: "0.1.0", instance: "abc123" };

function setup({ storage = fakeStorage(), now = clock(), fetchImpl } = {}) {
  local._testing.reset();
  local._testing.inject({ storage, now: now.now, fetch: fetchImpl, wait: (ms) => Promise.resolve() });
  return { storage, now };
}

/* --------------------------------------------------------- address & pairings */

async function testAddress() {
  setup();
  check("address() defaults to DEFAULT_ADDRESS", local.address() === local.DEFAULT_ADDRESS);
  local.setAddress("http://127.0.0.1:9999/");
  check("setAddress() persists", local.address() === "http://127.0.0.1:9999/");
}

/* --------------------------------------------------------- probe classification */

async function testProbeUnreachable() {
  const { now } = setup({ fetchImpl: async () => { throw new TypeError("fetch failed"); } });
  const status = await local.probe();
  check("a network failure classifies as unreachable", status.state === "unreachable", status.state);
  check("unreachable carries instructions", /librepaper local start/.test(status.instructions));

  let calls = 0;
  local._testing.inject({ fetch: async () => { calls += 1; throw new TypeError("fetch failed"); } });
  const cached = await local.probe();
  check("a repeat probe within the backoff uses the cache", calls === 0, `calls=${calls}`);
  check("the cached status is returned", cached.state === "unreachable");

  now.advance(61_000);
  await local.probe();
  check("the probe runs again once the backoff elapses", calls === 1, `calls=${calls}`);
}

async function testProbeDenied() {
  setup({ fetchImpl: async () => { throw new Error("Access to the network address space was blocked"); } });
  const status = await local.probe();
  check("a permission-denied fetch classifies as denied", status.state === "denied", status.state);
  check("denied carries instructions", /local network/.test(status.instructions));
}

async function testProbeIncompatible() {
  setup({ fetchImpl: async () => jsonResponse(200, { service: "librepaper-local", protocol: [2], version: "9.0", instance: "x" }) });
  const status = await local.probe();
  check("a protocol without 1 classifies as incompatible", status.state === "incompatible", status.state);
}

async function testProbeUnauthorizedThenConnected() {
  const storage = fakeStorage();
  local.configure({ project: "demo", origin: "https://librepaper.example" });
  setup({ storage, fetchImpl: async () => jsonResponse(200, HEALTH_OK) });
  local.configure({ project: "demo", origin: "https://librepaper.example" });
  const status = await local.probe();
  check("reachable with no pairing classifies as unauthorized", status.state === "unauthorized", status.state);

  storage.setItem("librepaper-local-pairings", JSON.stringify({ "https://librepaper.example|demo": { token: "tok", expires: 0, instance: "abc" } }));
  local._testing.inject({
    fetch: async (url) => {
      if (url.endsWith("/health")) return jsonResponse(200, HEALTH_OK);
      if (url.endsWith("/capabilities")) return jsonResponse(200, { tools: {}, confinement: { available: false, kind: "none", reason: "" }, platform: "linux", distribution: null });
      throw new Error(`unexpected ${url}`);
    },
  });
  const connected = await local.probe({ force: true });
  check("a valid pairing verifies into connected", connected.state === "connected", connected.state);
  check("capabilities are attached", connected.capabilities && typeof connected.capabilities === "object");
}

async function testProbeDropsExpiredToken() {
  const storage = fakeStorage();
  local.configure({ project: "demo", origin: "https://librepaper.example" });
  storage.setItem("librepaper-local-pairings", JSON.stringify({ "https://librepaper.example|demo": { token: "stale", expires: 0, instance: "abc" } }));
  setup({ storage, fetchImpl: async (url) => {
    if (url.endsWith("/health")) return jsonResponse(200, HEALTH_OK);
    if (url.endsWith("/capabilities")) return jsonResponse(401, { error: "expired" });
    throw new Error(`unexpected ${url}`);
  } });
  local.configure({ project: "demo", origin: "https://librepaper.example" });
  const status = await local.probe();
  check("a 401 verifying capabilities falls back to unauthorized", status.state === "unauthorized", status.state);
  const stored = JSON.parse(storage.getItem("librepaper-local-pairings"));
  check("the stale token is dropped from storage", !("https://librepaper.example|demo" in stored));
}

/* --------------------------------------------------------- negative-cache backoff */

async function testBackoffDoubles() {
  const { now } = setup({ fetchImpl: async () => { throw new TypeError("fetch failed"); } });
  await local.probe();
  now.advance(60_000);
  await local.probe(); // still unreachable, backoff should double to 120s
  now.advance(61_000);
  let calls = 0;
  local._testing.inject({ fetch: async () => { calls += 1; throw new TypeError("still down"); } });
  await local.probe();
  check("a 61s gap after a doubled 60s backoff is not yet enough", calls === 0, `calls=${calls}`);
  now.advance(60_000);
  await local.probe();
  check("the doubled backoff (120s total) eventually elapses", calls === 1, `calls=${calls}`);
}

async function testRetryAndConnectClearBackoff() {
  const { now } = setup({ fetchImpl: async () => { throw new TypeError("fetch failed"); } });
  await local.probe();
  let calls = 0;
  local._testing.inject({ fetch: async (url) => { calls += 1; return jsonResponse(200, HEALTH_OK); } });
  await local.retry();
  check("retry() bypasses the negative cache", calls >= 1, `calls=${calls}`);
}

/* --------------------------------------------------------- connect / disconnect */

async function testConnectStoresPairing() {
  const storage = fakeStorage();
  local.configure({ project: "proj1", origin: "https://app.example" });
  setup({ storage, fetchImpl: async (url, init) => {
    if (url.endsWith("/health")) return jsonResponse(200, HEALTH_OK);
    if (url.endsWith("/connect")) {
      const body = JSON.parse(init.body);
      check("connect posts origin/project/code", body.origin === "https://app.example" && body.project === "proj1" && body.code === "654321");
      return jsonResponse(200, { token: "newtoken", expires: 9999999999 });
    }
    if (url.endsWith("/capabilities")) return jsonResponse(200, { tools: {}, confinement: { available: false, kind: "none", reason: "" }, platform: "linux", distribution: null });
    throw new Error(`unexpected ${url}`);
  } });
  local.configure({ project: "proj1", origin: "https://app.example" });
  await local.connect("654321");
  const stored = JSON.parse(storage.getItem("librepaper-local-pairings"));
  check("the pairing is stored under origin|project", stored["https://app.example|proj1"]?.token === "newtoken", JSON.stringify(stored));
}

async function testDisconnectRemovesPairing() {
  const storage = fakeStorage();
  local.configure({ project: "proj1", origin: "https://app.example" });
  storage.setItem("librepaper-local-pairings", JSON.stringify({ "https://app.example|proj1": { token: "tok", expires: 0, instance: "x" } }));
  setup({ storage, fetchImpl: async (url) => {
    if (url.endsWith("/disconnect")) return jsonResponse(200, {});
    if (url.endsWith("/health")) return jsonResponse(200, HEALTH_OK);
    throw new Error(`unexpected ${url}`);
  } });
  local.configure({ project: "proj1", origin: "https://app.example" });
  await local.disconnect();
  const stored = JSON.parse(storage.getItem("librepaper-local-pairings"));
  check("disconnect() removes the pairing", !("https://app.example|proj1" in stored));
}

/* --------------------------------------------------------- multipart bodies */

async function testMultipartBiberBody() {
  const storage = fakeStorage();
  local.configure({ project: "proj1", origin: "https://app.example" });
  storage.setItem("librepaper-local-pairings", JSON.stringify({ "https://app.example|proj1": { token: "tok", expires: 0, instance: "x" } }));
  const nonUtf8 = new Uint8Array([0xff, 0xfe, 0x00, 0x80, 0x81, 0x41, 0x42]);
  let capturedForm = null;
  setup({ storage, fetchImpl: async (url, init) => {
    if (url.endsWith("/jobs") && init.method === "POST") { capturedForm = init.body; return jsonResponse(202, { id: "job1", status: "queued" }); }
    if (url.endsWith("/jobs/job1")) return jsonResponse(200, { id: "job1", status: "done", exit: 0, outputs: { bbl: { size: 1 }, blg: { size: 1 } }, provenance: { tools: { biber: "2.21" } } });
    if (url.endsWith("/files/bbl")) return bytesResponse(200, new TextEncoder().encode("bbl-bytes"));
    if (url.endsWith("/files/blg")) return bytesResponse(200, new TextEncoder().encode("blg-bytes"));
    throw new Error(`unexpected ${url}`);
  } });
  local.configure({ project: "proj1", origin: "https://app.example" });
  const result = await local.runBiber({
    job: { snapshot: "snap1", generation: 1 },
    stem: "main",
    bcf: new TextEncoder().encode("<bcf/>"),
    files: { "refs.bib": new TextEncoder().encode("@article{x,}"), "assets/weird.bin": nonUtf8 },
    identity: "id1",
  });
  check("runBiber resolves ok on a clean done job", result.ok === true, JSON.stringify(result));
  check("the tool version comes from provenance", result.tool.version === "2.21");
  check("the body is a FormData", capturedForm instanceof FormData);
  const jobPart = capturedForm.get("job");
  check("the job part is present", jobPart != null);
  const jobJson = JSON.parse(await jobPart.text());
  check("the job part parses to a JobRequest", jobJson.kind === "biber" && jobJson.stem === "main", JSON.stringify(jobJson));
  check("the BCF is named <stem>.bcf in the manifest", jobJson.manifest.some((m) => m.path === "main.bcf"));
  const fileParts = capturedForm.getAll("file");
  check("every input became a file part", fileParts.length === 3, `got ${fileParts.length}`);
  const bcfPart = fileParts.find((f) => f.name === "main.bcf");
  check("the bcf part carries its filename", bcfPart != null);
  const weirdPart = fileParts.find((f) => f.name === "assets/weird.bin");
  check("a nested path survives as the file part's name", weirdPart != null);
  const weirdBytes = new Uint8Array(await weirdPart.arrayBuffer());
  check(
    "non-UTF-8 bytes survive the multipart round trip exactly",
    weirdBytes.length === nonUtf8.length && weirdBytes.every((b, i) => b === nonUtf8[i]),
    `${[...weirdBytes]}`,
  );
}

/* --------------------------------------------------------- polling / cancel */

async function testPollingIntervals() {
  const storage = fakeStorage();
  local.configure({ project: "p", origin: "https://a" });
  storage.setItem("librepaper-local-pairings", JSON.stringify({ "https://a|p": { token: "tok", expires: 0, instance: "x" } }));
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
          return jsonResponse(200, { id: "j1", status: "done", exit: 0, outputs: { bbl: { size: 1 }, blg: { size: 1 } }, provenance: {} });
        }
        if (url.endsWith("/files/bbl")) return bytesResponse(200, new Uint8Array([1]));
        if (url.endsWith("/files/blg")) return bytesResponse(200, new Uint8Array([2]));
        throw new Error(`unexpected ${url}`);
      };
    })(),
  });
  await local.runBiber({ job: { snapshot: "s", generation: 1 }, stem: "main", bcf: new Uint8Array([1]), files: {}, identity: "i" });
  check("polling starts at 500ms", waits[0] === 500, `${waits[0]}`);
  check("polling switches to 1000ms after 10s elapsed", waits.some((w) => w === 1000), JSON.stringify(waits));
}

async function testCancelViaAbort() {
  const storage = fakeStorage();
  local.configure({ project: "p", origin: "https://a" });
  storage.setItem("librepaper-local-pairings", JSON.stringify({ "https://a|p": { token: "tok", expires: 0, instance: "x" } }));
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
    local.runBiber({ job: { snapshot: "s", generation: 1 }, stem: "main", bcf: new Uint8Array([1]), files: {}, identity: "i" }, { signal: controller.signal }),
    "Canceled",
    "an aborted signal cancels the job and rejects",
  );
  check("aborting posts a cancel request", canceled);
}

/* --------------------------------------------------------- rejection shapes */

async function testRefusedUnreachableUnauthorized() {
  const storage = fakeStorage();
  local.configure({ project: "p", origin: "https://a" });
  storage.setItem("librepaper-local-pairings", JSON.stringify({ "https://a|p": { token: "tok", expires: 0, instance: "x" } }));

  setup({ storage, fetchImpl: async () => jsonResponse(409, { error: "a newer generation is already queued" }) });
  local.configure({ project: "p", origin: "https://a" });
  const refused = await rejects(local.capabilities({}), "Refused", "a 409 with a JSON error rejects as Refused");
  check("the Refused message is the server's own", refused?.message === "a newer generation is already queued", refused?.message);

  setup({ storage, fetchImpl: async () => { throw new TypeError("network down"); } });
  local.configure({ project: "p", origin: "https://a" });
  await rejects(local.capabilities({}), "Unreachable", "a network failure rejects as Unreachable");

  setup({ storage: fakeStorage(), fetchImpl: async () => jsonResponse(200, {}) });
  local.configure({ project: "p", origin: "https://a" });
  await rejects(local.capabilities({}), "Unauthorized", "no stored pairing rejects as Unauthorized");

  setup({ storage, fetchImpl: async () => jsonResponse(401, { error: "nope" }) });
  local.configure({ project: "p", origin: "https://a" });
  await rejects(local.capabilities({}), "Unauthorized", "a 401 rejects as Unauthorized");
}

/* --------------------------------------------------------- run */

const tests = [
  testAddress,
  testProbeUnreachable,
  testProbeDenied,
  testProbeIncompatible,
  testProbeUnauthorizedThenConnected,
  testProbeDropsExpiredToken,
  testBackoffDoubles,
  testRetryAndConnectClearBackoff,
  testConnectStoresPairing,
  testDisconnectRemovesPairing,
  testMultipartBiberBody,
  testPollingIntervals,
  testCancelViaAbort,
  testRefusedUnreachableUnauthorized,
];

for (const test of tests) {
  await test();
}
local._testing.reset();

if (failures) {
  console.error(`latex-local: ${failures} check(s) failed`);
  process.exit(1);
}
console.log(`latex-local: ${tests.length} scenario(s) passed`);
