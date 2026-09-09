import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { mkdtempSync, writeFileSync, rmSync } from "node:fs";
import { spawnSync } from "node:child_process";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { buildBloom } from "./bloom.mjs";

const directory = mkdtempSync(join(tmpdir(), "librepaper-mirror-test-"));
const checker = fileURLToPath(new URL("./check-mirror.mjs", import.meta.url));
function asset(url, content) {
  const bytes = Buffer.from(content);
  writeFileSync(join(directory, url), bytes);
  return { url, size: bytes.length, sha256: createHash("sha256").update(bytes).digest("hex") };
}
function complete() {
  const files = {
    "pdftex/26/article.cls": asset("article.cls", "class"),
    "pdftex/11/pdftex.map": asset("pdftex.map", "fonts"),
  };
  return {
    default_release: "r1",
    releases: { r1: { snapshot: "s1", engines: { pdftex: { worker: "worker.js", files: ["worker.js"] } }, files: { "worker.js": asset("worker.js", "worker") } } },
    texlive: { s1: { files, initial: Object.keys(files), bloom: asset("bloom.bin", buildBloom(Object.keys(files))) } },
  };
}
function check(manifest, expected, message) {
  writeFileSync(join(directory, "manifest.json"), JSON.stringify(manifest));
  const result = spawnSync(process.execPath, [checker, directory], { encoding: "utf8" });
  if (result.error) throw result.error;
  assert.equal(result.status, expected, result.stderr || result.stdout);
  if (message) assert.match(result.stderr, message);
}
try {
  check(complete(), 0);
  check({ version: 1, distributions: {} }, 1, /no default WasmTex/);
  let manifest = complete();
  delete manifest.texlive.s1.files["pdftex/11/pdftex.map"];
  check(manifest, 1, /package set is missing/);
  manifest = complete();
  rmSync(join(directory, "worker.js"));
  check(manifest, 1, /ENOENT/);
  manifest = complete();
  writeFileSync(join(directory, "worker.js"), "broken");
  check(manifest, 1, /digest mismatch/);
  manifest = complete();
  manifest.texlive.s1.bloom = asset("bloom.bin", buildBloom([]));
  check(manifest, 1, /lookup filter hides/);
  manifest = complete();
  manifest.texlive.s1.initial.push("pdftex/26/missing.sty");
  check(manifest, 1, /initial package.*absent/);
  console.log("mirror preflight: legacy, missing assets, corruption, stale filters and invalid prefetch entries rejected");
} finally {
  rmSync(directory, { recursive: true, force: true });
}
