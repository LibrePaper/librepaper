import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { mkdtempSync, mkdirSync, writeFileSync, readFileSync, rmSync } from "node:fs";
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
// SPEC-latex.md "The index": two tiny bundle "tars" (any bytes; check-mirror
// only hashes them) under a release's own `bundles/` directory, named by
// digest as the real ones would be.
function bundlesFixture(dir = "r1") {
  mkdirSync(join(directory, dir, "bundles"), { recursive: true });
  const core = Buffer.from("fixture core bundle");
  const tikz = Buffer.from("fixture tikz bundle");
  const put = (bytes, slug) => {
    const digest = createHash("sha256").update(bytes).digest("hex");
    const rel = `${dir}/bundles/b/${digest}/${slug}.tar`;
    mkdirSync(join(directory, dir, "bundles", "b", digest), { recursive: true });
    return asset(rel, bytes);
  };
  const coreFile = put(core, "core");
  const tikzFile = put(tikz, "tex-latex-tikz");
  const index = {
    schemaVersion: 1,
    snapshot: "texlive-20260301",
    sourceDateEpoch: 1,
    bundles: {
      core: { url: coreFile.url.slice(`${dir}/bundles/`.length), size: coreFile.size, sha256: coreFile.sha256 },
      "tex/latex/tikz": { url: tikzFile.url.slice(`${dir}/bundles/`.length), size: tikzFile.size, sha256: tikzFile.sha256 },
    },
    files: { "tex/latex/base/article.cls": "core", "tex/generic/pgf/basiclayer/pgfcore.code.tex": "tex/latex/tikz" },
  };
  const indexFile = asset(`${dir}/bundles/bundles.json`, JSON.stringify(index));
  return { index: indexFile.url, sha256: indexFile.sha256, snapshot: index.snapshot, count: 2, bytes: coreFile.size + tikzFile.size };
}

function complete() {
  const files = {
    "pdftex/26/article.cls": asset("article.cls", "class"),
    "pdftex/11/pdftex.map": asset("pdftex.map", "fonts"),
  };
  return {
    default_release: "r1",
    releases: { r1: { snapshot: "s1", engines: { pdftex: { worker: "worker.js", files: ["worker.js"] } }, files: { "worker.js": asset("worker.js", "worker") }, bundles: bundlesFixture() } },
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
  // With bundles the per-file set is optional, but a stale initial entry is
  // still a lie about what the mirror holds.
  check(manifest, 1, /initial package .* is absent/);
  manifest = complete();
  manifest.releases[manifest.default_release].bundles = null;
  delete manifest.texlive.s1.files["pdftex/11/pdftex.map"];
  check(manifest, 1, /package set is missing/);
  // A bundled release needs no per-file snapshot at all.
  manifest = complete();
  delete manifest.texlive;
  check(manifest, 0);
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

  // SPEC-latex.md "The index": bundles.json's own digest must match what
  // the release entry pins, and every bundle path it names must actually be
  // on disk -- both independent of `release.files`, which only proves the
  // files the importer copied are intact, not that bundles.json still
  // agrees with them.
  manifest = complete();
  check(manifest, 0);
  manifest = complete();
  writeFileSync(join(directory, "r1", "bundles", "bundles.json"), "tampered");
  check(manifest, 1, /bundles\.json digest does not match/);
  manifest = complete();
  const [coreName, coreBundle] = Object.entries(JSON.parse(readFileSync(join(directory, "r1", "bundles", "bundles.json"), "utf8")).bundles)[0];
  writeFileSync(join(directory, "r1", "bundles", coreBundle.url), "corrupted bundle bytes");
  check(manifest, 1, new RegExp(`bundle asset size or digest mismatch: ${coreBundle.url}`));
  manifest = complete();
  rmSync(join(directory, "r1", "bundles", coreBundle.url));
  check(manifest, 1, new RegExp(`bundle "${coreName}".*is missing from the mirror`));

  console.log("mirror preflight: legacy, missing assets, corruption, stale filters, invalid prefetch entries and bundle index/tar mismatches rejected");
} finally {
  rmSync(directory, { recursive: true, force: true });
}
