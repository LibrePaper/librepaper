import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdtemp, readFile, writeFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

const here = new URL(".", import.meta.url).pathname;
const check = join(here, "check-renderer-pins.mjs");
const update = join(here, "update-module-pin.mjs");
const sha = "a".repeat(64);
const brotliSha = "b".repeat(64);
const lock = `markdown.wasm wasm-markdown v0.1.1 ${sha} ${brotliSha}\nbibliography.wasm wasm-bibliography v0.1.1 ${sha} ${brotliSha}\ncitations.wasm wasm-bibliography v0.1.1 ${sha} ${brotliSha}\ntypst.wasm wasm-typst v0.1.1 ${sha} ${brotliSha}\n`;

async function fixture() {
  const root = await mkdtemp(join(tmpdir(), "librepaper-pins-"));
  await writeFile(join(root, "Cargo.toml"), "[workspace]\nmembers = [\"crates/librepaper\"]\n");
  await writeFile(join(root, "wasm-modules.lock"), lock);
  await writeFile(join(root, "crates-placeholder"), "");
  const crate = join(root, "crates", "librepaper");
  await (await import("node:fs/promises")).mkdir(crate, { recursive: true });
  await writeFile(join(crate, "Cargo.toml"), '[dependencies]\nwasm-markdown = { git = "x", tag = "v0.1.1" }\nwasm-bibliography = { git = "x", tag = "v0.1.1" }\nwasm-typst = { git = "x", tag = "v0.1.1" }\n');
  return root;
}

const run = (script, args, cwd = undefined) => execFileSync(process.execPath, [script, ...args], { cwd, encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] });

test("pin check rejects a native/browser tag mismatch and malformed lock", async () => {
  const root = await fixture();
  try {
    const cargo = join(root, "crates", "librepaper", "Cargo.toml");
    await writeFile(cargo, (await readFile(cargo, "utf8")).replaceAll("v0.1.1", "v0.2.0"));
    assert.throws(() => run(check, ["--root", root]), /does not match browser tag/);
    await writeFile(cargo, (await readFile(cargo, "utf8")).replaceAll("v0.2.0", "v0.1.1"));
    await writeFile(join(root, "wasm-modules.lock"), lock.replace("citations.wasm wasm-bibliography v0.1.1", "citations.wasm wasm-bibliography v0.2.0"));
    assert.throws(() => run(check, ["--root", root], "/tmp"), /conflicting tags/);
    await writeFile(join(root, "wasm-modules.lock"), "broken lock\n");
    assert.throws(() => run(check, ["--root", root]), /expected module repo tag/);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("pin update validates checksums before changing either pin", async () => {
  const root = await fixture();
  const cargo = join(root, "crates", "librepaper", "Cargo.toml");
  const lockPath = join(root, "wasm-modules.lock");
  const args = ["--root", root, "--repo", "wasm-markdown", "--tag", "v0.2.0"];
  try {
    const beforeCargo = await readFile(cargo, "utf8");
    const beforeLock = await readFile(lockPath, "utf8");
    const badSums = join(root, "SHA256SUMS");
    await writeFile(badSums, "not-a-checksum markdown.wasm\n");
    assert.throws(() => run(update, [...args, "--sums-file", badSums]), /invalid SHA256SUMS/);
    assert.equal(await readFile(cargo, "utf8"), beforeCargo);
    assert.equal(await readFile(lockPath, "utf8"), beforeLock);

    const nextSha = "c".repeat(64);
    const nextBrotliSha = "d".repeat(64);
    await writeFile(badSums, `${nextSha} markdown.wasm\n${nextBrotliSha} markdown.wasm.br\n`);
    run(update, [...args, "--sums-file", badSums]);
    assert.match(await readFile(cargo, "utf8"), /wasm-markdown[^\n]*tag = "v0\.2\.0"/);
    assert.match(await readFile(lockPath, "utf8"), new RegExp(`markdown\\.wasm wasm-markdown v0\\.2\\.0 ${nextSha} ${nextBrotliSha}`));
    run(check, ["--root", root]);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});
