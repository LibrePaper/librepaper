// Move the latex row in assets.lock, pinning a specific release from the mirror.
// Mirror must contain exactly one release directory, named by its manifest sha256.
// Usage: update-latex-pin.mjs [--root DIR] [MIRROR]. --root names the directory
// holding assets.lock (the repository by default; tests point it at a copy).
import { readFile, writeFile, readdir } from "node:fs/promises";
import { createHash } from "node:crypto";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const args = process.argv.slice(2);
const rootAt = args.indexOf("--root");
const rootArg = rootAt >= 0 ? args.splice(rootAt, 2)[1] : undefined;
if (rootAt >= 0 && !rootArg) throw new Error("--root requires a directory");
const mirrorDir = args[0] || "../wasm-latex/mirror";

async function sha256File(path) {
  const data = await readFile(path);
  return createHash("sha256").update(data).digest("hex");
}

async function main() {
  // Validate mirror directory contains exactly one hex-named directory.
  const entries = await readdir(mirrorDir, { withFileTypes: true });
  const hexDirs = entries.filter((e) => e.isDirectory() && /^[a-f0-9]{64}$/.test(e.name));

  if (hexDirs.length !== 1) {
    throw new Error(
      `Mirror must contain exactly one release directory (64 hex chars), found ${hexDirs.length}`
    );
  }

  const releaseId = hexDirs[0].name;
  const releaseDir = join(mirrorDir, releaseId);

  // Validate: manifest.json sha256 must equal directory name.
  const manifestPath = join(releaseDir, "MANIFEST.json");
  const manifestSha = await sha256File(manifestPath);
  if (manifestSha !== releaseId) {
    throw new Error(`MANIFEST.json sha256 (${manifestSha}) does not match directory name (${releaseId})`);
  }

  // Read release.json and extract tag from source URL.
  const releasePath = join(releaseDir, "release.json");
  const releaseJson = JSON.parse(await readFile(releasePath, "utf8"));
  const sourceUrl = releaseJson?.source?.corresponding_source?.url;
  if (!sourceUrl) {
    throw new Error("release.json missing source.corresponding_source.url");
  }

  // Extract tag from URL: ...releases/download/<tag>/...
  const match = sourceUrl.match(/\/releases\/download\/([^/]+)\//);
  if (!match) {
    throw new Error(`Could not extract tag from source URL: ${sourceUrl}`);
  }
  const tag = match[1];

  // Read assets.lock and rewrite latex row.
  const repoRoot = rootArg || fileURLToPath(new URL("../..", import.meta.url));
  const lockPath = join(repoRoot, "assets.lock");
  const lock = await readFile(lockPath, "utf8");

  // Parse lock file, validate structure.
  const lines = lock.split(/\r?\n/);
  const outputLines = [];
  let latexLineIndex = -1;
  let latexLineOriginal = "";

  for (let i = 0; i < lines.length; i++) {
    const raw = lines[i];
    const line = raw.replace(/#.*$/, "").trim();

    if (!line) {
      outputLines.push(raw);
      continue;
    }

    const fields = line.split(/\s+/);
    if (fields.length !== 4 || !/^[a-f0-9]{64}$/.test(fields[3])) {
      throw new Error("assets.lock contains a malformed entry");
    }

    if (fields[0] === "latex") {
      if (latexLineIndex >= 0) {
        throw new Error("assets.lock contains multiple latex rows");
      }
      latexLineIndex = i;
      latexLineOriginal = raw;
    }

    outputLines.push(raw);
  }

  if (latexLineIndex < 0) {
    throw new Error("assets.lock contains no latex row");
  }

  // Parse and update the latex row, preserving whitespace.
  const oldLatexLine = outputLines[latexLineIndex];
  const lineMatch = oldLatexLine.replace(/#.*$/, "").match(/^(\s*)(\S+)(\s+)(\S+)(\s+)(\S+)(\s+)([a-f0-9]{64})(.*)$/);
  if (!lineMatch) {
    throw new Error("latex row in assets.lock is malformed");
  }

  const [, indent, module, middleSpace, repo, repoSpace, oldTag, tagSpace, oldSha, rest] = lineMatch;
  const wasAlreadyPinned = oldTag === tag && oldSha === manifestSha;

  if (wasAlreadyPinned) {
    console.log(`latex: already pinned ${tag} ${manifestSha}`);
    return;
  }

  // Reconstruct the row, preserving spacing structure but adjusting for tag width changes.
  // Keep the spacing structure: indent, module, space, repo, space, tag, space, sha
  const newLatexLine = `${indent}${module}${middleSpace}${repo}${repoSpace}${tag}${tagSpace}${manifestSha}${rest}`;
  outputLines[latexLineIndex] = newLatexLine;
  const newLock = outputLines.join("\n");

  await writeFile(lockPath, newLock);
  console.log(`latex: pinned ${tag} ${manifestSha}`);
}

main().catch((err) => {
  console.error(`Error: ${err.message}`);
  process.exit(1);
});
