import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdir, mkdtemp, readFile, writeFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { PINNED_FILES } from "./pins-lock.mjs";

const here = fileURLToPath(new URL(".", import.meta.url));
const pins = join(here, "pins.mjs");
const sha = "a".repeat(64);
const rowFor = (file, { repo }) => `${file} ${repo} v0.1.1 ${sha}`;
const lock = `${[...PINNED_FILES].map(([file, entry]) => rowFor(file, entry)).join("\n")}\nlatex wasm-latex v0.1.0 ${sha}\n`;

async function fixture(contents = lock) {
  const root = await mkdtemp(join(tmpdir(), "librepaper-pins-"));
  await writeFile(join(root, "assets.lock"), contents);
  return root;
}

const run = (script, args, cwd = undefined) => execFileSync(process.execPath, [script, ...args], { cwd, encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] });

test("fetch rejects an unknown selector instead of succeeding without fetching", () => {
  let error;
  try {
    run(pins, ["fetch", "not-a-module"]);
  } catch (caught) {
    error = caught;
  }
  assert.match(String(error?.stderr ?? ""), /unknown file not-a-module/);
});

test("fetch parses --root before the optional module selector", async () => {
  const root = await fixture();
  try {
    let error;
    try { run(pins, ["fetch", "--root", root, "not-a-module"]); } catch (caught) { error = caught; }
    assert.match(String(error?.stderr ?? ""), /unknown file not-a-module/);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("pin commands reject a lock with a missing latex row before fetching", async () => {
  const root = await fixture(lock.replace(/^latex .*\n/m, ""));
  try {
    let error;
    try { run(pins, ["fetch", "markdown.wasm", "--root", root]); } catch (caught) { error = caught; }
    assert.match(String(error?.stderr ?? ""), /exactly the required pinned files .* plus one latex row/);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("pin update validates checksums before changing the pin", async () => {
  const root = await fixture();
  const lockPath = join(root, "assets.lock");
  const args = ["release", "--root", root, "--repo", "wasm-markdown", "--tag", "v0.2.0"];
  try {
    const beforeLock = await readFile(lockPath, "utf8");
    const badSums = join(root, "SHA256SUMS");
    await writeFile(badSums, "not-a-checksum markdown.wasm\n");
    assert.throws(() => run(pins, ["update", ...args, "--sums-file", badSums]), /invalid SHA256SUMS/);
    assert.equal(await readFile(lockPath, "utf8"), beforeLock);

    const nextSha = "c".repeat(64);
    await writeFile(badSums, `${nextSha} markdown.wasm\n`);
    run(pins, ["update", ...args, "--sums-file", badSums]);
    assert.match(await readFile(lockPath, "utf8"), new RegExp(`markdown\\.wasm wasm-markdown v0\\.2\\.0 ${nextSha}`));
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("a lock missing a font row is rejected", async () => {
  const root = await fixture(lock.replace(/^fira-code\.woff2 .*\n/m, ""));
  try {
    assert.throws(() => run(pins, ["fetch", "markdown.wasm", "--root", root]), /exactly the required pinned files/);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("release update for editor-fonts rewrites all eleven font rows and no others", async () => {
  const root = await fixture();
  const lockPath = join(root, "assets.lock");
  try {
    const fonts = [...PINNED_FILES].filter(([, { repo }]) => repo === "editor-fonts").map(([file]) => file);
    assert.equal(fonts.length, 11);
    const sums = join(root, "SHA256SUMS");
    await writeFile(sums, fonts.map((file, i) => `${String(i).padStart(2, "0").repeat(32)}  ${file}`).join("\n") + "\n");
    run(pins, ["update", "release", "--root", root, "--repo", "editor-fonts", "--tag", "v1.0.0", "--sums-file", sums]);
    const rows = (await readFile(lockPath, "utf8")).trim().split("\n");
    fonts.forEach((file, i) => assert.ok(rows.includes(`${file} editor-fonts v1.0.0 ${String(i).padStart(2, "0").repeat(32)}`), file));
    assert.ok(rows.includes(`markdown.wasm wasm-markdown v0.1.1 ${sha}`));
    assert.throws(() => run(pins, ["update", "release", "--root", root, "--repo", "wasm-latex", "--tag", "v1.0.0", "--sums-file", sums]), /usage: pins update release/);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("latex pin update rewrites the row with valid release", async () => {
  const { createHash } = await import("node:crypto");
  const tabbedLock = lock.replace(/^latex wasm-latex v0\.1\.0 ([a-f0-9]{64})$/m, "latex\twasm-latex\tv0.1.0\t$1 # keep this comment");
  const root = await fixture(tabbedLock);
  const lockPath = join(root, "assets.lock");
  const latexDir = join(root, "mirror");
  const manifest = JSON.stringify({ version: 1, files: [] });
  const manifestHash = createHash("sha256").update(manifest).digest("hex");
  const releaseDir = join(latexDir, manifestHash);

  try {
    // Create valid release structure.
    await mkdir(releaseDir, { recursive: true });
    await writeFile(join(releaseDir, "MANIFEST.json"), manifest);

    // Create release.json with source URL containing tag.
    const release = JSON.stringify({
      source: { corresponding_source: { url: "https://github.com/LibrePaper/wasm-latex/releases/download/engines-2026.09.15/source.tar.xz" } },
    });
    await writeFile(join(releaseDir, "release.json"), release);

    const beforeLock = await readFile(lockPath, "utf8");
    run(pins, ["update", "latex", "--root", root, latexDir]);

    const newLock = await readFile(lockPath, "utf8");
    assert.notEqual(newLock, beforeLock);
    assert.match(newLock, new RegExp(`latex.*wasm-latex.*engines-2026\\.09\\.15.*${manifestHash}`));
    assert.match(newLock, /# keep this comment/);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("latex pin update fails if manifest hash does not match dir name", async () => {
  const root = await fixture();
  const lockPath = join(root, "assets.lock");
  const latexDir = join(root, "mirror");
  const releaseId = "b".repeat(64);
  const releaseDir = join(latexDir, releaseId);

  try {
    // Create release structure with mismatched manifest hash.
    await mkdir(releaseDir, { recursive: true });
    await writeFile(join(releaseDir, "MANIFEST.json"), JSON.stringify({ bad: true }));

    const beforeLock = await readFile(lockPath, "utf8");
    assert.throws(() => run(pins, ["update", "latex", "--root", root, latexDir]), /does not match directory name/);
    assert.equal(await readFile(lockPath, "utf8"), beforeLock);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});
