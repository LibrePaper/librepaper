import assert from "node:assert/strict";
import { durableProjectPersistence } from "../../src/lib/offline-projects.js";

const blockedIndexedDB = {
  open() {
    const request = {};
    queueMicrotask(() => request.onblocked?.());
    return request;
  },
};

let failure = null;
const persistence = durableProjectPersistence({ origin: "https://example.test", slug: "paper", createdAt: "now" }, blockedIndexedDB);
const opened = persistence.open({}, {
  writing() {},
  hydrated() { throw new Error("blocked storage must not report hydration"); },
  failed(error) { failure = error; },
});
await opened.hydration;
assert.match(failure?.message || "", /blocked by another tab/);
opened.close();
