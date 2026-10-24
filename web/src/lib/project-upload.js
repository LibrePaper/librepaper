// ZIP imports use the same path rules and multipart bundle as directories.
// Keep selection separate from extraction, so the rule for which generated or
// local-only inputs to omit can be read apart from reading the ZIP.
import { checkPath, collisionKey, normalisePath } from "./paths.js";
import { openingFile } from "./opening.js";

const extension = (path) => path.slice(path.lastIndexOf(".")).toLowerCase();
const basename = (path) => path.slice(path.lastIndexOf("/") + 1);
const stem = (path) => path.slice(0, path.lastIndexOf("."));
const editorial = new Set([".qmd", ".md", ".bib", ".csl", ".yml", ".yaml", ".css", ".png", ".jpg", ".jpeg", ".gif", ".svg", ".webp", ".pdf", ".r", ".py", ".jl", ".lua"]);
const generatedFolders = new Set(["_freeze", "_site", "_book", "site_libs", "node_modules", "renv", "venv", "env"]);
const hidden = (path) => path.split("/").some((part) => part.startsWith(".") || part === "__MACOSX");
const renderable = (config, path) => (config.extensions || []).some((ext) => path.toLowerCase().endsWith(ext.toLowerCase()));
const renderedOutput = (path, quartoStems) => ([".md", ".html", ".htm", ".ipynb", ".pdf", ".docx", ".tex"].includes(extension(path)) && quartoStems.has(stem(path))) || path.split("/").some((part) => generatedFolders.has(part) || part.endsWith("_files") || part.endsWith("_cache"));

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
  // Only the documents are read to find the file it opens on, not the output beside them.
  const quartoStems = new Set(files.filter((file) => extension(file.path) === ".qmd").map((file) => stem(file.path)));
  const texts = {};
  for (const file of files) {
    if (candidates.includes(file.path) && !renderedOutput(file.path, quartoStems)) texts[file.path] = new TextDecoder().decode(file.bytes);
  }
  const opening = openingFile(texts) ?? "";
  return { files, candidates, opening, skipped, include, root: prefix.replace(/\/$/, "") };
}

export function archiveSelection(project, opening, config) {
  if (!project.candidates.includes(opening)) throw new Error("The archive has no document to open.");
  const quarto = extension(opening) === ".qmd";
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
    const generated = renderedOutput(file.path, quartoStems);
    const excluded = quarto
      ? !project.include.includes(file.path) && (generated || !(editorial.has(ext) || renderable(config, file.path)))
      : file.path === `${stem(opening)}.pdf`;
    const derived = (config.derived_extensions || []).some((suffix) => file.path.toLowerCase().endsWith(suffix));
    if (file.path !== opening && (excluded || derived)) {
      skipped.push(file.path);
      continue;
    }
    const checked = checkPath(config, file.path);
    if (checked.error) throw new Error(checked.error);
    if (checked.kind === "text") {
      try { new TextDecoder("utf-8", { fatal: true }).decode(file.bytes); }
      catch { throw new Error(`${file.path}: document source must be UTF-8 text.`); }
    }
    files.push({ ...file, kind: checked.kind });
  }
  if (files.length > config.max_files) throw new Error(`A document may hold ${config.max_files} files.`);
  return { files, skipped };
}

/// The folders a set of paths implies, parents before children, so a caller
/// that has to make each one before writing into it can walk this in order.
function foldersFor(paths) {
  const seen = new Set();
  for (const path of paths) {
    const parts = path.split("/");
    for (let depth = 1; depth < parts.length; depth++) seen.add(parts.slice(0, depth).join("/"));
  }
  return [...seen].sort((a, b) => a.split("/").length - b.split("/").length || a.localeCompare(b));
}

/// A ZIP among a set of dropped or chosen files becomes the files inside it.
///
/// This is how an existing paper gets into a project: the archive's own rules
/// still apply -- the wrapper folder is stripped, `.librepaper-share.json` is
/// honoured, and Quarto's rendered output is left behind -- and the file the
/// archive opens on is used only to decide what it renders and therefore what
/// not to carry in.
export async function expandArchives(items, rules, unzip) {
  const files = [];
  const folders = [];
  const skipped = [];
  const roots = [];
  for (const item of items) {
    if (!/\.zip$/i.test(item.path)) {
      files.push(item);
      continue;
    }
    // A document's text cannot outgrow its log quota and its figures cannot outgrow what the account may hold.
    const entries = await unzip(item.file, {
      maxBytes: (rules.log_quota_bytes || 0) + (rules.storage?.per_owner || 0),
      maxFiles: (rules.max_files || 200) * 9,
    });
    const project = archiveProject(entries, rules);
    const selected = archiveSelection(project, project.opening, rules);
    skipped.push(...selected.skipped);
    roots.push(project.root);
    // Paths are relative to the archive, so the whole tree lands under
    // whatever folder it was dropped on, and the caller's own collision
    // handling decides what happens where it meets a file already there.
    for (const entry of selected.files) {
      files.push({ file: new File([entry.bytes], basename(entry.path)), path: entry.path });
    }
    folders.push(...foldersFor(selected.files.map((entry) => entry.path)));
  }
  return { files, folders, skipped, roots };
}
