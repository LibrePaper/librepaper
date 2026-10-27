// The deployment's file rules are kept in this browser, so a project can
// open again with no connection to the server: the service worker does not
// keep API answers.
import assert from "node:assert/strict";
import { config, configOrRemembered } from "../../src/lib/api.js";
import { CONFIG } from "../../src/lib/storage.js";

const store = new Map();
globalThis.localStorage = {
  getItem: (key) => (store.has(key) ? store.get(key) : null),
  setItem: (key, value) => store.set(key, String(value)),
  removeItem: (key) => store.delete(key),
};

const rules = { text_extensions: [".tex", ".md"], asset_extensions: [".png"], max_path: 128 };
const answered = (body, status = 200) => async () => ({ ok: status >= 200 && status < 300, status, json: async () => body });
const unreachable = new TypeError("Failed to fetch");
const down = async () => { throw unreachable; };

// A successful load is returned as it is and kept for the next visit.
{
  store.clear();
  globalThis.fetch = answered(rules);
  assert.deepEqual(await config(), rules);
  assert.deepEqual(JSON.parse(store.get(CONFIG)), rules);
  assert.deepEqual(await configOrRemembered(), { config: rules, remembered: false });
}

// A newer answer replaces the kept one.
{
  const newer = { ...rules, max_path: 64 };
  globalThis.fetch = answered(newer);
  await config();
  assert.deepEqual(JSON.parse(store.get(CONFIG)), newer);
}

// An answer that is not an object is not kept.
{
  store.clear();
  globalThis.fetch = answered(null);
  await config();
  assert.equal(store.has(CONFIG), false);
}

// When the server cannot be reached the kept copy is used, and it says so.
{
  store.clear();
  store.set(CONFIG, JSON.stringify(rules));
  globalThis.fetch = down;
  assert.deepEqual(await configOrRemembered(), { config: rules, remembered: true });
  // The same when the server answers with an error.
  globalThis.fetch = answered({ error: "overloaded" }, 503);
  assert.deepEqual(await configOrRemembered(), { config: rules, remembered: true });
  // Reading the kept copy does not turn it into a new answer.
  assert.deepEqual(JSON.parse(store.get(CONFIG)), rules);
}

// With no kept copy the original failure is what the page sees.
{
  store.clear();
  globalThis.fetch = down;
  await assert.rejects(configOrRemembered(), (error) => error === unreachable);
  globalThis.fetch = answered({ error: "overloaded" }, 503);
  await assert.rejects(configOrRemembered(), { message: "overloaded" });
}

// A kept value that is not a set of rules is no copy at all.
{
  globalThis.fetch = down;
  for (const kept of ["not json", JSON.stringify("text"), JSON.stringify(null), JSON.stringify(7)]) {
    store.set(CONFIG, kept);
    await assert.rejects(configOrRemembered(), (error) => error === unreachable, `a kept value of ${kept} is ignored`);
  }
}

// Storage can be switched off, and the page must still work without it.
{
  globalThis.localStorage = {
    getItem: () => { throw new Error("storage is off"); },
    setItem: () => { throw new Error("storage is off"); },
  };
  globalThis.fetch = answered(rules);
  assert.deepEqual(await configOrRemembered(), { config: rules, remembered: false }, "a load still works when it cannot be kept");
  globalThis.fetch = down;
  await assert.rejects(configOrRemembered(), (error) => error === unreachable, "with no storage the original failure surfaces");
}

console.log("api-config: the rules are kept on success and stand in when the server cannot be reached");
