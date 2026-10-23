// When the network fails, the reader opens its remembered copy. That copy
// carries the documents origin, which the preview frame cannot do without.
import assert from "node:assert/strict";
import { createReaderBoot } from "../../src/lib/reader/boot.js";

const opened = new Promise((resolve) => {
  const boot = createReaderBoot({
    slug: "paper",
    fetcher: async () => { throw new TypeError("network down"); },
    whoami: async () => ({}),
    findLocal: async () => ({ document: { title: "Paper", docs_origin: "https://docs.example.test" } }),
    onDocument: resolve,
    onError: (error) => resolve(Promise.reject(error)),
  });
  void boot.start();
});
const document_ = await opened;
assert.equal(document_.docs_origin, "https://docs.example.test");
assert.equal(document_.local_copy, true);
