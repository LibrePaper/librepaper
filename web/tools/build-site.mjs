#!/usr/bin/env node
// Build the static docs site: every site/**/*.md rendered to HTML by the same
// markdown engine the application embeds, wrapped in one template with the
// sidebar from site/nav.js and a per-page table of contents, mirroring
// web/pages/documentation.html.
//
// This is a Node script rather than a browser page because the site has no
// server of its own to fetch a wasm module from -- it is a directory of
// static files, produced once and handed to a CDN. web/src/lib/renderer-wasm.js's
// `load()` fetches its module, which is exactly the wrong shape for a build
// step reading files off disk, so this instantiates the module itself and
// reuses only `call` and `validateExports`, the parts of that file that do
// not care where the bytes came from.
import { readFile, writeFile, mkdir, readdir, copyFile } from "node:fs/promises";
import { resolve, relative, dirname, join, sep } from "node:path";
import { fileURLToPath } from "node:url";
import { call, validateExports } from "../src/lib/renderer-wasm.js";
import { nav } from "../../site/nav.js";

const here = dirname(fileURLToPath(import.meta.url));
const siteDir = resolve(here, "../../site");
const outDir = resolve(siteDir, ".build");
const wasmPath = resolve(here, "../dist/wasm/markdown.wasm");

/* -------------------------------------------------------------- the engine */

async function loadMarkdownEngine() {
  let bytes;
  try {
    bytes = await readFile(wasmPath);
  } catch (error) {
    if (error.code !== "ENOENT") throw error;
    // Loud and specific: a silent skip here would ship a site with no pages,
    // and a bare ENOENT names a file nobody building the site for the first
    // time would recognise.
    throw new Error(
      `${wasmPath} is missing. Run \`make wasm\` from the repository root to fetch the pinned renderer, then build the site again.`,
    );
  }
  const { exports: wasm } = await WebAssembly.instantiate(await WebAssembly.compile(bytes), {});
  validateExports(wasm, "markdown.wasm");
  return wasm;
}

/* ------------------------------------------------------------ frontmatter */

// Only `title:` is needed, so a hand-rolled parser is enough: everything
// between the first two `---` lines is scanned for a `title:` key, quoted or
// not, and the rest of the document -- frontmatter included -- is passed on
// unparsed. Pulling in a YAML library for one string would be a dependency
// this build does not need.
function splitFrontmatter(source) {
  if (!source.startsWith("---")) return { title: "", body: source };
  const end = source.indexOf("\n---", 3);
  if (end === -1) return { title: "", body: source };
  const frontmatter = source.slice(3, end);
  const body = source.slice(end + 4).replace(/^\r?\n/, "");
  const match = frontmatter.match(/^title:\s*(.*)$/m);
  const title = match ? match[1].trim().replace(/^["']|["']$/g, "") : "";
  return { title, body };
}

/* --------------------------------------------------------------- the ABI */

// `compile` renders a standalone page -- <!doctype>, its own <style>, the
// lot -- because that is also what a reader downloads as a .html export. The
// site wants only what comrak actually produced from the markdown, styled by
// site.css instead, so the wrapper this build put around it is peeled back
// off to the <body> the same document already carries.
function bodyOf(html) {
  const match = html.match(/<body[^>]*>([\s\S]*)<\/body>/);
  return match ? match[1].trim() : html;
}

// The page's own contents list, built from its rendered headings the way
// web/src/entries/documentation.js builds it in the browser -- except this
// build has no browser to run in, so the list is fixed at build time instead
// of kept current by an IntersectionObserver. comrak gives every heading a
// stable id (see wasm-markdown's `header_id_prefix`), which is what these
// links point at.
function tableOfContents(body) {
  const links = [];
  for (const match of body.matchAll(/<h2\s+id="([^"]+)"[^>]*>([\s\S]*?)<\/h2>/g)) {
    const [, id, inner] = match;
    const text = inner.replace(/<[^>]*>/g, "").trim();
    links.push(`<a href="#${id}">${text}</a>`);
  }
  return links.join("\n");
}

/* ------------------------------------------------------------------ pages */

// Flatten site/nav.js into the order pages are walked and linked in. A group
// with a path of its own is a page as well as a heading, and comes before the
// pages under it.
const pages = nav.flatMap((entry) => [
  ...(entry.path ? [{ path: entry.path, label: entry.label }] : []),
  ...(entry.pages ?? []),
]);

async function collectMarkdownFiles(dir) {
  const entries = await readdir(dir, { withFileTypes: true });
  const files = [];
  for (const entry of entries) {
    const full = join(dir, entry.name);
    if (entry.isDirectory()) files.push(...(await collectMarkdownFiles(full)));
    else if (entry.name.endsWith(".md")) files.push(full);
  }
  return files;
}

/* -------------------------------------------------------------- rendering */

// The site-wide nav, shown on every page with the current page's own entry
// marked -- the same aria-current attribute .toc a[aria-current] already
// styles in documentation.css, so a page reading this sidebar looks exactly
// like one reading its own table of contents.
function renderNav(currentPath) {
  const link = (entry, className) => {
    const current = entry.path === currentPath ? ' aria-current="page"' : "";
    const attr = className ? ` class="${className}"` : "";
    return `<a${attr} href="/${entry.path}.html"${current}>${entry.label}</a>`;
  };
  return nav
    .map((entry) => {
      // A group's heading is a link when the group has a page of its own, and
      // plain text when it does not. Either way its children sit in a list
      // under it, indented, so the sidebar shows which pages belong to what
      // rather than one flat run of names.
      const heading = entry.path
        ? link(entry, "sitenav-parent")
        : `<p class="sitenav-parent">${entry.label}</p>`;
      if (!entry.pages) return `<nav class="sitenav-group">${heading}</nav>`;
      const children = entry.pages.map((child) => link(child)).join("\n");
      return `<nav class="sitenav-group">${heading}\n<nav class="sitenav-children">\n${children}\n</nav>\n</nav>`;
    })
    .join("\n");
}

// Where a page's own script tag points, relative to the file being written:
// pages nest to different depths under site/.build (site/.build/agents.html
// beside site/.build/collaborate/edit.html), but the entry they all share
// lives once, at web/src/site/docs.js.
const docsEntry = resolve(here, "../src/site/docs.js");

function template({ title, currentPath, toc, body, scriptSrc }) {
  return `<!doctype html>
<html data-theme="librepaper">
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width,initial-scale=1" />
    <title>${title ? `${title} · ` : ""}LibrePaper docs</title>
    <link rel="icon" type="image/svg+xml" href="/assets/librepaper-icon.svg" />
  </head>
  <body>
    <div id="siteBar"></div>
    <main class="documentation mx-auto w-full max-w-[88rem] px-4 py-8">
      <div class="documentation-layout">
        <aside class="sitenav" aria-label="Site navigation">
          <!-- A details rather than a button and a class: the drawer opens and
               closes, and is reachable from the keyboard, whether or not the
               page's script ever runs. It ships open, so a reader with no
               script keeps the navigation at every width; docs.js closes it
               below the breakpoint, where it becomes the drawer. -->
          <details class="sitenav-drawer" open>
            <summary class="sitenav-toggle">
              <svg viewBox="0 0 24 24" width="18" height="18" aria-hidden="true" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round">
                <path d="M3 6h18M3 12h18M3 18h18" />
              </svg>
              <span>Menu</span>
            </summary>
            <div class="sitenav-panel">
              ${renderNav(currentPath)}
            </div>
          </details>
        </aside>
        <article class="prose"><h1>${title}</h1>${body}</article>
        ${
          toc
            ? `<aside class="pagetoc" aria-label="On this page"><p>On this page</p><nav id="tableOfContents">\n${toc}\n</nav></aside>`
            : ""
        }
      </div>
    </main>
    <dialog id="imageLightbox" class="image-lightbox m-auto max-w-[90vw] bg-transparent p-0" aria-label="Expanded image">
      <button type="button" class="btn-icon preset-filled-surface-200-800 absolute top-2 right-2" aria-label="Close expanded image">×</button>
      <img alt="" />
    </dialog>
    <script type="module" src="${scriptSrc}"></script>
  </body>
</html>
`;
}

async function buildPage(wasm, entry) {
  const source = await readFile(resolve(siteDir, `${entry.path}.md`), "utf8");
  const { title, body: markdown } = splitFrontmatter(source);
  const rendered = call(wasm, "compile", markdown, title || entry.label);
  if (!rendered.ok) {
    throw new Error(`${entry.path}.md failed to render: ${JSON.stringify(rendered.diagnostics)}`);
  }
  const body = bodyOf(rendered.text);
  const outPath = resolve(outDir, `${entry.path}.html`);
  // A path, not a URL: on Windows path.relative comes out with backslashes,
  // which is not a module specifier vite (or a browser) can resolve, so it
  // is normalised the same way whichever platform builds the site.
  const posixRelative = relative(dirname(outPath), docsEntry).split(sep).join("/");
  const html = template({
    title: title || entry.label,
    currentPath: entry.path,
    toc: tableOfContents(body),
    body,
    scriptSrc: posixRelative.startsWith(".") ? posixRelative : `./${posixRelative}`,
  });
  await mkdir(dirname(outPath), { recursive: true });
  await writeFile(outPath, html);
  return outPath;
}

async function main() {
  const wasm = await loadMarkdownEngine();
  const files = await collectMarkdownFiles(siteDir).catch((error) => {
    if (error.code === "ENOENT") return [];
    throw error;
  });
  const known = new Set(files.map((file) => relative(siteDir, file).replace(/\.md$/, "")));
  let built = 0;
  let skipped = 0;
  // Every page in nav.js is a candidate, not a requirement: other agents own
  // the content, some of it does not exist yet, and this build has to keep
  // running -- and keep the pages that do exist linked from a complete
  // sidebar -- while that catches up.
  for (const entry of pages) {
    if (!known.has(entry.path)) {
      skipped++;
      continue;
    }
    await buildPage(wasm, entry);
    built++;
  }
  // The landing page has no markdown to render it from, so it is not among
  // `pages`; but vite.site.config.js's root is this same directory, and an
  // html entry outside a Rollup build's root cannot be given a stable output
  // path. Copying it in, unchanged, is cheaper than teaching that config
  // about a second root.
  await copyFile(resolve(here, "../src/site/index.html"), resolve(outDir, "index.html"));
  console.log(
    `build-site: wrote ${built} page(s) to ${relative(process.cwd(), outDir)}${skipped ? ` (${skipped} page(s) in nav.js have no markdown yet)` : ""}`,
  );
}

main().catch((error) => {
  console.error(`build-site: ${error.message}`);
  process.exit(1);
});
