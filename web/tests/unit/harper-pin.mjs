// The JavaScript glue from the harper.js npm package only works with the wasm
// built in the same release, so the pinned harper.wasm must be that package's
// slim wasm, byte for byte. This test fails when one is bumped without the other.
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { existsSync, readFileSync } from "node:fs";
import test from "node:test";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("../../../", import.meta.url));
const wasm = fileURLToPath(new URL("../../node_modules/harper.js/dist/harper_wasm_slim_bg.wasm", import.meta.url));

function pinnedDigest() {
  const text = readFileSync(`${root}assets.lock`, "utf8");
  for (const raw of text.split(/\r?\n/)) {
    const fields = raw.replace(/#.*$/, "").trim().split(/\s+/);
    if (fields[0] === "harper.wasm") return fields[3];
  }
  throw new Error("assets.lock has no harper.wasm row");
}

test("the pinned harper.wasm is the harper.js slim wasm, byte for byte", (t) => {
  if (!existsSync(wasm)) return t.skip("web/node_modules/harper.js missing");
  const digest = createHash("sha256").update(readFileSync(wasm)).digest("hex");
  assert.equal(digest, pinnedDigest());
});
