// What a download is cut from: the live directory, and the file the preview
// is pointed at. Keep this small check close to Reader, because the two trees
// are easy to confuse when changing the preview code.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import vm from "node:vm";
import {
  availableDownloads,
  docxFile,
  entryDownload,
  inlineBlobUrls,
  projectFiles,
  renderingFile,
} from "../../src/lib/reader/downloads.js";

const reader = readFileSync(new URL("../../src/components/Reader.svelte", import.meta.url), "utf8");
const body = (start, end) => {
  const from = reader.indexOf(start);
  assert.notEqual(from, -1, `Reader function not found: ${start}`);
  const to = reader.indexOf(end, from);
  assert.notEqual(to, -1, `Reader function boundary not found: ${end}`);
  return reader.slice(from, to);
};

const liveTreeNow = body("  function liveTreeNow()", "  function treeNow()");
const treeNow = body("  function treeNow()", "  // Painting the preview");

const live = {
  main: "project/current.md",
  texts: { "project/current.md": "live", "project/new.md": "new live file" },
  digests: { "project/fig.png": "asset-sha" },
};
const context = vm.createContext({
  session: { tree: () => live, text: { toString: () => "fallback" } },
  sourceFormat: "markdown",
  editing: false,
  previewMain: "project/new.md",
});

vm.runInContext(`${liveTreeNow}\n${treeNow}`, context);
assert.deepEqual(vm.runInContext("liveTreeNow()", context), live, "the directory is the live one");
// Compared as data: `treeNow` builds its result inside the VM realm, whose
// object prototype is not this one.
assert.deepEqual(
  JSON.parse(JSON.stringify(vm.runInContext("treeNow()", context))),
  { ...live, main: "project/new.md" },
  "the preview follows the explicitly previewed file",
);

// The archive is cut from the tree and directory the caller passed, so a
// folder added while the assets are in flight cannot reach it. That ordering
// used to need a VM harness to observe; it is now a property of the
// signature, and what is left to check is that the assets arrive at all.
const folders = ["project", "project/old-folder"];
const gathering = (assets, missing = []) => async () => ({ held: { assets }, missing });

{
  const files = await projectFiles({
    tree: live,
    folders: [...folders],
    gather: gathering({ "project/fig.png": Uint8Array.of(7) }),
  });
  assert.equal(files["project/current.md"], "live");
  assert.equal(files["project/new.md"], "new live file");
  assert.deepEqual([...files["project/fig.png"]], [7]);
  assert(files["project/"] instanceof Uint8Array);
  assert(files["project/old-folder/"] instanceof Uint8Array);
  assert(!files["project/new-folder/"]);
}

// A project archive tolerates no hole: a figure the server does not have
// fails the download rather than producing a zip found to be incomplete later.
await assert.rejects(
  projectFiles({ tree: live, folders: [...folders], gather: gathering({}, ["project/fig.png"]) }),
  /could not download project\/fig\.png/,
);

// Reader and commenter sessions both set `mayEdit` to false. Even if a stale
// menu or another caller reaches the action directly, it must stop before it
// gathers assets or creates the project archive.
{
  const guard = body("  async function downloadTree()", "  const addDroppedText");
  assert.ok(
    guard.indexOf("Editor access is required to download the project") < guard.indexOf("projectFiles("),
    "downloadTree refuses a non-editor before it gathers anything",
  );
}

// The action is absent as well as guarded. Editors and owners are the two
// roles for which Reader sets `mayEdit`.
assert.match(
  reader,
  /\{#if mayEdit\}(?:(?!\{#if|\{\/if\})[\s\S])*<Menu\.Item value="download" class="menuitem">Download project<\/Menu\.Item>[\s\S]*?\{\/if\}/,
  "the File menu only offers the project archive to editors and owners",
);

{
  const chosen = await entryDownload({
    entry: { kind: "folder", path: "project" },
    tree: live,
    folders: [...folders],
    gather: gathering({ "project/fig.png": Uint8Array.of(7) }),
  });
  assert.equal(chosen.name, "project.zip");
  assert.equal(chosen.files["project/current.md"], "live");
  assert.equal(chosen.files["project/new.md"], "new live file");
  assert.deepEqual([...chosen.files["project/fig.png"]], [7]);
  assert(chosen.files["project/"] instanceof Uint8Array);
  assert(chosen.files["project/old-folder/"] instanceof Uint8Array);
}

// One file downloads as itself, not as an archive of one.
{
  const chosen = await entryDownload({
    entry: { kind: "file", path: "project/current.md" },
    tree: live,
    folders: [...folders],
    gather: gathering({}),
  });
  assert.deepEqual(chosen, { name: "current.md", bytes: "live" });
}

await assert.rejects(
  entryDownload({
    entry: { kind: "file", path: "project/gone.md" },
    tree: live,
    folders: [...folders],
    gather: gathering({}),
  }),
  /This file is no longer available/,
);

// The File menu offers the download for the document's output kind, and only
// once there is something to hand over.
{
  const paged = (extra) => availableDownloads({ outputKind: "pdf", ...extra });
  assert.deepEqual(paged({}), { pdf: false, html: false }, "a paged document with nothing rendered offers nothing");
  assert.deepEqual(paged({ deliveredKind: "pdf" }), { pdf: true, html: false }, "a PDF the frame was handed can be saved");
  assert.deepEqual(paged({ deliveredKind: "" }), { pdf: false, html: false }, "an unrendered document has no PDF fallback");
  assert.deepEqual(paged({ deliveredKind: "html" }), { pdf: false, html: false }, "a paged document never offers HTML");

  const flow = (extra) => availableDownloads({ outputKind: "html", ...extra });
  assert.deepEqual(flow({}), { pdf: false, html: false }, "a page not yet painted offers nothing");
  assert.deepEqual(flow({ deliveredKind: "html" }), { pdf: false, html: true }, "a painted page can be saved");
  assert.deepEqual(flow({ displayedFormat: "html" }), { pdf: false, html: true }, "an authored HTML document is its own rendering");
  assert.deepEqual(flow({ deliveredKind: "" }), { pdf: false, html: false }, "a flow document has no generated-output fallback");
  assert.deepEqual(availableDownloads({ outputKind: "" }), { pdf: false, html: false });
}

// An export is cut from what this tab is holding, and says so when it is
// holding nothing.
{
  const pdf = await renderingFile({ kind: "pdf", slug: "paper", preview: { kind: "pdf", bytes: Uint8Array.of(1) } });
  assert.equal(pdf.name, "paper.pdf");
  assert.equal(pdf.blob.type, "application/pdf");
  await assert.rejects(
    renderingFile({ kind: "pdf", slug: "paper", preview: { kind: "html", html: "<p>x</p>" } }),
    /Render the document before exporting its PDF/,
  );

  const html = await renderingFile({ kind: "html", slug: "paper", preview: { kind: "html", html: "<p>x</p>" } });
  assert.equal(html.name, "paper.html");
  assert.equal(await html.blob.text(), "<p>x</p>");

  // Authored HTML is its own rendering: its source is exported verbatim, and
  // a source that could not be read is an error rather than a quiet fall back
  // to whatever the frame happens to be showing.
  const authored = await renderingFile({
    kind: "html", slug: "paper", authored: true, authoredHtml: "<h1>source</h1>",
    preview: { kind: "html", html: "<p>painted</p>" },
  });
  assert.equal(await authored.blob.text(), "<h1>source</h1>");
  await assert.rejects(
    renderingFile({ kind: "html", slug: "paper", authored: true, authoredHtml: null, preview: { kind: "html", html: "<p>painted</p>" } }),
    /Render the document before exporting its HTML/,
  );

  const docx = docxFile(Uint8Array.of(1, 2), "paper");
  assert.equal(docx.name, "paper.docx");
  assert.match(docx.blob.type, /wordprocessingml/);
}

// A painted page names its figures by object URL; the download carries the
// bytes instead, and leaves alone what it cannot fetch.
{
  const fetched = [];
  const fetcher = async (url) => {
    fetched.push(url);
    if (url.endsWith("gone")) return { ok: false };
    return {
      ok: true,
      headers: { get: () => "image/png" },
      arrayBuffer: async () => Uint8Array.of(1, 2, 3).buffer,
    };
  };
  const page = '<img src="blob:https://x/one#librepaper-asset=a"><img src="blob:https://x/one#librepaper-asset=a"><img src="blob:https://x/gone">';
  const inlined = await inlineBlobUrls(page, fetcher);
  assert.deepEqual(fetched, ["blob:https://x/one", "blob:https://x/gone"], "each object URL is fetched once, without its fragment");
  assert.equal(inlined, '<img src="data:image/png;base64,AQID"><img src="data:image/png;base64,AQID"><img src="blob:https://x/gone">');
  assert.equal(await inlineBlobUrls("<p>no figures</p>", fetcher), "<p>no figures</p>");
}

console.log("downloads: file exports follow transient output handed to the frame");
