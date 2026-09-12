import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { mkdtempSync, mkdirSync, writeFileSync, readFileSync, rmSync } from "node:fs";
import { spawnSync } from "node:child_process";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

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
  return {
    format: 1,
    default_release: "r1",
    releases: {
      r1: {
        id: "r1",
        engines: { pdftex: { worker: "worker.js", files: ["worker.js"] } },
        files: { "worker.js": asset("worker.js", "worker") },
        bundles: bundlesFixture(),
      },
    },
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
  check({ version: 1, distributions: {} }, 1, /unsupported manifest format/);
  let manifest = complete();
  manifest.format = 2;
  check(manifest, 1, /unsupported manifest format/);
  manifest = complete();
  delete manifest.releases.r1.engines.pdftex;
  check(manifest, 1, /no default release with a complete pdfTeX/);
  manifest = complete();
  manifest.releases.r1.bundles = null;
  check(manifest, 1, /has no bundles/);
  manifest = complete();
  rmSync(join(directory, "worker.js"));
  check(manifest, 1, /file missing on disk|ENOENT/);
  manifest = complete();
  writeFileSync(join(directory, "worker.js"), "broken");
  check(manifest, 1, /digest or size mismatch/);

  // SPEC-latex.md "The index": bundles.json's own digest must match what
  // the release entry pins, and every bundle path it names must actually be
  // on disk -- both independent of `release.files`, which only proves the
  // files the importer copied are intact, not that bundles.json still
  // agrees with them.
  manifest = complete();
  check(manifest, 0);
  manifest = complete();
  writeFileSync(join(directory, "r1", "bundles", "bundles.json"), "tampered");
  check(manifest, 1, /bundle index digest mismatch/);
  manifest = complete();
  const [coreName, coreBundle] = Object.entries(JSON.parse(readFileSync(join(directory, "r1", "bundles", "bundles.json"), "utf8")).bundles)[0];
  writeFileSync(join(directory, "r1", "bundles", coreBundle.url), "corrupted bundle bytes");
  check(manifest, 1, new RegExp(`bundle asset size or digest mismatch: ${coreBundle.url}`));
  manifest = complete();
  rmSync(join(directory, "r1", "bundles", coreBundle.url));
  check(manifest, 1, new RegExp(`bundle "${coreName}".*is missing from the mirror`));

  console.log("mirror preflight: legacy format, missing engines, missing bundles, missing assets, corruption and bundle index/tar mismatches rejected");
} finally {
  rmSync(directory, { recursive: true, force: true });
}
