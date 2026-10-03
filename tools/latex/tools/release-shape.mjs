export function validateReleaseShape(release) {
  if (release?.format !== 2) throw new Error(`unsupported release format: ${release?.format}`);
  if (!release.files || typeof release.files !== "object" || Array.isArray(release.files)) {
    throw new Error("release has no files map");
  }
  if (!release.engines || typeof release.engines !== "object" || Array.isArray(release.engines) || !release.engines.pdftex) {
    throw new Error("no complete pdfTeX engine: release has no pdfTeX specification");
  }

  for (const [name, spec] of Object.entries(release.engines)) {
    const incomplete = () => name === "pdftex"
      ? "no complete pdfTeX engine: invalid worker inventory"
      : `engine ${name} has no valid worker inventory`;
    if (!spec || typeof spec !== "object" || typeof spec.worker !== "string" || !spec.worker.trim() ||
        !Array.isArray(spec.files) || !spec.files.length || !spec.files.includes(spec.worker)) {
      throw new Error(incomplete());
    }
    for (const file of spec.files) {
      if (typeof file !== "string" || !file.trim() || !release.files[file]) {
        throw new Error(`engine ${name} file is absent from release.json: ${file}`);
      }
    }
    const worker = release.files[spec.worker];
    if (!worker || !Number.isSafeInteger(worker.size) || worker.size <= 0 ||
        typeof worker.sha256 !== "string" || !/^[a-f0-9]{64}$/i.test(worker.sha256) ||
        typeof worker.url !== "string" || !worker.url.trim()) {
      throw new Error(`${name} worker is missing or has no valid non-empty file record`);
    }
  }
  if (!release.bundles) throw new Error(`release ${release.id} has no bundles; this mirror ships bundled releases only`);
  return release;
}
