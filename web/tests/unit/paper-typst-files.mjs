import assert from "node:assert/strict";
import { existsSync, readFileSync } from "node:fs";
import { createTypstPaperRenderer } from "../../src/lib/paper/typst-renderer.js";

const typstWasm = new URL("../../wasm/typst.wasm", import.meta.url);

if (!existsSync(typstWasm)) {
  console.log("typst.wasm is missing; skipping the Typst paper project tests");
} else {
  const bytes = readFileSync(typstWasm);
  // A 1x1 PNG.
  const png = Uint8Array.from(
    Buffer.from(
      "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==",
      "base64",
    ),
  );

  // A project file is imported, and an image is drawn, by a block compiled in the project.
  {
    const renderer = await createTypstPaperRenderer(bytes, { fetchBytes: async () => png });
    const project = {
      main: "chapters/a.typ",
      texts: { "chapters/util.typ": '#let thing = [Imported marker]' },
      assets: { "chapters/fig.png": "v1" },
      assetUrl: async (path) => `blob:${path}`,
    };
    const preamble = '#import "util.typ": thing\n';
    renderer.setProject(project);
    // Without the image, the block fails and draws nothing.
    assert.equal(renderer.render(preamble + '#image("fig.png")'), "");
    assert.match(renderer.render(preamble + "#thing"), /Imported marker/);
    let updates = 0;
    const changed = await renderer.prepare(
      { ...project, texts: { ...project.texts, "chapters/a.typ": '#image("fig.png")' } },
      preamble,
      () => updates++,
    );
    assert.equal(changed, true);
    assert.equal(updates, 1);
    const html = renderer.render(preamble + '#image("fig.png")');
    assert.match(html, /<img|<svg/);
    // Nothing changed: nothing is asked for again.
    assert.equal(await renderer.prepare(project, preamble), false);
  }

  // A package the preamble imports is fetched once, before the blocks compile.
  {
    const asked = [];
    const fakeFetch = async (needs, fetched) => {
      asked.push(needs.packages.map((pkg) => pkg.dir));
      for (const pkg of needs.packages) {
        fetched.set(`${pkg.dir}/typst.toml`, new TextEncoder().encode('[package]\nname = "fakepkg"\nversion = "0.1.0"\nentrypoint = "lib.typ"\n'));
        fetched.set(`${pkg.dir}/lib.typ`, new TextEncoder().encode("#let packaged = [Package marker]"));
      }
      return true;
    };
    const renderer = await createTypstPaperRenderer(bytes, { fetchNeeds: fakeFetch });
    const project = { main: "main.typ", texts: {}, assets: {} };
    const preamble = '#import "@preview/fakepkg:0.1.0": packaged\n';
    renderer.setProject(project);
    // Before the package arrives the block shows source.
    assert.equal(renderer.render(preamble + "#packaged"), "");
    let updates = 0;
    await renderer.prepare(project, preamble, () => updates++);
    assert.deepEqual(asked.length >= 1, true);
    assert.equal(updates, 1);
    assert.match(renderer.render(preamble + "#packaged"), /Package marker/);
    // The same preamble is not resolved again.
    const before = asked.length;
    await renderer.prepare(project, preamble);
    assert.equal(asked.length, before);
  }
}
