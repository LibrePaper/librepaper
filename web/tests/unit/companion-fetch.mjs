import assert from "node:assert/strict";
import * as local from "../../src/lib/companion/client.js";

const response = (body, status = 200, contentType = "application/json") => ({
  status,
  ok: status >= 200 && status < 300,
  headers: { get: (name) => name.toLowerCase() === "content-type" ? contentType : null },
  json: async () => body,
  text: async () => JSON.stringify(body),
  clone() { return response(body, status, contentType); },
});

const storeWithPairing = () => new Map([[
  "librepaper-local-connections",
  JSON.stringify({ "https://papers.example": { token: "pairing-token", expires: 1_000_000, instance: "companion" } }),
]]);

function setup(fetch, storage = new Map()) {
  local._testing.reset();
  local._testing.inject({
    storage: {
      getItem: (key) => storage.get(key) ?? null,
      setItem: (key, value) => storage.set(key, value),
      removeItem: (key) => storage.delete(key),
    },
    location: () => ({ href: "https://papers.example/docs/paper" }),
    fetch,
  });
  local.configure({ project: "paper", origin: "https://papers.example" });
  return storage;
}

// An explicit option validation failure during health falls back once. The
// remembered decision then applies to pairing verification, management, and
// job submission.
{
  const requests = [];
  let rejectedHealthOption = false;
  setup(async (url, init = {}) => {
    requests.push({ url, init });
    if (url.endsWith("/librepaper/local/health")) {
      if (init.targetAddressSpace && !rejectedHealthOption) {
        rejectedHealthOption = true;
        throw new TypeError("targetAddressSpace is not a valid RequestInit value");
      }
      return response({ service: "librepaper-local", protocol: [2], instance: "companion" });
    }
    if (url.endsWith("/librepaper/local/capabilities")) return response({ manage: true, builders: {} });
    if (url.endsWith("companion/api/settings")) return response({ setting: true });
    if (url.endsWith("/librepaper/local/jobs")) return response({ id: "job-1" });
    if (url.endsWith("/librepaper/local/jobs/job-1")) return response({ status: "done", exit: 0, outputs: {} });
    throw new Error(`Unexpected request: ${url}`);
  }, storeWithPairing());

  const connected = await local.connectApp({ timeoutMs: 1, pollMs: 0 });
  assert.equal(connected.state, "connected");
  assert.deepEqual(await local.manage("/settings"), { setting: true });
  const built = await local.runBuild({ tree: { main: "paper.qmd", texts: { "paper.qmd": "# Paper" } }, builder: "quarto" });
  assert.equal(built.ok, false, "the stub omits Quarto outputs after exercising job submission and polling");
  const jobPost = requests.find(({ url, init }) => url.endsWith("/librepaper/local/jobs") && init.method === "POST");
  assert.ok(jobPost, "job submission sends a POST");
  assert.ok(jobPost.init.body instanceof FormData, "job submission preserves its multipart body");
  assert.equal(jobPost.init.mode, "cors");
  assert.equal(jobPost.init.credentials, "omit");

  const healthRequests = requests.filter(({ url }) => url.endsWith("/librepaper/local/health"));
  assert.equal(healthRequests.length, 2, "health option validation falls back once");
  assert.equal(healthRequests[0].init.targetAddressSpace, "loopback");
  assert.equal("targetAddressSpace" in healthRequests[1].init, false);
  assert.ok(requests.slice(2).every(({ init }) => !("targetAddressSpace" in init)), "the support decision applies to later paired requests");
}

// The same explicit validation fallback is safe for the session POST itself.
{
  let attempts = 0;
  const sessionRequests = [];
  setup(async (url, init = {}) => {
    if (url.endsWith("companion/api/session")) {
      attempts += 1;
      sessionRequests.push(init);
      if (init.targetAddressSpace) throw new TypeError("targetAddressSpace is unsupported");
      return response({ token: "pairing-token", expires: 1_000_000, instance: "companion" });
    }
    if (url.endsWith("/librepaper/local/health")) return response({ service: "librepaper-local", protocol: [2], instance: "companion" });
    if (url.endsWith("/librepaper/local/capabilities")) return response({ manage: true, builders: {} });
    throw new Error(`Unexpected request: ${url}`);
  });
  assert.equal((await local.connectApp({ timeoutMs: 1, pollMs: 0 })).state, "connected");
  assert.equal(attempts, 2, "the explicit pre-send option validation error retries the POST once");
  const [withOption, withoutOption] = sessionRequests;
  assert.equal(withOption.method, "POST");
  assert.equal(withOption.body, "{}");
  assert.ok(withOption.signal instanceof AbortSignal);
  for (const key of ["method", "mode", "credentials", "cache", "headers", "body", "signal"]) {
    assert.equal(withoutOption[key], withOption[key], `fallback preserves ${key}`);
  }
}

// A generic network TypeError during job POST is never replayed.
{
  for (const message of ["Failed to fetch", "targetAddressSpace blocked by mixed-content policy"]) {
    let posts = 0;
    setup(async (url, init = {}) => {
      if (url.endsWith("/librepaper/local/jobs") && init.method === "POST") {
        posts += 1;
        throw new TypeError(message);
      }
      throw new Error(`Unexpected request: ${url}`);
    }, storeWithPairing());
    await assert.rejects(
      local.runBuild({ tree: { main: "paper.qmd", texts: { "paper.qmd": "# Paper" } }, builder: "quarto" }),
      (error) => error.name === "Unreachable",
    );
    assert.equal(posts, 1, `a generic TypeError is not replayed: ${message}`);
  }
}

local._testing.reset();
console.log("companion-fetch: option fallback is shared across pairing, management, and job requests without network replay");
