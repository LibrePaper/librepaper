#!/usr/bin/env node
// Inspect bundle catalogs as JSON data; do not execute generated engine code.
import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { HERE, ROOT, digest } from "../corpus.mjs";

const mirror = join(ROOT, "latex/mirror");
const manifestBytes = readFileSync(join(mirror, "manifest.json"));
const manifest = JSON.parse(manifestBytes);
const distribution = manifest.distributions["texlyre-busytex"];
const loaders = [distribution.files["texlive-basic.js"], ...Object.values(distribution.bundles).map((pair) => pair[".js"])];
const paths = new Map();
const bundles = loaders.map((loader) => {
  const script = readFileSync(join(mirror, loader.url), "utf8");
  const tail = script.slice(script.lastIndexOf("loadPackage("));
  const metadata = JSON.parse(tail.slice("loadPackage(".length, tail.indexOf("});") + 1));
  for (const file of metadata.files) paths.set(file.filename, (paths.get(file.filename) || 0) + 1);
  const groups = {};
  const formats = [];
  let logs = 0;
  for (const file of metadata.files) {
    const bytes = file.end - file.start;
    const relative = file.filename.replace(/^\/texlive\/texmf-dist\//, "");
    const group = relative.split("/").slice(0, 2).join("/");
    groups[group] = (groups[group] || 0) + bytes;
    if (file.filename.endsWith(".fmt")) formats.push({ path: file.filename, bytes });
    if (file.filename.endsWith(".log")) logs += bytes;
  }
  return {
    loader: loader.url, files: metadata.files.length,
    payloadBytes: metadata.remote_package_size,
    virtualBytes: metadata.files.reduce((sum, f) => sum + f.end - f.start, 0),
    internalCompression: script.includes("Module['LZ4'].loadPackage") ? "LZ4" : "unidentified",
    largestGroups: Object.entries(groups).sort((a, b) => b[1] - a[1]).slice(0, 12),
    formats, generatedLogBytes: logs,
  };
});
const files = [
  ...Object.values(distribution.files), ...Object.values(distribution.extra),
  ...Object.values(distribution.bundles).flatMap(Object.values),
];
const report = {
  manifestSha256: digest(manifestBytes), release: distribution.release, bundles,
  uniqueCataloguedPaths: paths.size,
  pathsInMultipleBundles: [...paths.values()].filter((n) => n > 1).length,
  assetsOver25MiB: files.filter((f) => f.size > 25 * 1024 * 1024).map(({ url, size }) => ({ url, bytes: size })),
};
writeFileSync(join(HERE, "candidates/PACKAGING.json"), JSON.stringify(report, null, 2) + "\n");
const mb = (bytes) => (bytes / 1e6).toFixed(2);
const lines = [
  "# Existing TeXlyre bundle packaging", "",
  "Catalog inspection only. Payload size is the internally LZ4-compressed .data asset; virtual size is the sum of catalogued file lengths. Neither number measures browser peak memory. No assets were rebuilt or deployed.", "",
  `Mirror manifest: \`${report.manifestSha256}\`. Release: \`${report.release}\`.`, "",
  "| Bundle | Files | Payload MB | Virtual MB | Format MB | Generated log MB |",
  "| --- | ---: | ---: | ---: | ---: | ---: |",
  ...bundles.map((b) => `| ${b.loader.split("/").at(-1)} | ${b.files} | ${mb(b.payloadBytes)} | ${mb(b.virtualBytes)} | ${mb(b.formats.reduce((sum, f) => sum + f.bytes, 0))} | ${mb(b.generatedLogBytes)} |`), "",
  "## Implications", "",
  "The basic bundle includes formats and runtime files for multiple engines. Serving it whole makes a pdfLaTeX document download resources for other engines. The catalog quantifies an opportunity to split/precompute resources; it does not establish how small a working bundle can be. Generated logs are removable build residue, but removing them alone will not solve startup cost.", "",
  `${report.pathsInMultipleBundles} paths occur in multiple bundle catalogs, out of ${report.uniqueCataloguedPaths} unique paths. These are overlapping trees, not three disjoint additions. Identical paths do not prove byte-identical contents; coherent repackaging must resolve those versions rather than blindly deduplicate by name.`, "",
  "These payloads are already internally compressed. Describing the local benchmark as lacking HTTP compression should not be read as saying the TeX files inside its .data assets are uncompressed. Additional transport compression may help, but requires measurement.", "",
  "Komodoc's current deployment uses Workers Static Assets. That service has a 25 MiB per-file limit: the oversized artifacts below need splitting, smaller engine builds, or an object-storage/CDN hosting path. This is a deployment constraint, not a reason to run compilation on a server. [Cloudflare limits](https://developers.cloudflare.com/workers/platform/limits/).", "",
  ...report.assetsOver25MiB.map((f) => `- ${f.url.split("/").at(-1)}: ${mb(f.bytes)} MB.`), "",
  "Reproduce with `node latex/benchmark/candidates/packaging.mjs`. Full catalog summaries are in PACKAGING.json.", "",
];
writeFileSync(join(HERE, "candidates/PACKAGING.md"), lines.join("\n"));
console.log(lines.join("\n"));
