import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { copyFile, mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const here = fileURLToPath(new URL(".", import.meta.url));

async function copyIntoFixture(source, destination) {
  await mkdir(dirname(destination), { recursive: true });
  await copyFile(source, destination);
}

function buildSite(root) {
  execFileSync(process.execPath, [join(root, "web/tools/build-site.mjs")], {
    cwd: root,
    encoding: "utf8",
    stdio: ["ignore", "pipe", "pipe"],
  });
}

test("a second docs build removes pages absent from the current nav and sources", async () => {
  const root = await mkdtemp(join(tmpdir(), "librepaper-site-build-"));
  try {
    await mkdir(join(root, "docs"), { recursive: true });
    await mkdir(join(root, "web/src/site"), { recursive: true });
    await writeFile(join(root, "package.json"), '{"type":"module"}\n');
    await writeFile(
      join(root, "docs/nav.js"),
      'export const nav = [{ path: "kept", label: "Kept" }, { path: "removed", label: "Removed" }];\n',
    );
    await writeFile(join(root, "docs/kept.md"), "# Kept page\n");
    await writeFile(join(root, "docs/removed.md"), "# Removed page\n");
    await writeFile(join(root, "web/src/site/index.html"), "<!doctype html><title>Home</title>\n");

    // `make test` runs `pins` first, so use the same verified renderer artifact
    // while keeping every generated page inside this temporary project.
    for (const [source, destination] of [
      [join(here, "build-site.mjs"), join(root, "web/tools/build-site.mjs")],
      [join(here, "../src/lib/renderer-wasm.js"), join(root, "web/src/lib/renderer-wasm.js")],
      [join(here, "../wasm/markdown.wasm"), join(root, "web/wasm/markdown.wasm")],
    ]) {
      await copyIntoFixture(source, destination);
    }

    buildSite(root);
    const removedPage = join(root, "docs/.build/removed.html");
    assert.match(await readFile(removedPage, "utf8"), /Removed page/);

    await writeFile(join(root, "docs/nav.js"), 'export const nav = [{ path: "kept", label: "Kept" }];\n');
    await rm(join(root, "docs/removed.md"));
    buildSite(root);

    await assert.rejects(readFile(removedPage), { code: "ENOENT" });
    assert.match(await readFile(join(root, "docs/.build/kept.html"), "utf8"), /Kept page/);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});
