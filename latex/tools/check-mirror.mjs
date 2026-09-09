// Reject legacy mirrors before starting a deployment or uploading its assets.
import { readFile } from "node:fs/promises";
import { createHash } from "node:crypto";
import { resolve } from "node:path";
import { verifyBloom } from "./bloom.mjs";

// Keep the default in step with
// crates/librepaper/src/latex.rs::DEFAULT_MIRROR.
const base = process.argv[2] || "https://librepaper-latex.vincentarelbundock.workers.dev/";
try {
  let manifest;
  if (/^https?:\/\//.test(base)) {
    const response = await fetch(`${base.replace(/\/$/, "")}/manifest.json`, {
      signal: AbortSignal.timeout(30000),
      cache: "no-store",
    });
    if (!response.ok) throw new Error(`manifest.json returned HTTP ${response.status}`);
    manifest = await response.json();
  } else {
    manifest = JSON.parse(await readFile(resolve(base, "manifest.json"), "utf8"));
  }
  const release = manifest.releases?.[manifest.default_release];
  if (!release?.engines?.pdftex?.worker) throw new Error("no default WasmTex compiler release (legacy mirrors are incompatible)");
  const snapshot = manifest.texlive?.[release.snapshot];
  // A release that ships bundles carries its own package set (SPEC-latex.md,
  // wasm-latex); the per-file snapshot is then only for engines without
  // bundle mode, and may be absent. Without bundles it is what every compile
  // reads from, so it must be complete.
  if (!release.bundles &&
      (!snapshot?.files?.["pdftex/26/article.cls"] || !snapshot?.files?.["pdftex/11/pdftex.map"] || !snapshot?.bloom?.url)) {
    throw new Error("the default release's TeX Live package set is missing; build it with make latex-mirror");
  }
  // For an upload, verify the actual bytes, not just entries in an index.
  // Remote mirrors are exercised through the browser smoke check instead
  // of downloading the entire distribution on every `make deploy`.
  if (!/^https?:\/\//.test(base)) {
    for (const engine of Object.values(release.engines)) {
      for (const name of engine.files || []) {
        if (!release.files?.[name]) throw new Error(`engine file ${name} is absent from the manifest`);
      }
    }
    const files = [
      ...Object.values(release.files || {}),
      ...Object.values(snapshot?.files || {}),
      ...Object.values(snapshot?.root || {}),
      ...(snapshot?.bloom ? [snapshot.bloom] : []),
    ];
    for (let i = 0; i < files.length; i += 32) {
      await Promise.all(files.slice(i, i + 32).map(async file => {
        const path = resolve(base, file.url);
        if (!path.startsWith(resolve(base) + "/")) throw new Error(`asset path escapes the mirror: ${file.url}`);
        const bytes = await readFile(path);
        if (bytes.length !== file.size || createHash("sha256").update(bytes).digest("hex") !== file.sha256) {
          throw new Error(`asset size or digest mismatch: ${file.url}`);
        }
      }));
    }
    if (snapshot?.bloom) {
      const bloom = await readFile(resolve(base, snapshot.bloom.url));
      const { ok, missing } = verifyBloom(bloom, Object.keys(snapshot.files));
      if (!ok) throw new Error(`package lookup filter hides ${missing.length} available files`);
    }
    for (const key of snapshot?.initial || []) {
      if (!snapshot.files?.[key]) throw new Error(`initial package ${key} is absent from the manifest`);
    }
    // SPEC-latex.md "The index": a release's `bundles.json` is verified twice
    // -- its own bytes against the digest the release entry pins, and every
    // bundle path it names against the mirror on disk. `release.files` above
    // already hashed every payload file the importer copied, bundle tars
    // included, but that only proves the files that landed are correct; it
    // says nothing about whether `bundles.json`'s own `files`/`bundles` map
    // still agrees with them. This check is cheap (bundle counts are in the
    // thousands, not millions) so it is a full check, not a spot check.
    if (release.bundles) {
      const indexPath = resolve(base, release.bundles.index);
      const indexBytes = await readFile(indexPath);
      if (createHash("sha256").update(indexBytes).digest("hex") !== release.bundles.sha256) {
        throw new Error("bundles.json digest does not match the release's bundles entry");
      }
      const index = JSON.parse(indexBytes.toString("utf8"));
      // Bundle URLs are relative to the directory bundles.json itself lives
      // in (see web/src/lib/latex/worker.js), not to the release root.
      const bundleDir = release.bundles.index.slice(0, release.bundles.index.lastIndexOf("/") + 1);
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
          if (bytes.length !== bundle.size || createHash("sha256").update(bytes).digest("hex") !== bundle.sha256) {
            throw new Error(`bundle asset size or digest mismatch: ${bundle.url}`);
          }
        }));
      }
      const fileTargets = new Set(Object.values(index.files || {}));
      for (const bundleName of fileTargets) {
        if (!index.bundles?.[bundleName]) throw new Error(`bundles.json names unknown bundle "${bundleName}" in its files map`);
      }
    }
  }
  console.log(`LaTeX mirror ready: ${base} (${manifest.default_release})`);
} catch (error) {
  console.error(`LaTeX mirror unavailable at ${base}: ${error.message}\n` +
    "Build it with make latex-mirror, then use make deploy LATEX=latex/mirror.\n" +
    "To update the hosted mirror, run make latex-push with its deployment credentials.");
  process.exitCode = 1;
}
