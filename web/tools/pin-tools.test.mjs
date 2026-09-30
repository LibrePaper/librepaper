import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdtemp, readFile, writeFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

const here = new URL(".", import.meta.url).pathname;
const update = join(here, "update-module-pin.mjs");
const sha = "a".repeat(64);
const lock = `markdown.wasm wasm-markdown v0.1.1 ${sha}\nbibliography.wasm wasm-bibliography v0.1.1 ${sha}\ncitations.wasm wasm-bibliography v0.1.1 ${sha}\ntypst.wasm wasm-typst v0.1.1 ${sha}\nlatex wasm-latex v0.1.0 ${sha}\n`;

async function fixture() {
  const root = await mkdtemp(join(tmpdir(), "librepaper-pins-"));
  await writeFile(join(root, "assets.lock"), lock);
  return root;
}

const run = (script, args, cwd = undefined) => execFileSync(process.execPath, [script, ...args], { cwd, encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] });

test("pin update validates checksums before changing the pin", async () => {
  const root = await fixture();
  const lockPath = join(root, "assets.lock");
  const args = ["--root", root, "--repo", "wasm-markdown", "--tag", "v0.2.0"];
  try {
    const beforeLock = await readFile(lockPath, "utf8");
    const badSums = join(root, "SHA256SUMS");
    await writeFile(badSums, "not-a-checksum markdown.wasm\n");
    assert.throws(() => run(update, [...args, "--sums-file", badSums]), /invalid SHA256SUMS/);
    assert.equal(await readFile(lockPath, "utf8"), beforeLock);

    const nextSha = "c".repeat(64);
    await writeFile(badSums, `${nextSha} markdown.wasm\n`);
    run(update, [...args, "--sums-file", badSums]);
    assert.match(await readFile(lockPath, "utf8"), new RegExp(`markdown\\.wasm wasm-markdown v0\\.2\\.0 ${nextSha}`));
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});
