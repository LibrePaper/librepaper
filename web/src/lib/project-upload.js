// ZIP imports use the same path rules and multipart publication as directories.
// Keep selection separate from extraction so choosing another main file can
// recompute which generated or local-only inputs should be omitted.
import { checkPath, collisionKey, normalisePath } from "./paths.js";

const extension = (path) => path.slice(path.lastIndexOf(".")).toLowerCase();
const stem = (path) => path.slice(0, path.lastIndexOf("."));
const editorial = new Set([".qmd", ".md", ".bib", ".csl", ".yml", ".yaml", ".css", ".png", ".jpg", ".jpeg", ".gif", ".svg", ".webp", ".pdf", ".r", ".py", ".jl", ".lua"]);
const generatedFolders = new Set(["_freeze", "_site", "_book", "site_libs", "node_modules", "renv", "venv", "env"]);
const hidden = (path) => path.split("/").some((part) => part.startsWith(".") || part === "__MACOSX");
const renderable = (config, path) => (config.extensions || []).some((ext) => path.toLowerCase().endsWith(ext.toLowerCase()));

export function archiveProject(entries, config) {
  const skipped = entries.filter((entry) => hidden(entry.path)).map((entry) => entry.path);
  // Ignore OS metadata before deciding whether the ZIP has a wrapper folder.
  let files = entries.filter((entry) => !hidden(entry.path));
  const root = files[0]?.path.split("/")[0];
  const prefix = root && files.every((entry) => entry.path.startsWith(`${root}/`)) ? `${root}/` : "";
  files = files.map((entry) => ({ ...entry, path: normalisePath(entry.path.slice(prefix.length)) }));
  const policyPath = `${prefix}.librepaper-share.json`;
  const policy = entries.find((entry) => entry.path === policyPath);
  let include = [];
  if (policy) {
    if (policy.bytes.length > 64 * 1024) throw new Error(".librepaper-share.json exceeds 64 KiB.");
    let value;
    try { value = JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(policy.bytes)); }
    catch { throw new Error("Invalid .librepaper-share.json."); }
    if (!value || typeof value !== "object" || Array.isArray(value) || Object.keys(value).some((key) => key !== "include") || (value.include !== undefined && (!Array.isArray(value.include) || value.include.some((path) => typeof path !== "string")))) {
      throw new Error(".librepaper-share.json must contain an include list of file paths.");
    }
    include = value.include || [];
  }
  const seen = new Set();
  for (const file of files) {
    const key = collisionKey(file.path);
    if (seen.has(key)) throw new Error(`${file.path}: two files cannot share one name.`);
    seen.add(key);
  }
  const candidates = files.filter((entry) => renderable(config, entry.path)).map((entry) => entry.path).sort();
  if (!candidates.length) throw new Error("The archive contains no supported document file.");
  const top = candidates.filter((path) => !path.includes("/") && !([".html", ".htm", ".md", ".markdown"].includes(extension(path)) && candidates.includes(`${stem(path)}.qmd`)));
  const main = top.length === 1 ? top[0] : top.find((path) => stem(path).toLowerCase() === "main") || "";
  return { files, candidates, main, skipped, include };
}

export function archiveSelection(project, main, config) {
  if (!project.candidates.includes(main)) throw new Error("Choose the document file to open from the archive.");
  const quarto = extension(main) === ".qmd";
  const quartoStems = new Set(project.files.filter((file) => extension(file.path) === ".qmd").map((file) => stem(file.path)));
  const skipped = [...project.skipped];
  const files = [];
  if (quarto) {
    for (const path of project.include) {
      const checked = checkPath(config, path);
      if (checked.error) throw new Error(checked.error);
      if (!project.files.some((file) => file.path === path)) throw new Error(`Shared file ${path} is missing or excluded from the archive.`);
    }
  }
  for (const file of project.files) {
    const ext = extension(file.path);
    const generated = ([".md", ".html", ".htm", ".ipynb", ".pdf", ".docx", ".tex"].includes(ext) && quartoStems.has(stem(file.path))) || file.path.split("/").some((part) => generatedFolders.has(part) || part.endsWith("_files") || part.endsWith("_cache"));
    const excluded = quarto
      ? !project.include.includes(file.path) && (generated || !editorial.has(ext))
      : file.path === `${stem(main)}.pdf`;
    const derived = (config.derived_extensions || []).some((suffix) => file.path.toLowerCase().endsWith(suffix));
    if (file.path !== main && (excluded || derived)) {
      skipped.push(file.path);
      continue;
    }
    const checked = checkPath(config, file.path);
    if (checked.error) throw new Error(checked.error);
    if (checked.kind === "text") {
      try { new TextDecoder("utf-8", { fatal: true }).decode(file.bytes); }
      catch { throw new Error(`${file.path}: document source must be UTF-8 text.`); }
    }
    if (checked.kind === "asset" && file.bytes.length > config.max_asset) throw new Error(`${file.path}: this figure exceeds the file size limit.`);
    files.push({ ...file, kind: checked.kind });
  }
  if (files.length > config.max_files) throw new Error(`A document may hold ${config.max_files} files.`);
  const size = (kind) => files.filter((file) => file.kind === kind).reduce((sum, file) => sum + file.bytes.length, 0);
  if (size("text") > config.max_document) throw new Error("The archive's document files exceed the document size limit.");
  if (size("asset") > config.max_assets) throw new Error("The archive's figures exceed the combined asset size limit.");
  return { files, skipped };
}
