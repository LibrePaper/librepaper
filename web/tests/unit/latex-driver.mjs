// The engine distribution supplies bundle-mode.js, but the browser driver
// hardens that runtime before executing it. Keep the three bundle ingress
// paths covered: synchronous network fetch, Cache Storage preload, and the
// host-preloaded bundle message.
import assert from "node:assert/strict";
import { hardenBundleModeSource } from "../../src/lib/latex/driver.js";

const runtime = [
  '            if (sha256Hex(bytes) !== meta.sha256) return "digest-mismatch";',
  '                                if (!meta || sha256Hex(bytes) !== meta.sha256) {',
  '            if (sha256Hex(u8) !== meta.sha256) return "digest-mismatch";',
].join("\n");

const hardened = hardenBundleModeSource(runtime);
assert.equal((hardened.match(/Number\.isSafeInteger\(meta\.size\)/g) || []).length, 3);
assert.match(hardened, /bytes\.length !== meta\.size\) return "size-mismatch";\n\s+if \(sha256Hex\(bytes\)/, "network bytes are sized before hashing");
assert.match(hardened, /!meta \|\| !Number\.isSafeInteger\(meta\.size\)[^\n]+bytes\.length !== meta\.size[^\n]+sha256Hex\(bytes\)/, "cache bytes are sized before hashing");
assert.match(hardened, /u8\.length !== meta\.size\) return "size-mismatch";\n\s+if \(sha256Hex\(u8\)/, "preloaded bytes are sized before hashing");
assert.match(hardened, /size-mismatch/);
assert.equal(hardenBundleModeSource(hardened), hardened, "hardening is idempotent for a newer runtime");
assert.throws(() => hardenBundleModeSource("var BundleMode = {};"), /Unrecognised bundle-mode\.js verification runtime/);

console.log("latex-driver: bundle network, cache, and preload paths require digest and size");
