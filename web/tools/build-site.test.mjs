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

function tomlCodeFromHtml(html) {
  const match = html.match(/<pre><code class="language-toml">([\s\S]*?)<\/code><\/pre>/);
  assert.ok(match, "rendered page contains a TOML code block");
  const code = match[1].replace(/&(#x[\da-f]+|#\d+|amp|lt|gt|quot|apos);/gi, (_, name) => {
    if (name.startsWith("#x")) return String.fromCodePoint(Number.parseInt(name.slice(2), 16));
    if (name.startsWith("#")) return String.fromCodePoint(Number.parseInt(name.slice(1), 10));
    return { amp: "&", lt: "<", gt: ">", quot: '"', apos: "'" }[name.toLowerCase()];
  });
  return code.endsWith("\n") ? code.slice(0, -1) : code;
}

test("hosting docs render the production TOML source and fail loudly when it is missing", async () => {
  const root = await mkdtemp(join(tmpdir(), "librepaper-host-config-"));
  try {
    await mkdir(join(root, "docs"), { recursive: true });
    await mkdir(join(root, "web/src/site"), { recursive: true });
    await mkdir(join(root, "tools"), { recursive: true });
    await writeFile(join(root, "package.json"), '{"type":"module"}\n');
    await writeFile(join(root, "docs/nav.js"), 'export const nav = [{ path: "host", label: "Self-hosting" }];\n');
    await writeFile(
      join(root, "docs/host.md"),
      '---\ntitle: "Self-hosting"\n---\n\n<!-- include: tools/deploy-production.toml -->\n',
    );
    await writeFile(join(root, "web/src/site/index.html"), "<!doctype html><title>Home</title>\n");

    for (const [source, destination] of [
      [join(here, "build-site.mjs"), join(root, "web/tools/build-site.mjs")],
      [join(here, "../src/lib/renderer-wasm.js"), join(root, "web/src/lib/renderer-wasm.js")],
      [join(here, "../wasm/markdown.wasm"), join(root, "web/wasm/markdown.wasm")],
    ]) {
      await copyIntoFixture(source, destination);
    }

    const configPath = join(root, "tools/deploy-production.toml");
    const sourceConfig = await readFile(join(here, "../../tools/deploy-production.toml"), "utf8");
    await writeFile(configPath, sourceConfig);
    buildSite(root);
    const pagePath = join(root, "docs/.build/host.html");
    let rendered = await readFile(pagePath, "utf8");
    assert.equal(tomlCodeFromHtml(rendered), sourceConfig.trimEnd());

    const changedConfig = `${sourceConfig.trimEnd()}\n# docs include update marker $& remains literal\n`;
    await writeFile(configPath, changedConfig);
    buildSite(root);
    rendered = await readFile(pagePath, "utf8");
    assert.equal(tomlCodeFromHtml(rendered), changedConfig.trimEnd());

    await rm(configPath);
    assert.throws(() => buildSite(root), (error) => {
      assert.match(error.stderr?.toString() ?? "", /host\.md includes tools\/deploy-production\.toml, but that source file is missing/);
      return true;
    });

    await writeFile(configPath, sourceConfig);
    await writeFile(join(root, "docs/host.md"), '---\ntitle: "Self-hosting"\n---\n\nNo include marker.\n');
    assert.throws(() => buildSite(root), (error) => {
      assert.match(error.stderr?.toString() ?? "", /host\.md must contain exactly one .* marker/);
      return true;
    });
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

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
