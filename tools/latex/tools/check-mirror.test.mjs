import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { mkdtempSync, mkdirSync, writeFileSync, readFileSync, rmSync } from "node:fs";
import { spawnSync } from "node:child_process";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const directory = mkdtempSync(join(tmpdir(), "librepaper-mirror-test-"));
const checker = fileURLToPath(new URL("./check-mirror.mjs", import.meta.url));
const digest = (bytes) => createHash("sha256").update(bytes).digest("hex");

// A mirror is `<id>/` release directories; id is the SHA-256 of MANIFEST.json
// and every path in release.json is relative to that directory.
let mirror;
let release;
let id;
function asset(url, content) {
  const bytes = Buffer.from(content);
  mkdirSync(join(release, url, ".."), { recursive: true });
  writeFileSync(join(release, url), bytes);
  return { url, size: bytes.length, sha256: digest(bytes) };
}
// SPEC-latex.md "The index": two tiny bundle "tars" (any bytes; check-mirror
// only hashes them) under a release's own `bundles/` directory, named by
// digest as the real ones would be.
function bundlesFixture() {
  const put = (content, slug) => {
    const bytes = Buffer.from(content);
    return asset(`bundles/b/${digest(bytes)}/${slug}.tar`, bytes);
  };
  const coreFile = put("fixture core bundle", "core");
  const tikzFile = put("fixture tikz bundle", "tex-latex-tikz");
  const index = {
    schemaVersion: 1,
    snapshot: "texlive-20260301",
    sourceDateEpoch: 1,
    bundles: {
      core: { url: coreFile.url.slice("bundles/".length), size: coreFile.size, sha256: coreFile.sha256 },
      "tex/latex/tikz": { url: tikzFile.url.slice("bundles/".length), size: tikzFile.size, sha256: tikzFile.sha256 },
    },
    files: { "tex/latex/base/article.cls": "core", "tex/generic/pgf/basiclayer/pgfcore.code.tex": "tex/latex/tikz" },
  };
  const indexFile = asset("bundles/bundles.json", JSON.stringify(index));
  return { index: indexFile.url, sha256: indexFile.sha256, snapshot: index.snapshot, count: 2, bytes: coreFile.size + tikzFile.size };
}

/// A fresh, complete mirror with one release; `mutate(entry)` may break the
/// release.json entry before it is written, and returns nothing.
function build(mutate = () => {}) {
  mirror = mkdtempSync(join(directory, "mirror-"));
  const manifest = Buffer.from(`{"label":"${Math.random()}"}\n`);
  id = digest(manifest);
  release = join(mirror, id);
  mkdirSync(release, { recursive: true });
  writeFileSync(join(release, "MANIFEST.json"), manifest);
  const entry = {
    format: 2,
    id,
    engines: { pdftex: { worker: "worker.js", files: ["worker.js"] } },
    files: { "worker.js": asset("worker.js", "worker") },
    bundles: bundlesFixture(),
  };
  mutate(entry);
  writeFileSync(join(release, "release.json"), JSON.stringify(entry));
}
function check(expected, message, target = mirror) {
  const result = spawnSync(process.execPath, [checker, target], { encoding: "utf8" });
  if (result.error) throw result.error;
  assert.equal(result.status, expected, result.stderr || result.stdout);
  if (message) assert.match(result.stderr, message);
}
try {
  build();
  check(0);
  check(0, undefined, release); // a single release directory
  build((entry) => { entry.format = 1; });
  check(1, /unsupported release format/);
  build((entry) => { delete entry.engines.pdftex; });
  check(1, /no complete pdfTeX/);
  build((entry) => { entry.bundles = null; });
  check(1, /has no bundles/);
  build();
  rmSync(join(release, "worker.js"));
  check(1, /ENOENT/);
  build();
  writeFileSync(join(release, "worker.js"), "broken");
  check(1, /digest or size mismatch/);

  // The directory name is the SHA-256 of MANIFEST.json.
  build();
  writeFileSync(join(release, "MANIFEST.json"), "changed");
  check(1, /not the SHA-256 of MANIFEST\.json/);
  build();
  rmSync(join(release, "MANIFEST.json"));
  check(1, /MANIFEST\.json is missing/);
  mirror = mkdtempSync(join(directory, "empty-"));
  check(1, /no release directories/);

  // SPEC-latex.md "The index": bundles.json's own digest must match what
  // the release pins, and every bundle path it names must actually be on
  // disk -- both independent of `release.files`, which only proves the
  // files the importer copied are intact, not that bundles.json still
  // agrees with them.
  build();
  writeFileSync(join(release, "bundles", "bundles.json"), "tampered");
  check(1, /bundle index digest mismatch/);
  build();
  const [coreName, coreBundle] = Object.entries(JSON.parse(readFileSync(join(release, "bundles", "bundles.json"), "utf8")).bundles)[0];
  writeFileSync(join(release, "bundles", coreBundle.url), "corrupted bundle bytes");
  check(1, new RegExp(`bundle asset size or digest mismatch: ${coreBundle.url}`));
  build();
  rmSync(join(release, "bundles", coreBundle.url));
  check(1, new RegExp(`bundle "${coreName}".*is missing from the mirror`));

  console.log("mirror preflight: legacy format, missing engines, missing bundles, missing assets, corruption, a wrong release id and bundle index/tar mismatches rejected");
} finally {
  rmSync(directory, { recursive: true, force: true });
}
