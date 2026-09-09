#!/usr/bin/env node
// The WasmTex half of `latex/mirror`: engine release plus TeX Live package
// snapshot, mirrored under LibrePaper's own names so the browser never depends
// on an upstream project's live service (see `docs/specs/wasmtex-interfaces.md`
// section 1, the contract this file produces exactly).
//
// Three things live here, and each is idempotent -- a file already on disk
// with the right digest is not refetched, so re-running costs a manifest
// read and, for the release, one upstream manifest fetch to compare against.
//
//   node latex/tools/wasmtex.mjs
//     Mirrors the pinned 2026 engine release into
//     latex/mirror/wasmtex/<engineRelease>/, verifies every file against the
//     pinned bytes/sha256 in the evaluation manifest, copies licence notices,
//     and writes the release entry into manifest.json.
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
  cpSync,
} from "node:fs";
import { dirname, join, basename } from "node:path";
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
export const WRAPPER_REVISION = "44c5861fcdf729838205b00b96ac9509bc7fb677";
export const RELEASE_ID = `${ENGINE_RELEASE}+${SNAPSHOT}`;

const WASMTEX_UPSTREAM = "https://corca-ai.github.io/wasmtex/wasmtex/2026/";
const TEXLIVE_UPSTREAM = `https://texlive.corca.ai/snapshots/${SNAPSHOT}/2026/`;

// The evaluation manifest this release was pinned from: `bytes`/`sha256` for
// every engine file, recorded once by the browser comparison so this script
// never has to trust a live fetch's digest against itself.
const PINNED_MANIFEST = join(
  REPO,
  "latex",
  "benchmark",
  "candidates",
  "wasmtex",
  "downloads",
  "manifest-2026.json",
);

// Where the licence notices are copied from. This is the upstream source
// checkout named in docs/specs/wasmtex.md ("Own the WasmTex release"), a build-time
// input this script reads but never writes -- and, being untracked, it is not
// guaranteed to exist in every checkout. A missing checkout fails loudly with
// the revision to fetch, rather than silently skipping notices a release must
// carry.
const SOURCE_CHECKOUT = join(REPO, "latex", "benchmark", "candidates", "wasmtex", "source");

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

const ENGINE_FILE_SETS = {
  pdftex: {
    worker: "wasmtex-pdftex.worker.js",
    format: "wasmtex-pdftex.fmt",
    files: [
      "wasmtex-pdftex.worker.js",
      "wasmtex-pdftex.js",
      "wasmtex-pdftex.wasm",
      "wasmtex-pdftex-resolver-evidence.js",
      "wasmtex-kpse-resolve.js",
      "wasmtex-pdftex.fmt",
    ],
  },
  xetex: {
    worker: "wasmtex-xetex.worker.js",
    format: "wasmtex-xetex.fmt.gz",
    files: [
      "wasmtex-xetex.worker.js",
      "wasmtex-xetex.js",
      "wasmtex-xetex.wasm",
      "wasmtex-xetex-resolver-evidence.js",
      "wasmtex-xetex.fmt.gz",
    ],
  },
  dvipdfm: {
    worker: "wasmtex-dvipdfm.worker.js",
    files: ["wasmtex-dvipdfm.worker.js", "wasmtex-dvipdfm.js", "wasmtex-dvipdfm.wasm"],
  },
  luatex: {
    worker: "wasmtex-luatex.worker.js",
    format: "wasmtex-luatex.fmt.gz",
    files: [
      "wasmtex-luatex.worker.js",
      "wasmtex-luatex.js",
      "wasmtex-luatex.wasm",
      "wasmtex-luatex-resolver-evidence.js",
      "wasmtex-luatex.fmt.gz",
    ],
  },
  bibtex: {
    worker: "wasmtex-bibtex.worker.js",
    files: ["wasmtex-bibtex.worker.js", "wasmtex-bibtex.js", "wasmtex-bibtex.wasm"],
  },
  bibtex8: {
    worker: "wasmtex-bibtex8.worker.js",
    files: ["wasmtex-bibtex8.worker.js", "wasmtex-bibtex8.js", "wasmtex-bibtex8.wasm"],
  },
  makeindex: {
    worker: "wasmtex-makeindex.worker.js",
    files: ["wasmtex-makeindex.worker.js", "wasmtex-makeindex.js", "wasmtex-makeindex.wasm"],
  },
};

async function mirrorRelease() {
  if (!existsSync(PINNED_MANIFEST)) {
    throw new Error(
      `wasmtex: no pinned evaluation manifest at ${PINNED_MANIFEST}.\n` +
        "  This is the record of the release the comparison verified; without it there is\n" +
        "  nothing to check a live fetch's digests against. See\n" +
        "  latex/benchmark/candidates/comparison/README.md for how it was produced.",
    );
  }
  const pinned = JSON.parse(readFileSync(PINNED_MANIFEST, "utf8"));
  if (pinned.releaseId !== ENGINE_RELEASE) {
    throw new Error(
      `wasmtex: pinned manifest releaseId ${pinned.releaseId} does not match the release this\n` +
        `  script is built for (${ENGINE_RELEASE}). Update ENGINE_RELEASE in wasmtex.mjs deliberately;\n` +
        "  this is a decision, not something to paper over.",
    );
  }
  const pinnedFiles = new Map(pinned.files.map((f) => [f.name, f]));

  console.log(`wasmtex: checking upstream release ${WASMTEX_UPSTREAM}manifest.json ...`);
  const liveManifest = JSON.parse((await fetchBytes(`${WASMTEX_UPSTREAM}manifest.json`)).toString("utf8"));
  if (liveManifest.releaseId !== ENGINE_RELEASE) {
    throw new Error(
      `wasmtex: refusing to mirror. The live upstream manifest's releaseId is\n` +
        `  ${liveManifest.releaseId}, not the pinned ${ENGINE_RELEASE}. A new release is a deliberate\n` +
        "  re-pin (new ENGINE_RELEASE, new evaluation), never an automatic follow.",
    );
  }

  const releaseDir = `wasmtex/${ENGINE_RELEASE}`;
  const files = {};
  let totalBytes = 0;
  for (const spec of liveManifest.files) {
    const pin = pinnedFiles.get(spec.name);
    if (!pin) {
      throw new Error(`wasmtex: upstream lists ${spec.name}, which the pinned manifest never evaluated`);
    }
    const path = join(OUT, releaseDir, spec.name);
    let bytes;
    if (existsSync(path) && statSync(path).size === pin.bytes) {
      bytes = readFileSync(path);
    } else {
      bytes = await fetchBytes(`${WASMTEX_UPSTREAM}${spec.name}`);
    }
    if (bytes.length !== pin.bytes) {
      throw new Error(`wasmtex: ${spec.name} is ${bytes.length} bytes, pinned manifest says ${pin.bytes}`);
    }
    const digest = sha256(bytes);
    if (digest !== pin.sha256) {
      throw new Error(`wasmtex: ${spec.name} sha256 ${digest} does not match pinned ${pin.sha256}`);
    }
    place(join(releaseDir, spec.name), bytes);
    files[spec.name] = { url: `${releaseDir}/${spec.name}`, sha256: digest, size: bytes.length };
    totalBytes += bytes.length;
  }

  // The upstream manifest itself, kept as evidence beside the files it
  // describes -- the receipt that this mirror once agreed with upstream, not
  // something anything here reads back.
  writeFileSync(join(OUT, releaseDir, "upstream-manifest.json"), JSON.stringify(liveManifest, null, 2) + "\n");

  // Notices, copied from the source checkout. A missing checkout is refused
  // rather than silently producing a release with no NOTICES directory.
  const notices = [
    ["LICENSE", "LICENSE"],
    ["THIRD_PARTY_NOTICES.md", "THIRD_PARTY_NOTICES.md"],
    ["docs/licensing.md", "licensing.md"],
    ["docs/corresponding-source.md", "corresponding-source.md"],
  ];
  if (!existsSync(SOURCE_CHECKOUT)) {
    throw new Error(
      `wasmtex: no source checkout at ${SOURCE_CHECKOUT}.\n` +
        `  Notices cannot be copied without it. Check out WasmTex revision\n` +
        `  ${WRAPPER_REVISION} there (see docs/specs/wasmtex.md "Starting point").`,
    );
  }
  mkdirSync(join(OUT, releaseDir, "NOTICES"), { recursive: true });
  for (const [from, to] of notices) {
    cpSync(join(SOURCE_CHECKOUT, from), join(OUT, releaseDir, "NOTICES", to));
  }

  // Bibliography identity: read from the mirrored snapshot's biblatex.sty
  // rather than assumed, per section 1's "read \blx@bcfversion, biblatex
  // version/date ... from the snapshot's biblatex.sty after mirroring it".
  const bibliography = await bibliographyIdentity();

  const engines = {};
  for (const [name, spec] of Object.entries(ENGINE_FILE_SETS)) {
    engines[name] = { worker: spec.worker, files: spec.files };
    if (spec.format) engines[name].format = spec.format;
  }

  const entry = {
    id: RELEASE_ID,
    engine_release: ENGINE_RELEASE,
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
      wrapper_revision: WRAPPER_REVISION,
      corresponding_source: {
        url: liveManifest.legal.correspondingSource.url,
        sha256: liveManifest.legal.correspondingSource.sha256,
      },
      build_receipts: liveManifest.buildReceipts.map((r) => r.name),
      // Not yet: the second half of "Own the WasmTex release" is
      // independently reproducing these builds from source, which this
      // script does not attempt (see latex/tools/README.md).
      reproduced: false,
    },
    licences: {
      wrapper: "MIT",
      pdftex: "GPL-2.0-only",
      xetex: "GPL-2.0-only AND LicenseRef-XeTeX",
      luatex: "GPL-2.0-only",
      bibtex: "LicenseRef-BibTeX-Web2C-Notices AND LGPL-2.1-or-later",
      notices: `${releaseDir}/NOTICES/`,
    },
    sizes: {
      pdftex: sizeOf(files, engines.pdftex.files),
      xetex: sizeOf(files, engines.xetex.files),
      luatex: sizeOf(files, engines.luatex.files),
      texlive_initial: 0, // filled in by --texlive/--initial runs, below
    },
  };

  const manifest = readManifest();
  manifest.version = 1;
  manifest.releases ||= {};
  manifest.default_release = RELEASE_ID;
  // texlive_initial is measured from what --initial names, which may run
  // after this in the same invocation or in a later one; preserve it rather
  // than resetting to 0 on a re-run of the release step alone.
  const previous = manifest.releases[RELEASE_ID];
  if (previous?.sizes?.texlive_initial) entry.sizes.texlive_initial = previous.sizes.texlive_initial;
  // The VM release is registered by `--vm` (or the biber-vm build) and is
  // part of this release's identity; a rebuild of the engine half must not
  // silently drop it.
  if (previous?.vm) entry.vm = previous.vm;
  entry.digest = canonicalDigest({ ...entry, digest: undefined });
  manifest.releases[RELEASE_ID] = entry;
  writeManifest(manifest);

  console.log(`wasmtex: release ${RELEASE_ID} mirrored, ${Object.keys(files).length} files, ${mb(totalBytes)}`);
  console.log(`wasmtex: bibliography bibtex ${bibliography.bibtex}, biblatex ${bibliography.biblatex}, bcf ${bibliography.control_file}`);
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
  const release = manifest.releases?.[RELEASE_ID];
  if (!release) throw new Error("wasmtex: mirror the release before registering a VM");
  const id = basename(dir);
  release.vm = { id, url: `biber-vm/${id}/vm.json`, sha256: sha256(bytes), size: bytes.length, biber: descriptor.biber };
  release.digest = canonicalDigest({ ...release, digest: undefined });
  writeManifest(manifest);
  console.log(`wasmtex: vm ${id} (biber ${descriptor.biber}) registered on ${RELEASE_ID}`);
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
  const release = manifest.releases?.[RELEASE_ID];
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
  const cuts = [texliveAt, texliveFromAt, initialAt, rootAt, vmAt, schemeAt]
    .filter((i) => i >= 0)
    .sort((a, b) => a - b);
  const restAfter = (at) => {
    if (at < 0) return [];
    const nextCut = cuts.find((c) => c > at);
    return argv.slice(at + 1, nextCut === undefined ? undefined : nextCut);
  };

  if (texliveAt < 0 && texliveFromAt < 0 && initialAt < 0 && rootAt < 0 && vmAt < 0 && schemeAt < 0) {
    await mirrorRelease();
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
