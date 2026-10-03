export function validateReleaseShape(release) {
  if (release?.format !== 2) throw new Error(`unsupported release format: ${release?.format}`);
  const pdftex = release.engines?.pdftex;
  if (!pdftex || typeof pdftex.worker !== "string" || !pdftex.worker.trim() ||
      !Array.isArray(pdftex.files) || !pdftex.files.length || !pdftex.files.includes(pdftex.worker)) {
    throw new Error("no complete pdfTeX engine in the release");
  }
  if (!release.files || typeof release.files !== "object" || Array.isArray(release.files)) {
    throw new Error("release has no files map");
  }
  const worker = release.files[pdftex.worker];
  if (!worker || !Number.isSafeInteger(worker.size) || worker.size <= 0 ||
      typeof worker.sha256 !== "string" || !/^[a-f0-9]{64}$/i.test(worker.sha256) ||
      typeof worker.url !== "string" || !worker.url.trim()) {
    throw new Error("pdfTeX worker is missing or has no valid non-empty file record");
  }
  for (const file of pdftex.files) {
    if (typeof file !== "string" || !file.trim() || !release.files[file]) {
      throw new Error(`pdfTeX engine file is absent from release.json: ${file}`);
    }
  }
  if (!release.bundles) throw new Error(`release ${release.id} has no bundles; this mirror ships bundled releases only`);
  return release;
}
