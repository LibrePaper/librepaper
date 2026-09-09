#!/usr/bin/env node
// The mirror: every file a distribution needs, under our own name.
//
// The browser never fetches from a distribution's own project site at
// runtime, for two reasons -- the list of packages a document
// asks for is a description of the document, and a third-party endpoint that
// goes away takes every LaTeX document on every deployment with it. This
// script is what makes that true: it fetches each distribution's release
// once, writes the files under a local directory in the layout the browser
// will fetch, and writes a manifest naming every file with its SHA-256 and
// size, with the digest in the URL as `typst.wasm`'s is. Cached forever is
// then safe, and an updated distribution is a new manifest rather than a
// cache to invalidate.
//
// It is idempotent: a file already present with the right digest is not
// fetched again, so re-running costs a manifest read.
//
// Nothing here uploads anything anywhere. The mirror is a directory; what
// goes into a bucket is a decision made elsewhere, with the byte totals this
// script prints in hand.
//
//     node latex/tools/mirror.mjs [--out latex/mirror] [--only <distribution>]
//     node latex/tools/mirror.mjs --packages <name>...   # add SwiftLaTeX packages
//     node latex/tools/mirror.mjs --scheme [<collection>...]  # a whole package set
//
// The second form is the answer to a problem the spec did not foresee.
// SwiftLaTeX's engines do not carry TeX Live at all. They call out to a
// package endpoint, one file at a time, over synchronous XHR, while the
// compile runs -- and not only for packages: the LaTeX format itself,
// `swiftlatexpdftex.fmt`, ten megabytes of it, is the first thing fetched,
// before a single line is typeset. SwiftLaTeX's own endpoint,
// `texlive2.swiftlatex.com`, answers 522 and has for some time. TeXlyre keeps
// one alive speaking the same protocol, and that is where this mirrors from,
// once, at build time; a TeX Live on this machine answers for anything it
// does not have. Either way the browser fetches from one base URL, which is
// the rule the spec actually cares about.
//
// `--packages` adds files by name; `serve.mjs --record` adds whatever a
// compile asked for and did not find, which is how the list was collected.

import { createHash } from "node:crypto";
import { execFileSync } from "node:child_process";
import {
  existsSync,
  mkdirSync,
  readFileSync,
  writeFileSync,
  statSync,
} from "node:fs";
import { dirname, join, extname, basename } from "node:path";
import {
  DISTRIBUTIONS,
  SWIFTLATEX_RELEASE,
  BUSYTEX_RELEASE,
  TEXLYRE_RELEASE,
} from "./distributions.mjs";
import { unzip } from "./unzip.mjs";
import { SCHEME, addScheme } from "./scheme.mjs";

const HERE = dirname(new URL(import.meta.url).pathname);
// This file is latex/tools/mirror.mjs, so the repository is two directories up.
const REPO = dirname(dirname(HERE));

/* --------------------------------------------------------------- arguments */

const argv = process.argv.slice(2);
const flag = (name, fallback) => {
  const at = argv.indexOf(name);
  return at < 0 ? fallback : argv[at + 1];
};
const OUT = flag("--out", join(REPO, "latex", "mirror"));
const ONLY = flag("--only", null);
const PACKAGES = argv.indexOf("--packages");
const SCHEME_AT = argv.indexOf("--scheme");

/* ----------------------------------------------------------- the manifest */

const MANIFEST = join(OUT, "manifest.json");

/// The manifest as it stands, or an empty one. It is the mirror's index and
/// the only file the browser fetches by a name without a digest in it.
export function readManifest(out = OUT) {
  const path = join(out, "manifest.json");
  if (!existsSync(path)) return { version: 1, distributions: {} };
  return JSON.parse(readFileSync(path, "utf8"));
}

// Used by `serve.mjs --record` to write back the `texlive.<snapshot>` section
// (see `wasmtex.mjs`, which owns the rest of that shape) without duplicating
// the write-and-newline convention every manifest writer here follows.
export function writeManifest(manifest, out = OUT) {
  mkdirSync(out, { recursive: true });
  writeFileSync(join(out, "manifest.json"), JSON.stringify(manifest, null, 2) + "\n");
}

const sha256 = (bytes) => createHash("sha256").update(bytes).digest("hex");

/// Writes one file into the mirror and returns its manifest entry.
///
/// The digest goes in a directory rather than in the filename, which is the
/// one place this differs from `typst.wasm`'s URL, and it differs for a
/// reason. Emscripten's loaders find their payload by name relative to
/// themselves: `busytex.js` asks for `busytex.wasm` beside it and
/// `texlive-basic.js` for `texlive-basic.data`, and neither will ask for a
/// name with a digest in it. So the whole release goes in one directory named
/// by a digest over all of it, the files inside keep the names their loaders
/// expect, and the URL still carries a digest -- which is what makes cached
/// forever safe. A new release is a new directory, never an overwrite.
///
/// A file already on disk at its digested path is left alone: the path names
/// the bytes, so its being there is proof enough.
function place(distribution, release, name, bytes) {
  const digest = sha256(bytes);
  const url = `${distribution}/${release}/${name}`;
  const path = join(OUT, url);
  if (!existsSync(path) || statSync(path).size !== bytes.length) {
    mkdirSync(dirname(path), { recursive: true });
    writeFileSync(path, bytes);
  }
  return { url, sha256: digest, size: bytes.length };
}

/// The name of a release's directory: a digest over every file in it, so that
/// changing any one file moves all of them and no half-updated release can be
/// assembled out of two caches.
function releaseDigest(files) {
  const lines = [...files.entries()]
    .map(([name, bytes]) => `${name} ${sha256(bytes)}`)
    .sort();
  return sha256(lines.join("\n")).slice(0, 16);
}

/* ----------------------------------------------------------------- fetching */

async function download(url, what) {
  process.stderr.write(`mirror: fetching ${what} ...`);
  const response = await fetch(url, { redirect: "follow" });
  if (!response.ok) throw new Error(`${url}: ${response.status} ${response.statusText}`);
  const bytes = Buffer.from(await response.arrayBuffer());
  process.stderr.write(` ${bytes.length} bytes\n`);
  return bytes;
}

// The SwiftLaTeX zip is fetched at most once per run and only if a file it
// holds is actually missing; it is six megabytes to get two.
let swiftlatexZip = null;
async function fromZip(entry) {
  if (!swiftlatexZip) {
    const cache = join(OUT, ".cache", `swiftlatex-${SWIFTLATEX_RELEASE.tag}.zip`);
    if (existsSync(cache)) {
      swiftlatexZip = unzip(readFileSync(cache));
    } else {
      const bytes = await download(SWIFTLATEX_RELEASE.url, `SwiftLaTeX ${SWIFTLATEX_RELEASE.tag}`);
      mkdirSync(dirname(cache), { recursive: true });
      writeFileSync(cache, bytes);
      swiftlatexZip = unzip(bytes);
    }
  }
  const bytes = swiftlatexZip.get(entry);
  if (!bytes) throw new Error(`SwiftLaTeX release has no ${entry}`);
  return bytes;
}

// Kept under `.cache` beside the mirror rather than fetched again: BusyTeX's
// release is two hundred megabytes and the layout under it may yet change,
// and re-deciding the layout should not cost the network twice.
async function fromBusytex(entry) {
  const cache = join(OUT, ".cache", BUSYTEX_RELEASE.tag, entry);
  if (existsSync(cache)) return readFileSync(cache);
  const bytes = await download(`${BUSYTEX_RELEASE.base}/${entry}`, `busytex ${entry}`);
  mkdirSync(dirname(cache), { recursive: true });
  writeFileSync(cache, bytes);
  return bytes;
}

// TeXlyre's build is one tar.gz of everything -- half a gigabyte compressed,
// seven hundred megabytes out -- so it is fetched once into `.cache` and
// unpacked once beside itself, and the files are read out of that directory
// afterwards. Unpacked with the `tar` already on the machine rather than with
// a reader written here, unlike `unzip.mjs`: a gzip stream cannot be seeked,
// so taking five files out of it costs decompressing all of it either way,
// and at this size doing that on disk beats doing it in a Buffer.
async function fromTexlyre(entry) {
  const root = join(OUT, ".cache", TEXLYRE_RELEASE.tag);
  const path = join(root, TEXLYRE_RELEASE.prefix, entry);
  if (!existsSync(path)) {
    const archive = join(root, "busytex-assets.tar.gz");
    if (!existsSync(archive)) {
      const bytes = await download(TEXLYRE_RELEASE.url, `texlyre-busytex ${TEXLYRE_RELEASE.tag}`);
      mkdirSync(dirname(archive), { recursive: true });
      writeFileSync(archive, bytes);
    }
    process.stderr.write(`mirror: unpacking ${TEXLYRE_RELEASE.tag} ...`);
    execFileSync("tar", ["-xzf", archive, "-C", root]);
    process.stderr.write(" done\n");
  }
  if (!existsSync(path)) throw new Error(`texlyre-busytex release has no ${entry}`);
  return readFileSync(path);
}

/// Where a file named by a distribution's `upfront` or `extra` comes from.
const SOURCES = { zip: fromZip, busytex: fromBusytex, texlyre: fromTexlyre };

/* ------------------------------------------------ SwiftLaTeX's package half */

// SwiftLaTeX asks its endpoint for `<engine>/<format>/<name>`, where <format>
// is kpathsea's numeric format code and <name> is a bare filename with no
// directory in it -- and often with no extension either: a font is asked for
// as `cmr10`, and only the format code says whether that means `cmr10.tfm` or
// `cmr10.pfb`. So the format code is part of the key, here and upstream. The
// engine wants a `fileid` header back and treats a 301 -- not a 404 -- as "no
// such file"; `serve.mjs` supplies both from what is written here.

/// Where a mirror of the SwiftLaTeX package protocol can still be reached.
/// Used at build time by this script and never by a browser.
export const SWIFTLATEX_UPSTREAM = "https://texlive.texlyre.org";

function kpsewhich(name) {
  try {
    const found = execFileSync("kpsewhich", ["--", name], { encoding: "utf8" }).trim();
    return found && existsSync(found) ? found : null;
  } catch {
    return null;
  }
}

/// Adds package files to the mirror by name, from the upstream endpoint if it
/// answers and from a TeX Live on this machine if it does not. Returns what it
/// could and could not find, so a caller can say which package a document
/// wanted and did not get rather than leaving a compile to fail with nothing
/// to point at. A name recorded as missing is remembered, so a document that
/// asks for a font that does not exist is not re-fetched on every run.
///
/// Where a file came from matters, and the manifest says: `from` is
/// `upstream` for the TeX Live that matches the engine's preloaded format,
/// and `local` for the one on this machine. The two disagree on any package
/// that has changed since the format was built -- `amsmath` from a 2025 TeX
/// Live calls a kernel command a 2020 format has never heard of, and the
/// compile stops there. So a `local` file is provisional: `replace` asks
/// upstream again for a name the mirror already has and keeps the upstream
/// bytes when it answers, which is what `serve.mjs --record` does for every
/// file a compile touches. A file marked `local` after a failed attempt is
/// one upstream does not have, and is not asked for again.
export async function addPackages(
  names,
  engine = "pdftex",
  format = "0",
  out = OUT,
  { replace = false } = {},
) {
  const manifest = readManifest(out);
  const store = (manifest.packages ||= {});
  const absent = (manifest.absent ||= {});
  const found = [];
  const missing = [];
  for (const name of names) {
    const key = `${engine}/${format}/${name}`;
    if (absent[key]) continue;
    const had = store[key];
    // A `from` that is missing is a manifest written before the field
    // existed, which was built local-first and is treated as local.
    if (had && !(replace && had.from !== "upstream")) continue;
    let bytes = null;
    let from = "upstream";
    try {
      const response = await fetch(`${SWIFTLATEX_UPSTREAM}/${engine}/${format}/${name}`);
      if (response.ok) bytes = Buffer.from(await response.arrayBuffer());
    } catch {
      /* fall through to the TeX Live on this machine */
    }
    if (!bytes && had) {
      // Upstream has no better answer; what is here stays, and is not asked
      // about again.
      store[key] = { ...had, from: "local" };
      writeManifest(manifest, out);
      continue;
    }
    if (!bytes) {
      const path = kpsewhich(name);
      if (path) {
        bytes = readFileSync(path);
        from = "local";
      }
    }
    if (!bytes) {
      absent[key] = true;
      missing.push(name);
      continue;
    }
    const digest = sha256(bytes);
    const url = `packages/${engine}/${digest.slice(0, 2)}/${digest.slice(0, 16)}-${name}`;
    const full = join(out, url);
    if (!existsSync(full)) {
      mkdirSync(dirname(full), { recursive: true });
      writeFileSync(full, bytes);
    }
    store[key] = { url, sha256: digest, size: bytes.length, from };
    found.push(name);
  }
  if (found.length || missing.length) writeManifest(manifest, out);
  return { found, missing };
}

/* --------------------------------------------------------------------- run */

async function mirror() {
  const manifest = readManifest();
  manifest.version = 1;
  manifest.distributions ||= {};
  manifest.packages ||= {};

  for (const spec of DISTRIBUTIONS) {
    if (ONLY && ONLY !== spec.name) continue;
    const entry = (manifest.distributions[spec.name] ||= {});
    entry.label = spec.label;
    // Written on every run rather than only on a fetch, so flipping `shown` in
    // `distributions.mjs` and re-running is enough to put one on the card.
    entry.shown = spec.shown === true;
    entry.measured = spec.measured || null;
    entry.engines = spec.engines;
    entry.bibliography = spec.bibliography;
    entry.licence = spec.licence;
    entry.trade = spec.trade;
    entry.packages = spec.packages;
    entry.files ||= {};
    entry.extra ||= {};
    entry.bundles ||= {};

    // Idempotence: a release whose every file is on disk under its digested
    // directory is not fetched again. That is the whole of the check, because
    // the directory is the digest of what is in it.
    const wanted = [
      ...spec.upfront.map((file) => file.as || file.entry),
      ...(spec.extra || []),
      ...(spec.bundles || []).flatMap((b) => [b + ".js", b + ".data"]),
    ];
    const complete =
      entry.release &&
      wanted.every((name) => existsSync(join(OUT, `${spec.name}/${entry.release}/${name}`)));
    // Every file of one distribution comes from one release, so the source
    // kind its up-front files name is the source kind for its bundles and its
    // extras too.
    const from = SOURCES[spec.upfront[0].from];
    if (!from) throw new Error(`${spec.name}: no source kind ${spec.upfront[0].from}`);

    if (!complete) {
      const bytes = new Map();
      for (const file of spec.upfront) {
        bytes.set(file.as || file.entry, await from(file.entry));
      }
      for (const name of spec.extra || []) bytes.set(name, await from(name));
      for (const bundle of spec.bundles || []) {
        for (const suffix of [".js", ".data"]) {
          bytes.set(bundle + suffix, await from(bundle + suffix));
        }
      }
      entry.release = releaseDigest(bytes);
      entry.files = {};
      entry.extra = {};
      entry.bundles = {};
      for (const file of spec.upfront) {
        const name = file.as || file.entry;
        entry.files[name] = place(spec.name, entry.release, name, bytes.get(name));
      }
      // Kept out of `files` on purpose: `files` is what is fetched before a
      // first compile can begin, and the `upfront` total below is its sum. An
      // extra is placed in the same release directory -- its loader looks for
      // its payload beside itself like every other -- but is fetched only when
      // a compile turns out to need it, so counting it up front would be a
      // lie on the card.
      for (const name of spec.extra || []) {
        entry.extra[name] = place(spec.name, entry.release, name, bytes.get(name));
      }
      for (const bundle of spec.bundles || []) {
        entry.bundles[bundle] = {
          ".js": place(spec.name, entry.release, bundle + ".js", bytes.get(bundle + ".js")),
          ".data": place(spec.name, entry.release, bundle + ".data", bytes.get(bundle + ".data")),
        };
      }
    }

    entry.upfront = Object.values(entry.files).reduce((sum, one) => sum + one.size, 0);
    entry.bundle_bytes =
      Object.values(entry.bundles).reduce(
        (sum, pair) => sum + Object.values(pair).reduce((a, b) => a + b.size, 0),
        0,
      ) + Object.values(entry.extra).reduce((sum, one) => sum + one.size, 0);
    writeManifest(manifest);
  }

  // A whole package set, by TeX Live collection rather than by what the
  // corpus asked for. See `scheme.mjs` for why the mirror needs one and where
  // the two halves of the answer come from.
  if (SCHEME_AT >= 0) {
    const named = argv.slice(SCHEME_AT + 1).filter((one) => !one.startsWith("--"));
    const collections = named.length ? named : SCHEME;
    const { per, skipped, absent } = await addScheme(collections, OUT, readManifest(), writeManifest);
    for (const [name, tally] of Object.entries(per)) {
      console.log(`mirror: ${name.padEnd(18)} ${tally.files} files, ${mb(tally.bytes)}`);
    }
    const files = Object.values(per).reduce((sum, one) => sum + one.files, 0);
    const bytes = Object.values(per).reduce((sum, one) => sum + one.bytes, 0);
    console.log(`mirror: ${"scheme total".padEnd(18)} ${files} files, ${mb(bytes)}`);
    console.log(
      `mirror: ${skipped} files pdfTeX has no format code for, ${absent} not installed here`,
    );
  }

  if (PACKAGES >= 0) {
    const rest = argv.slice(PACKAGES + 1);
    const engine = rest[0] === "pdftex" || rest[0] === "xetex" ? rest.shift() : "pdftex";
    const format = /^\d+$/.test(rest[0]) ? rest.shift() : "0";
    // `--replace` asks upstream again for names the mirror already has from
    // the TeX Live on this machine.
    const replace = rest.includes("--replace");
    const names = rest.filter((name) => name !== "--replace");
    const { found, missing } = await addPackages(names, engine, format, OUT, { replace });
    console.log(`mirror: ${found.length} package files added, ${missing.length} not on this machine`);
    if (missing.length) console.log(`mirror: missing ${missing.join(" ")}`);
  }

  const final = readManifest();
  console.log("");
  for (const [name, entry] of Object.entries(final.distributions)) {
    const bundles = entry.bundle_bytes ? `, ${mb(entry.bundle_bytes)} in on-demand bundles` : "";
    console.log(`mirror: ${name.padEnd(18)} ${mb(entry.upfront)} up front${bundles}`);
  }
  const packages = Object.values(final.packages || {});
  if (packages.length) {
    const bytes = packages.reduce((sum, one) => sum + one.size, 0);
    console.log(`mirror: ${"packages".padEnd(18)} ${packages.length} files, ${mb(bytes)}`);
  }
  // The WasmTex half, built by `wasmtex.mjs` into the same manifest: printed
  // here too so `mirror.mjs` alone still shows the whole mirror's shape.
  for (const [id, release] of Object.entries(final.releases || {})) {
    const marker = id === final.default_release ? " (default)" : "";
    console.log(`mirror: ${("release " + id).padEnd(18)} ${mb(release.sizes.pdftex)} pdftex engine+format${marker}`);
  }
  for (const [snapshot, entry] of Object.entries(final.texlive || {})) {
    const files = Object.values(entry.files || {});
    const bytes = files.reduce((sum, one) => sum + one.size, 0);
    console.log(
      `mirror: ${("texlive " + snapshot).padEnd(18)} ${files.length} files, ${mb(bytes)}, ` +
        `${Object.keys(entry.absent || {}).length} recorded absent`,
    );
  }
  console.log(`mirror: written to ${OUT}`);
}

const mb = (bytes) => `${(bytes / 1e6).toFixed(1)} MB`;

if (process.argv[1] && process.argv[1].endsWith("mirror.mjs")) {
  mirror().catch((error) => {
    console.error(`mirror: ${error.message}`);
    process.exit(1);
  });
}
