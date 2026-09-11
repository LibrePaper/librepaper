// Reject an incomplete or legacy mirror before starting a deployment.
//
// The mirror itself -- engines and TeX Live packages -- is built and pushed
// from the wasm-latex repository (`make mirror`, `make push`; layout and
// manifest in wasm-latex/docs/mirror.md, format 1, bundled releases only, no
// per-file TeX Live snapshot). This check is the
// consumer side: it verifies whatever `--latex-mirror <url>` or `MIRROR=` names is
// actually a complete format-1 mirror before LibrePaper is pointed at it.
import { readFile } from "node:fs/promises";
import { createHash } from "node:crypto";
import { resolve } from "node:path";

// Keep the default in step with
// crates/librepaper/src/server/latex.rs::DEFAULT_MIRROR.
const base = process.argv[2] || "https://latex.librepaper.workers.dev/";
const sha256 = (bytes) => createHash("sha256").update(bytes).digest("hex");

function checkShape(manifest) {
  if (manifest.format !== 1) throw new Error(`unsupported manifest format: ${manifest.format}`);
  const release = manifest.releases?.[manifest.default_release];
  if (!release?.engines?.pdftex?.worker) throw new Error("no default release with a complete pdfTeX engine");
  if (!release.bundles) throw new Error(`release ${release.id || manifest.default_release} has no bundles; this mirror ships bundled releases only`);
  return release;
}

try {
  let manifest;
  if (/^https?:\/\//.test(base)) {
    const response = await fetch(`${base.replace(/\/$/, "")}/manifest.json`, {
      signal: AbortSignal.timeout(30000),
      cache: "no-store",
    });
    if (!response.ok) throw new Error(`manifest.json returned HTTP ${response.status}`);
    manifest = await response.json();
    checkShape(manifest);
    console.log(`LaTeX mirror ready: ${base} (${manifest.default_release}, format ${manifest.format})`);
  } else {
    manifest = JSON.parse(await readFile(resolve(base, "manifest.json"), "utf8"));
    const release = checkShape(manifest);

    // Every engine file present on disk with matching digest and size.
    for (const [name, spec] of Object.entries(release.engines)) {
      for (const file of spec.files || []) {
        const info = release.files?.[file];
        if (!info) throw new Error(`engine ${name}: file ${file} is absent from the manifest`);
        const path = resolve(base, info.url);
        if (!path.startsWith(resolve(base) + "/")) throw new Error(`asset path escapes the mirror: ${info.url}`);
        const bytes = await readFile(path);
        if (bytes.length !== info.size || sha256(bytes) !== info.sha256) {
          throw new Error(`engine ${name}: digest or size mismatch: ${info.url}`);
        }
      }
    }

    // SPEC-latex.md "The index": bundles.json's own digest must match what
    // the release entry pins, and every bundle path it names must actually
    // be on disk -- independent of `release.files`, which only proves the
    // engine files landed intact, not that bundles.json still agrees with
    // the tars beside it.
    const indexPath = resolve(base, release.bundles.index);
    const indexBytes = await readFile(indexPath);
    if (sha256(indexBytes) !== release.bundles.sha256) {
      throw new Error(`bundle index digest mismatch: ${release.bundles.index}`);
    }
    const index = JSON.parse(indexBytes.toString("utf8"));
    const bundleDir = release.bundles.index.slice(0, release.bundles.index.lastIndexOf("/") + 1);
    const bundleNames = Object.keys(index.bundles || {});
    if (bundleNames.length !== release.bundles.count) {
      throw new Error(`release.bundles.count is ${release.bundles.count} but the index names ${bundleNames.length}`);
    }
    const bundleEntries = Object.entries(index.bundles || {});
    for (let i = 0; i < bundleEntries.length; i += 32) {
      await Promise.all(bundleEntries.slice(i, i + 32).map(async ([name, bundle]) => {
        const path = resolve(base, bundleDir + bundle.url);
        if (!path.startsWith(resolve(base) + "/")) throw new Error(`bundle path escapes the mirror: ${bundle.url}`);
        let bytes;
        try {
          bytes = await readFile(path);
        } catch {
          throw new Error(`bundle "${name}" (${bundle.url}) is missing from the mirror`);
        }
        if (bytes.length !== bundle.size || sha256(bytes) !== bundle.sha256) {
          throw new Error(`bundle asset size or digest mismatch: ${bundle.url}`);
        }
      }));
    }
    for (const bundleName of new Set(Object.values(index.files || {}))) {
      if (!index.bundles?.[bundleName]) throw new Error(`bundles.json names unknown bundle "${bundleName}" in its files map`);
    }

    console.log(`LaTeX mirror ready: ${base} (${manifest.default_release}, format ${manifest.format})`);
  }
} catch (error) {
  console.error(`LaTeX mirror unavailable at ${base}: ${error.message}\n` +
    "The mirror is built and pushed from the wasm-latex repository: `make mirror` there, then `make push`.\n" +
    "Point LibrePaper at it with `make deploy LATEX_MIRROR=<mirror URL>` or `librepaper serve --latex-mirror <url>`.");
  process.exitCode = 1;
}
