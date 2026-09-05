// A package set chosen the way a distribution chooses one.
//
// `mirror.mjs`'s package half is built from what the corpus asked for: mirror
// the four example documents, record what they fetched, stop. That is enough
// to test a compiler and not nearly enough to ship one. A fifth document that
// says `\usepackage{siunitx}` meets a 301, and the compile stops on a missing
// `.sty` with nothing the browser can do about it. So the mirror needs a set
// chosen by TeX Live collection rather than by what four files happened to
// need, and this is the `--scheme` form of `mirror.mjs` that builds it.
//
// Two sources, and they answer different questions. *Which* files a collection
// holds is in TeX Live's package database, fetched once at build time and
// cached beside the mirror. The *bytes* come from the TeX Live on this
// machine, which is where every other file in the package half came from. A
// file a collection names and this machine does not have is counted and
// skipped rather than fetched from somewhere else, and the count is printed:
// an installation missing half a collection should say so rather than quietly
// mirror half of it.

import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { basename, dirname, join } from "node:path";

/// The collections `05-SPEC-latex.md` step 3 needs before a document that is
/// not in the corpus can compile. TeX Live spells them without the hyphen the
/// spec uses: `latex-recommended` is `latexrecommended` and `science` is
/// `mathscience`.
export const SCHEME = ["latexrecommended", "latexextra", "fontsrecommended", "mathscience"];

const TLPDB = "https://mirror.ctan.org/systems/texlive/tlnet/tlpkg/texlive.tlpdb.xz";

/// SwiftLaTeX's kpathsea format code for a file, and the name the engine asks
/// for it under.
///
/// The engine builds `<endpoint>/<engine>/<format>/<name>` itself, in C, and
/// the format code is part of the key because the name is often not enough: a
/// font is asked for as `cmr10`, and only the code says whether that means
/// `cmr10.tfm` or `cmr10.vf`. These codes are not guessed. They are the ones
/// the corpus' own compiles were seen asking for, recorded in the manifest by
/// `serve.mjs --record`, which is the only authority short of reading
/// kpathsea's own table.
const FORMATS = {
  ".tfm": { format: "3", bare: true },
  ".vf": { format: "33", bare: false },
  ".bst": { format: "7", bare: true },
  ".pfb": { format: "32", bare: false },
  ".pfa": { format: "32", bare: false },
  ".map": { format: "11", bare: false },
  ".enc": { format: "44", bare: false },
};

/// Everything TeX reads as source is one format code, whatever it is called: a
/// class, a package, a font definition, a configuration, a language
/// definition, a bibliography style's LaTeX half.
const TEX_SOURCE = new Set([
  ".sty",
  ".cls",
  ".clo",
  ".def",
  ".fd",
  ".cfg",
  ".tex",
  ".ldf",
  ".cbx",
  ".bbx",
  ".lbx",
  ".dbx",
  ".mkii",
  ".sto",
  ".ltx",
  ".dfu",
  ".spl",
]);

function formatFor(name) {
  const at = name.lastIndexOf(".");
  if (at < 0) return null;
  const ext = name.slice(at);
  if (FORMATS[ext]) return FORMATS[ext];
  if (TEX_SOURCE.has(ext)) return { format: "26", bare: false };
  return null;
}

const sha256 = (bytes) => createHash("sha256").update(bytes).digest("hex");

/// The package database, fetched once and kept beside the mirror. Twenty
/// megabytes of text that changes when TeX Live does, which is not often and
/// never in the middle of a build.
async function database(out) {
  const cache = join(out, ".cache", "texlive.tlpdb");
  if (existsSync(cache)) return readFileSync(cache, "utf8");
  process.stderr.write("scheme: fetching the TeX Live package database ...");
  const response = await fetch(TLPDB, { redirect: "follow" });
  if (!response.ok) throw new Error(`${TLPDB}: ${response.status}`);
  const compressed = Buffer.from(await response.arrayBuffer());
  process.stderr.write(` ${compressed.length} bytes\n`);
  const plain = execFileSync("xz", ["-dc"], { input: compressed, maxBuffer: 1 << 29 });
  mkdirSync(dirname(cache), { recursive: true });
  writeFileSync(cache, plain);
  return plain.toString("utf8");
}

/// The database, as stanzas by package name. A stanza is `name`, then
/// `depend` lines naming other packages, then `runfiles size=N` followed by
/// one indented path per file, and stanzas are separated by a blank line.
function stanzas(text) {
  const found = new Map();
  for (const stanza of text.split("\n\n")) {
    const name = stanza.match(/^name (\S+)/m)?.[1];
    if (name) found.set(name, stanza);
  }
  return found;
}

function dependsOf(stanza) {
  return [...stanza.matchAll(/^depend (\S+)$/gm)]
    .map((match) => match[1])
    .filter((name) => !name.startsWith("collection-"));
}

function runFilesOf(stanza) {
  const lines = stanza.split("\n");
  const at = lines.findIndex((line) => line.startsWith("runfiles "));
  if (at < 0) return [];
  const files = [];
  for (let i = at + 1; i < lines.length && lines[i].startsWith(" "); i++) {
    files.push(lines[i].trim());
  }
  return files;
}

/// Mirrors every file of those collections that pdfTeX could ask for.
///
/// Idempotent in the same way as everything else here: a key already in the
/// manifest is left alone, and a file already on disk under its digested path
/// is not written again. Returns the tally per collection, so the measurement
/// is a by-product of building rather than a second pass over the tree.
///
/// A package depended on by two collections is counted under the first that
/// names it, so the per-collection rows add up to the total and no file is
/// counted twice.
export async function addScheme(collections, out, manifest, write) {
  const text = await database(out);
  const all = stanzas(text);
  const root = execFileSync("kpsewhich", ["-var-value=TEXMFDIST"], { encoding: "utf8" }).trim();

  const owner = new Map();
  for (const collection of collections) {
    const stanza = all.get(`collection-${collection}`);
    if (!stanza) throw new Error(`no collection-${collection} in the package database`);
    for (const name of dependsOf(stanza)) if (!owner.has(name)) owner.set(name, collection);
  }

  const store = (manifest.packages ||= {});
  const per = Object.fromEntries(collections.map((one) => [one, { files: 0, bytes: 0 }]));
  let skipped = 0;
  let absent = 0;
  for (const [pkg, collection] of owner) {
    const stanza = all.get(pkg);
    if (!stanza) continue;
    for (const path of runFilesOf(stanza)) {
      const name = basename(path);
      const shape = formatFor(name);
      // A file pdfTeX has no format code for -- documentation, a `.dtx`
      // source, an OpenType font only XeTeX can use -- is not a file the
      // engine can ask for, so mirroring it would be bytes nobody fetches.
      if (!shape) {
        skipped += 1;
        continue;
      }
      const key = `pdftex/${shape.format}/${shape.bare ? name.slice(0, name.lastIndexOf(".")) : name}`;
      if (store[key]) continue;
      // The database names a file by its path from the TeX Live root, and
      // `TEXMFDIST` already is `texmf-dist`.
      const full = join(root, path.replace(/^(RELOC|texmf-dist)\//, ""));
      if (!existsSync(full)) {
        absent += 1;
        continue;
      }
      const bytes = readFileSync(full);
      const digest = sha256(bytes);
      const url = `packages/pdftex/${digest.slice(0, 2)}/${digest.slice(0, 16)}-${name}`;
      const where = join(out, url);
      if (!existsSync(where)) {
        mkdirSync(dirname(where), { recursive: true });
        writeFileSync(where, bytes);
      }
      store[key] = { url, sha256: digest, size: bytes.length };
      per[collection].files += 1;
      per[collection].bytes += bytes.length;
    }
  }
  write(manifest, out);
  return { per, skipped, absent };
}
