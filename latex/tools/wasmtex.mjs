#!/usr/bin/env node
// The WasmTex half of `latex/mirror`: engine release plus TeX Live package
// snapshot, mirrored under LibrePaper's own names so the browser never depends
// on an upstream project's live service (see `docs/specs/wasmtex-interfaces.md`
// section 1, the contract this file produces exactly).
//
// Release import and package mirroring are idempotent: verified release
// files are left alone, and known package keys are not fetched again.
//
//   node latex/tools/wasmtex.mjs --release <staged directory> --sha256 <manifest digest>
//     Imports a verified wasm-latex release, including its notices and receipts,
//     into a digest-named directory and registers its complete engines.
//
//   node latex/tools/wasmtex.mjs --texlive <key>...
//   node latex/tools/wasmtex.mjs --texlive-from <file.json>
//     Fetches TeX Live package files by key (`pdftex/26/amsmath.sty`) from
//     the pinned snapshot into latex/mirror/texlive/<snapshot>/, records them
//     (or their absence) in the manifest, and regenerates the bloom filter.
//     <file.json> is a JSON array of keys, or an object whose values are
//     arrays of keys (as `wasmtex-record.mjs` writes per document).
//
//   node latex/tools/wasmtex.mjs --initial <key>...
//   node latex/tools/wasmtex.mjs --texlive-root icudt68l.dat   # snapshot-root files
//   node latex/tools/wasmtex.mjs --vm latex/mirror/biber-vm/<id>  # register the Biber VM
//     Sets manifest.texlive[snapshot].initial to exactly these keys -- the
//     compact set the browser prefetches in parallel before a first compile.
//     Every key must already be present (fetched by a prior --texlive run);
//     this flag never fetches.
//
//   node latex/tools/wasmtex.mjs --scheme [<collection>...]
//     A package set chosen by TeX Live collection rather than by what the
//     corpus happened to compile -- see `scheme.mjs`, whose key derivation
//     this reuses. Defaults to `basic latex latexrecommended latexextra
//     fontsrecommended mathscience` (DEFAULT_SCHEME). Fetches every key from
//     the pinned upstream snapshot, 16 requests in flight, records present
//     and absent, and regenerates the bloom filter once at the end -- this
//     is what keeps an ordinary document's `\usepackage{amssymb}` from
//     meeting a bloom filter that has never heard of it.
//
// The forms compose in one invocation, in this order: release, then
// --scheme, then --texlive, then --texlive-root, then --initial, then --vm,
// so `--scheme --texlive-from corpus.json --initial $(cat initial-keys.txt)`
// is one call.

import { createHash } from "node:crypto";
import {
  existsSync,
  mkdirSync,
  readFileSync,
  writeFileSync,
  statSync,
} from "node:fs";
import { dirname, join, basename } from "node:path";
import { readRelease } from "./release.mjs";
import { buildBloom, verifyBloom } from "./bloom.mjs";
import { SCHEME, schemeKeys } from "./scheme.mjs";

const HERE = dirname(new URL(import.meta.url).pathname);
// This file is latex/tools/wasmtex.mjs, so the repository is two directories up.
const REPO = dirname(dirname(HERE));

/* --------------------------------------------------------------- pinning */

// The first release inputs, per docs/specs/wasmtex.md "Starting point". Not moving
// dependencies: a new upstream id is a new release, mirrored beside this one,
// never an overwrite of it.
export const ENGINE_RELEASE = "2026-8b7946970153c52e";
export const SNAPSHOT = "2026-ba38749b8714505a";
const TEXLIVE_UPSTREAM = `https://texlive.corca.ai/snapshots/${SNAPSHOT}/2026/`;

const OUT = join(REPO, "latex", "mirror");
const sha256 = (bytes) => createHash("sha256").update(bytes).digest("hex");

/* ------------------------------------------------------------- manifest */

function readManifest() {
  const path = join(OUT, "manifest.json");
  if (!existsSync(path)) return { version: 1, distributions: {}, packages: {} };
  return JSON.parse(readFileSync(path, "utf8"));
}

function writeManifest(manifest) {
  mkdirSync(OUT, { recursive: true });
  writeFileSync(join(OUT, "manifest.json"), JSON.stringify(manifest, null, 2) + "\n");
}

/// sha256 of a JSON object's canonical form: keys sorted at every level, so
/// the digest depends on content and not on insertion order or formatting.
function canonicalDigest(value) {
  const canon = (v) => {
    if (Array.isArray(v)) return v.map(canon);
    if (v && typeof v === "object") {
      const out = {};
      for (const key of Object.keys(v).sort()) out[key] = canon(v[key]);
      return out;
    }
    return v;
  };
  return sha256(Buffer.from(JSON.stringify(canon(value))));
}

/* ---------------------------------------------------------- 1. release */

async function fetchBytes(url) {
  const response = await fetch(url);
  if (!response.ok) throw new Error(`${url}: ${response.status} ${response.statusText}`);
  return Buffer.from(await response.arrayBuffer());
}

/// Places one file's bytes at `wasmtex/<engineRelease>/<name>`. Idempotent:
/// a file already there of the right size is left alone, since the path is
/// pinned by the caller (the release directory) rather than derived from the
/// bytes -- unlike the texlive half below, an engine release's own directory
/// name is already the identity, so files inside it keep upstream names for
/// their loaders to find each other by.
function place(relative, bytes) {
  const path = join(OUT, relative);
  if (!existsSync(path) || statSync(path).size !== bytes.length) {
    mkdirSync(dirname(path), { recursive: true });
    writeFileSync(path, bytes);
  }
}

export async function mirrorRelease(directory, expectedDigest) {
  const { manifest: staged, files: payload, digest, engines } = readRelease(directory, expectedDigest);
  const engineRelease = `librepaper-${digest}`;
  const releaseId = `${engineRelease}+${SNAPSHOT}`;
  const releaseDir = `wasmtex/${engineRelease}`;
  const files = {};
  // Verify everything before writing any release bytes. Notices retain their
  // original paths so relative links and receipts remain usable.
  for (const [name, bytes] of payload) {
    const url = `${releaseDir}/${name}`;
    const path = join(OUT, url);
    mkdirSync(dirname(path), { recursive: true });
    if (!existsSync(path) || sha256(readFileSync(path)) !== sha256(bytes)) writeFileSync(path, bytes);
    files[name] = { url, sha256: sha256(bytes), size: bytes.length };
  }
  const bibliography = await bibliographyIdentity();
  const entry = {
    id: releaseId,
    engine_release: engineRelease,
    snapshot: SNAPSHOT,
    texlive: "2026",
    kernel: "LaTeX2e 2026-06-01",
    base: `${releaseDir}/`,
    texlive_base: `texlive/${SNAPSHOT}/`,
    engines,
    files,
    bibliography,
    vm: null,
    source: {
      corresponding_source: staged.correspondingSource,
      manifest: files["MANIFEST.json"],
      build_receipts: [...payload.keys()].filter((name) => /^(BUILD|FORMAT|SOURCE)-RECEIPT/.test(name)),
      reproduced: false,
    },
    licences: {
      ...Object.fromEntries(staged.families.map(({ family, combinedTerms }) => [family, combinedTerms])),
      notices: `${releaseDir}/`,
    },
    sizes: {
      ...Object.fromEntries(Object.entries(engines).map(([name, spec]) => [name, sizeOf(files, spec.files)])),
      texlive_initial: 0,
    },
  };
  const manifest = readManifest();
  manifest.version = 1;
  manifest.releases ||= {};
  const previous = manifest.releases[releaseId];
  if (previous?.vm) entry.vm = previous.vm;
  const initial = manifest.texlive?.[SNAPSHOT]?.initial || [];
  entry.sizes.texlive_initial = initial.reduce((sum, key) => sum + (manifest.texlive[SNAPSHOT].files[key]?.size || 0), 0);
  entry.digest = canonicalDigest({ ...entry, digest: undefined });
  manifest.releases[releaseId] = entry;
  manifest.default_release = releaseId;
  writeManifest(manifest);
  console.log(`wasmtex: release ${releaseId} mirrored (${Object.keys(engines).join(", ")})`);
}

function sizeOf(files, names) {
  return names.reduce((sum, name) => sum + (files[name]?.size || 0), 0);
}

/// Reads bibliography version identity out of the snapshot's own
/// `biblatex.sty`, fetching it into the texlive mirror first if it is not
/// already there -- the same path a document that `\usepackage{biblatex}`s
/// would resolve.
async function bibliographyIdentity() {
  const key = "pdftex/26/biblatex.sty";
  const { entry } = await ensureTexlive([key]).then((r) => ({ entry: r.present[key] }));
  const bytes = readFileSync(join(OUT, entry.url));
  const text = bytes.toString("utf8");
  const bcf = /\\def\\blx@bcfversion\{([^}]+)\}/.exec(text)?.[1] ?? null;
  const version = /\\def\\abx@version\{([^}]+)\}/.exec(text)?.[1] ?? null;
  const date = /\\def\\abx@date\{([^}]+)\}/.exec(text)?.[1] ?? null;
  if (!bcf || !version) {
    throw new Error("wasmtex: could not read \\blx@bcfversion/\\abx@version out of the snapshot's biblatex.sty");
  }
  // biblatex's documented pairing is bcf-version to bcf-version, not
  // package-version to package-version: the control file is what biber
  // actually reads, and biblatex 3.21 (bcf 3.11) is documented as needing
  // Biber 2.21. This snapshot's biblatex is 3.22 (bcf still 3.11, unchanged
  // from 3.21), so the same Biber build is expected compatible, but that is
  // an inference from the unchanged control-file version rather than a
  // pairing CTAN documents for 3.22 by name -- flagged here rather than
  // asserted as verified.
  const compatible = bcf === "3.11" ? ["2.21"] : [];
  const incompatible_hint =
    bcf === "3.11"
      ? `biblatex ${version} (bcf ${bcf}) is inferred compatible with Biber 2.21 (the documented pairing for` +
        " bcf 3.11, biblatex 3.21) because the control-file version did not change; not independently" +
        " re-verified against a CTAN changelog for this exact biblatex point release."
      : `biblatex ${version} uses control file ${bcf}, not the 3.11 this pinning was checked against; ` +
        "re-derive the Biber pairing before trusting it.";
  return { bibtex: "0.99e", biblatex: version, control_file: bcf, biber: { compatible, incompatible_hint }, biblatex_date: date };
}

/* -------------------------------------------------------- 2. texlive */

/// Fetches package files by manifest key (`pdftex/<format>/<name>`) from the
/// pinned snapshot, idempotently: a key already present or already recorded
/// absent is not asked for again. Returns `{ present, absent }` maps of the
/// keys this call touched (not the whole manifest).
export async function ensureTexlive(keys) {
  const manifest = readManifest();
  const section = (manifest.texlive ||= {});
  const entry = (section[SNAPSHOT] ||= { upstream: TEXLIVE_UPSTREAM, files: {}, absent: {}, initial: [] });
  const present = {};
  const absent = {};
  let changed = false;
  for (const key of new Set(keys)) {
    if (entry.files[key]) {
      present[key] = entry.files[key];
      continue;
    }
    if (entry.absent[key]) {
      absent[key] = true;
      continue;
    }
    const url = `${TEXLIVE_UPSTREAM}${key}`;
    let response;
    try {
      response = await fetch(url);
    } catch (error) {
      throw new Error(`wasmtex: fetching ${url} failed: ${error.message}`);
    }
    // The worker treats any status >= 400 as "not there" (see
    // wasm-build/pdftex-worker.js tryFetch: only status === 200 is a hit).
    if (response.status >= 400) {
      entry.absent[key] = true;
      absent[key] = true;
      changed = true;
      continue;
    }
    if (!response.ok) {
      throw new Error(`wasmtex: ${url}: unexpected status ${response.status}`);
    }
    const bytes = Buffer.from(await response.arrayBuffer());
    const digest = sha256(bytes);
    const name = key.slice(key.lastIndexOf("/") + 1);
    const relativeUrl = `texlive/${SNAPSHOT}/${digest.slice(0, 2)}/${digest.slice(0, 16)}-${name}`;
    const path = join(OUT, ...relativeUrl.split("/"));
    if (!existsSync(path)) {
      mkdirSync(dirname(path), { recursive: true });
      writeFileSync(path, bytes);
    }
    const fileEntry = { url: relativeUrl, sha256: digest, size: bytes.length };
    entry.files[key] = fileEntry;
    present[key] = fileEntry;
    changed = true;
  }
  if (changed) {
    regenerateBloom(entry);
    writeManifest(manifest);
  }
  return { present, absent };
}

/// The collections a document not in the corpus is likely to need before it
/// meets a package `--texlive`/`wasmtex-record.mjs` never happened to fetch --
/// `examples/standard-errors.tex`'s `amssymb` is exactly this: the bloom
/// filter marks any unrecorded name absent by design (see `bloom.mjs`), so a
/// mirror built only from what the corpus asked for is a mirror that
/// silently 404s on the next ordinary document. `latex`/`basic` carry the
/// LaTeX kernel and font metrics (`amsfonts`, the Computer Modern family)
/// that even a document with no `\usepackage` at all needs; the rest matches
/// `scheme.mjs`'s own `SCHEME` for the SwiftLaTeX mirror, so the two mirrors
/// agree on what "a normal document" means.
export const DEFAULT_SCHEME = ["basic", "latex", ...SCHEME];

/// A tiny fixed-size concurrency pool: runs `worker(item)` for every item in
/// `items`, never more than `limit` in flight. No dependency for something
/// this small, and simple enough to read next to the fetch loop it serves.
async function pool(items, limit, worker) {
  const queue = [...items];
  const results = [];
  const runners = Array.from({ length: Math.min(limit, queue.length) }, async () => {
    while (queue.length) {
      const item = queue.shift();
      results.push(await worker(item));
    }
  });
  await Promise.all(runners);
  return results;
}

/// Mirrors every key a set of TeX Live collections could ask for, fetched
/// from the pinned upstream snapshot (not this machine's TeX Live, unlike
/// `scheme.mjs`'s `addScheme` -- the WasmTex mirror only ever serves bytes it
/// fetched from the snapshot it is pinned to). Idempotent the same way as
/// `ensureTexlive`: a key already present or already recorded absent is not
/// asked for again. Unlike `ensureTexlive`, which regenerates the bloom
/// filter after every call so `serve.mjs --record`'s single-file fetches stay
/// cheap, this fetches the whole scheme with `CONCURRENCY` requests in flight
/// and regenerates the filter exactly once at the end -- doing it per file
/// here would mean rebuilding a filter over thousands of keys thousands of
/// times.
const SCHEME_CONCURRENCY = 16;

export async function mirrorScheme(collections) {
  const { keys, skipped } = await schemeKeys(collections, OUT);

  const manifest = readManifest();
  const section = (manifest.texlive ||= {});
  const entry = (section[SNAPSHOT] ||= { upstream: TEXLIVE_UPSTREAM, files: {}, absent: {}, initial: [] });

  const per = Object.fromEntries(collections.map((one) => [one, { files: 0, bytes: 0, absent: 0 }]));
  const todo = [...keys.entries()].filter(([key]) => !entry.files[key] && !entry.absent[key]);
  let changed = false;

  await pool(todo, SCHEME_CONCURRENCY, async ([key, { collection }]) => {
    const url = `${TEXLIVE_UPSTREAM}${key}`;
    let response;
    try {
      response = await fetch(url);
    } catch (error) {
      throw new Error(`wasmtex: fetching ${url} failed: ${error.message}`);
    }
    // Same rule as `ensureTexlive`: any status >= 400 is "not there", per
    // wasm-build/pdftex-worker.js's tryFetch.
    if (response.status >= 400) {
      entry.absent[key] = true;
      per[collection].absent += 1;
      changed = true;
      return;
    }
    if (!response.ok) {
      throw new Error(`wasmtex: ${url}: unexpected status ${response.status}`);
    }
    const bytes = Buffer.from(await response.arrayBuffer());
    const digest = sha256(bytes);
    const name = key.slice(key.lastIndexOf("/") + 1);
    const relativeUrl = `texlive/${SNAPSHOT}/${digest.slice(0, 2)}/${digest.slice(0, 16)}-${name}`;
    const path = join(OUT, ...relativeUrl.split("/"));
    if (!existsSync(path)) {
      mkdirSync(dirname(path), { recursive: true });
      writeFileSync(path, bytes);
    }
    entry.files[key] = { url: relativeUrl, sha256: digest, size: bytes.length };
    per[collection].files += 1;
    per[collection].bytes += bytes.length;
    changed = true;
  });

  if (changed) {
    regenerateBloom(entry);
    writeManifest(manifest);
  }

  // updmap generates this map; it is not a package-owned file in tlpdb.
  // Fonts can all be present yet pdfTeX cannot embed any of them without it.
  // Fetch the snapshot's generated map on every scheme build, including
  // incremental builds, and require it rather than accepting an absent key.
  const support = await ensureTexlive(["pdftex/11/pdftex.map"]);
  if (!support.present["pdftex/11/pdftex.map"]) {
    throw new Error("wasmtex: the snapshot lacks its required generated pdftex.map");
  }

  for (const [name, tally] of Object.entries(per)) {
    console.log(
      `wasmtex: scheme ${name.padEnd(18)} ${tally.files} files, ${mb(tally.bytes)}, ${tally.absent} absent`,
    );
  }
  const totalFiles = Object.values(per).reduce((sum, one) => sum + one.files, 0);
  const totalBytes = Object.values(per).reduce((sum, one) => sum + one.bytes, 0);
  const totalAbsent = Object.values(per).reduce((sum, one) => sum + one.absent, 0);
  console.log(
    `wasmtex: scheme total ${totalFiles} files fetched, ${mb(totalBytes)}, ${totalAbsent} absent, ` +
      `${keys.size - todo.length} already known, ${skipped} pdfTeX has no format code for`,
  );
  return { per, totalFiles, totalBytes, totalAbsent };
}

/// Fetches files the workers ask for at the snapshot's root rather than
/// under an engine/format pair -- XeTeX's ICU data (`icudt68l.dat`) is the
/// one that matters -- and keeps them at that literal path under the
/// snapshot directory, which is immutable, so the static route serves them
/// with no lookup. Recorded under `texlive[<snapshot>].root`.
export async function ensureTexliveRoot(names) {
  const manifest = readManifest();
  const section = (manifest.texlive ||= {});
  const entry = (section[SNAPSHOT] ||= { upstream: TEXLIVE_UPSTREAM, files: {}, absent: {}, initial: [] });
  const root = (entry.root ||= {});
  let changed = false;
  for (const name of new Set(names)) {
    if (!/^[A-Za-z0-9._-]+$/.test(name)) throw new Error(`wasmtex: not a root file name: ${name}`);
    if (root[name]) continue;
    const bytes = await fetchBytes(`${TEXLIVE_UPSTREAM}${name}`);
    const relativeUrl = `texlive/${SNAPSHOT}/${name}`;
    const path = join(OUT, ...relativeUrl.split("/"));
    mkdirSync(dirname(path), { recursive: true });
    writeFileSync(path, bytes);
    root[name] = { url: relativeUrl, sha256: sha256(bytes), size: bytes.length };
    changed = true;
    console.log(`wasmtex: root ${name} ${mb(bytes.length)}`);
  }
  if (changed) writeManifest(manifest);
  return root;
}

/// Registers a Biber VM release built by `latex/tools/biber-vm/build.mjs`
/// under this release: the descriptor's digest is what the browser verifies
/// before it boots anything.
export function registerVm(dir) {
  const descriptorPath = join(dir, "vm.json");
  if (!existsSync(descriptorPath)) throw new Error(`wasmtex: no vm.json in ${dir}`);
  const bytes = readFileSync(descriptorPath);
  const descriptor = JSON.parse(bytes.toString("utf8"));
  const manifest = readManifest();
  const release = manifest.releases?.[manifest.default_release];
  if (!release) throw new Error("wasmtex: mirror the release before registering a VM");
  const id = basename(dir);
  release.vm = { id, url: `biber-vm/${id}/vm.json`, sha256: sha256(bytes), size: bytes.length, biber: descriptor.biber };
  release.digest = canonicalDigest({ ...release, digest: undefined });
  writeManifest(manifest);
  console.log(`wasmtex: vm ${id} (biber ${descriptor.biber}) registered on ${manifest.default_release}`);
}

/// Regenerates `texlive/<snapshot>/bloom-filter.v2.bin` over every key
/// currently recorded present, and self-tests it: every present key must
/// test positive against the freshly built bytes, or the build is wrong.
function regenerateBloom(entry) {
  const keys = Object.keys(entry.files);
  const bytes = buildBloom(keys);
  const { ok, missing } = verifyBloom(bytes, keys);
  if (!ok) {
    throw new Error(`wasmtex: bloom filter self-test failed for ${missing.length} key(s): ${missing.slice(0, 5).join(", ")}`);
  }
  const path = join(OUT, "texlive", SNAPSHOT, "bloom-filter.v2.bin");
  mkdirSync(dirname(path), { recursive: true });
  writeFileSync(path, bytes);
  entry.bloom = {
    url: `texlive/${SNAPSHOT}/bloom-filter.v2.bin`,
    sha256: sha256(bytes),
    size: bytes.length,
  };
}

/* ------------------------------------------------------- 3. initial set */

function setInitial(keys) {
  const manifest = readManifest();
  const entry = manifest.texlive?.[SNAPSHOT];
  if (!entry) throw new Error("wasmtex: no texlive entry yet; run --texlive first");
  const missing = keys.filter((key) => !entry.files[key]);
  if (missing.length) {
    throw new Error(`wasmtex: --initial names keys never fetched: ${missing.join(", ")}`);
  }
  entry.initial = [...new Set(keys)];
  const bytes = entry.initial.reduce((sum, key) => sum + entry.files[key].size, 0);
  const release = manifest.releases?.[manifest.default_release];
  if (release) {
    release.sizes.texlive_initial = bytes;
    release.digest = canonicalDigest({ ...release, digest: undefined });
  }
  writeManifest(manifest);
  console.log(`wasmtex: initial set is ${entry.initial.length} keys, ${mb(bytes)}`);
}

/* --------------------------------------------------------------------- run */

const mb = (bytes) => `${(bytes / 1e6).toFixed(1)} MB`;

function flag(argv, name) {
  const at = argv.indexOf(name);
  return at < 0 ? -1 : at;
}

async function main() {
  const argv = process.argv.slice(2);
  const texliveAt = flag(argv, "--texlive");
  const texliveFromAt = flag(argv, "--texlive-from");
  const initialAt = flag(argv, "--initial");
  const rootAt = flag(argv, "--texlive-root");
  const vmAt = flag(argv, "--vm");
  const schemeAt = flag(argv, "--scheme");
  const releaseAt = flag(argv, "--release");
  const shaAt = flag(argv, "--sha256");
  const cuts = [releaseAt, shaAt, texliveAt, texliveFromAt, initialAt, rootAt, vmAt, schemeAt]
    .filter((i) => i >= 0)
    .sort((a, b) => a - b);
  const restAfter = (at) => {
    if (at < 0) return [];
    const nextCut = cuts.find((c) => c > at);
    return argv.slice(at + 1, nextCut === undefined ? undefined : nextCut);
  };

  if (releaseAt >= 0 || shaAt >= 0) {
    await mirrorRelease(restAfter(releaseAt)[0], restAfter(shaAt)[0]);
  }

  if (texliveAt < 0 && texliveFromAt < 0 && initialAt < 0 && rootAt < 0 && vmAt < 0 && schemeAt < 0) {
    if (releaseAt < 0 && shaAt < 0) await mirrorRelease();
    return;
  }

  if (schemeAt >= 0) {
    const named = restAfter(schemeAt);
    await mirrorScheme(named.length ? named : DEFAULT_SCHEME);
  }

  if (texliveAt >= 0) {
    const keys = restAfter(texliveAt);
    const { present, absent } = await ensureTexlive(keys);
    console.log(`wasmtex: texlive ${Object.keys(present).length} fetched/known, ${Object.keys(absent).length} absent`);
  }
  if (texliveFromAt >= 0) {
    const [path] = restAfter(texliveFromAt);
    if (!path) throw new Error("wasmtex: --texlive-from needs a file path");
    const data = JSON.parse(readFileSync(path, "utf8"));
    const keys = Array.isArray(data) ? data : [...new Set(Object.values(data).flat())];
    const { present, absent } = await ensureTexlive(keys);
    console.log(`wasmtex: texlive-from ${path}: ${Object.keys(present).length} fetched/known, ${Object.keys(absent).length} absent`);
  }
  if (rootAt >= 0) {
    await ensureTexliveRoot(restAfter(rootAt));
  }
  if (initialAt >= 0) {
    setInitial(restAfter(initialAt));
  }
  if (vmAt >= 0) {
    const [dir] = restAfter(vmAt);
    if (!dir) throw new Error("wasmtex: --vm needs the built VM release directory");
    registerVm(dir);
  }
}

if (process.argv[1] && process.argv[1].endsWith("wasmtex.mjs")) {
  main().catch((error) => {
    console.error(error.message);
    process.exit(1);
  });
}
