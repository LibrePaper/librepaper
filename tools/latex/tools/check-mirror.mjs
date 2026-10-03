// Reject an incomplete or legacy mirror before starting a deployment.
//
// The mirror itself -- engines and TeX Live packages -- is built and pushed
// from the wasm-latex repository (`make mirror`, `make push`; layout in
// wasm-latex/docs/release.md). A release is an immutable directory
// `latex/<id>/` where id is the SHA-256 of `<id>/MANIFEST.json`, described by
// `<id>/release.json` (format 2, every path relative to the release
// directory, bundled releases only, no per-file TeX Live snapshot). This
// check is the consumer side: it verifies that what it is given is a complete
// format-2 release before LibrePaper is pointed at it.
//
//     node check-mirror.mjs <mirror dir>            every <id>/ inside it
//     node check-mirror.mjs <release dir>           one release
//     node check-mirror.mjs <release URL>           <asset-mirror>latex/<id>/
import { readFile, readdir, realpath } from "node:fs/promises";
import { createHash } from "node:crypto";
import { basename, relative, resolve, sep } from "node:path";
import { validateReleaseShape } from "./release-shape.mjs";

const base = process.argv[2];
const sha256 = (bytes) => createHash("sha256").update(bytes).digest("hex");
const ID = /^[a-f0-9]{64}$/;

const safeAssetPath = (url, what) => {
  if (typeof url !== "string" || !url || url.startsWith("/") || /[\\%?#:\0]/.test(url) ||
      url.split("/").some((part) => !part || part === "." || part === "..")) {
    throw new Error(`${what} has an unsafe relative path`);
  }
  return url;
};

/// One release directory on disk: MANIFEST.json hashes to the directory name,
/// release.json is complete, and every engine file and bundle is present with
/// matching size and digest.
async function checkDirectory(directory) {
  const root = await realpath(resolve(directory));
  const inside = async (path, what) => {
    const actual = await realpath(path);
    const rel = relative(root, actual);
    if (!rel || rel === ".." || rel.startsWith(`..${sep}`)) throw new Error(`${what} escapes the release directory`);
    return actual;
  };
  const release = validateReleaseShape(JSON.parse(await readFile(resolve(root, "release.json"), "utf8")));
  if (ID.test(basename(root))) {
    let manifest;
    try {
      manifest = await readFile(resolve(root, "MANIFEST.json"));
    } catch {
      throw new Error("MANIFEST.json is missing");
    }
    if (sha256(manifest) !== basename(root)) throw new Error("release directory is not the SHA-256 of MANIFEST.json");
    if (release.id !== basename(root)) throw new Error(`release.json id ${release.id} does not match its directory`);
  }

  // Every engine file present on disk with matching digest and size.
  for (const [name, spec] of Object.entries(release.engines)) {
    for (const file of spec.files || []) {
      const info = release.files?.[file];
      if (!info) throw new Error(`engine ${name}: file ${file} is absent from release.json`);
      const url = safeAssetPath(info.url, `asset path ${info.url}`);
      const bytes = await readFile(await inside(resolve(root, url), `asset path ${info.url}`));
      if (bytes.length !== info.size || sha256(bytes) !== info.sha256) {
        throw new Error(`engine ${name}: digest or size mismatch: ${info.url}`);
      }
    }
  }

  // SPEC-latex.md "The index": bundles.json's own digest must match what
  // the release pins, and every bundle path it names must actually be on
  // disk -- independent of `release.files`, which only proves the engine
  // files landed intact, not that bundles.json still agrees with the tars
  // beside it.
  const indexPath = safeAssetPath(release.bundles.index, "bundle index");
  const indexBytes = await readFile(await inside(resolve(root, indexPath), "bundle index"));
  if (sha256(indexBytes) !== release.bundles.sha256) {
    throw new Error(`bundle index digest mismatch: ${release.bundles.index}`);
  }
  const index = JSON.parse(indexBytes.toString("utf8"));
  const bundleDir = release.bundles.index.slice(0, release.bundles.index.lastIndexOf("/") + 1);
  const bundleEntries = Object.entries(index.bundles || {});
  if (bundleEntries.length !== release.bundles.count) {
    throw new Error(`release.bundles.count is ${release.bundles.count} but the index names ${bundleEntries.length}`);
  }
  for (let i = 0; i < bundleEntries.length; i += 32) {
    await Promise.all(bundleEntries.slice(i, i + 32).map(async ([name, bundle]) => {
      const path = await inside(resolve(root, safeAssetPath(bundleDir + bundle.url, `bundle path ${bundle.url}`)), `bundle path ${bundle.url}`);
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
  return release;
}

try {
  if (!base) throw new Error("usage: check-mirror.mjs <mirror dir | release dir | release URL>");
  if (/^https?:\/\//.test(base)) {
    const releaseUrl = new URL(`${base.replace(/\/$/, "")}/release.json`);
    const response = await fetch(releaseUrl, { signal: AbortSignal.timeout(30000), redirect: "manual" });
    if (!response.ok) throw new Error(`release.json returned HTTP ${response.status}`);
    const release = validateReleaseShape(await response.json());
    const workerUrl = new URL(safeAssetPath(release.files[release.engines.pdftex.worker].url, "pdfTeX worker"), releaseUrl);
    if (workerUrl.origin !== releaseUrl.origin || !workerUrl.pathname.startsWith(releaseUrl.pathname.slice(0, releaseUrl.pathname.lastIndexOf("/") + 1))) {
      throw new Error("pdfTeX worker URL escapes the release directory");
    }
    const workerResponse = await fetch(workerUrl, { signal: AbortSignal.timeout(30000), redirect: "manual" });
    if (!workerResponse.ok) throw new Error(`pdfTeX worker returned HTTP ${workerResponse.status}`);
    const worker = Buffer.from(await workerResponse.arrayBuffer());
    const workerRecord = release.files[release.engines.pdftex.worker];
    if (worker.length !== workerRecord.size || sha256(worker) !== workerRecord.sha256) {
      throw new Error("pdfTeX worker payload does not match release.json");
    }
    console.log(`LaTeX release ready: ${base} (${release.id}, format ${release.format})`);
  } else {
    const root = resolve(base);
    const single = await readFile(resolve(root, "release.json")).then(() => true, () => false);
    const directories = single
      ? [root]
      : (await readdir(root, { withFileTypes: true })).filter((entry) => entry.isDirectory() && ID.test(entry.name)).map((entry) => resolve(root, entry.name));
    if (!directories.length) throw new Error("no release directories (<sha256>/) found");
    for (const directory of directories) {
      let release;
      try {
        release = await checkDirectory(directory);
      } catch (error) {
        throw new Error(`${basename(directory)}: ${error.message}`);
      }
      console.log(`LaTeX release ready: ${directory} (${release.id}, format ${release.format})`);
    }
  }
} catch (error) {
  console.error(`LaTeX mirror unavailable at ${base}: ${error.message}\n` +
    "The mirror is built and pushed from the wasm-latex repository: `make mirror` there, then `make push`.\n" +
    "Point LibrePaper at it with `make demo ASSET_MIRROR=<asset mirror URL>` or `librepaper admin serve --asset-mirror <url>`.");
  process.exitCode = 1;
}
