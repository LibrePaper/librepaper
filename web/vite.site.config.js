// The static docs site: the landing page and every page web/tools/build-site.mjs
// wrote into site/.build, built with the same Tailwind/Skeleton/Svelte
// toolchain as the reader shell but shipped on its own -- see
// vite.frame.config.js for the precedent of a second, purpose-built config
// beside vite.config.js's.
//
// This never writes into web/dist: that directory is embedded into the Rust
// binary, and the docs site is not -- it is a separate static artifact,
// deployed to its own host. site/_site belongs to this build alone, so
// emptying it first is safe in a way emptying web/dist would not be.
import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import tailwindcss from "@tailwindcss/vite";
import { resolve } from "node:path";
import { cpSync, existsSync, readdirSync } from "node:fs";

const root = resolve(import.meta.dirname, "../site/.build");
const outDir = resolve(import.meta.dirname, "../site/_site");

// The screenshots the docs and the landing page link to. Nothing builds
// them; they are only copied beside whatever this build produced, once, at
// the end, so plugin ordering elsewhere never matters.
// Where the application lives, which is a different host from this static
// site. The published site points at the deployment; a local run points at
// whatever `make deploy` started, so the Sign in button reaches a server that
// is actually listening instead of hanging on one that is not.
//
// Two places need it, so it is applied two ways: `define` for the Svelte bar,
// which is bundled JavaScript, and a transform for the pages, whose links are
// written as literal HTML so a crawler sees them without running anything.
const appOrigin = process.env.LIBREPAPER_APP_ORIGIN || "https://app.librepaper.org";

const pointAtApp = {
  name: "librepaper-site-app-origin",
  transformIndexHtml(html) {
    return html.split("https://app.librepaper.org").join(appOrigin);
  },
};

const copyImages = {
  name: "librepaper-site-copy-images",
  closeBundle() {
    const images = resolve(import.meta.dirname, "../site/images");
    if (existsSync(images)) cpSync(images, resolve(outDir, "images"), { recursive: true });
  },
};

// Every page web/tools/build-site.mjs wrote is its own entry, discovered
// rather than listed by hand: that script decides which pages in site/nav.js
// actually have markdown yet, so this walks its output instead of a list
// that would drift from it.
function htmlEntries(dir, prefix = "") {
  const entries = {};
  if (!existsSync(dir)) return entries;
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const path = resolve(dir, entry.name);
    if (entry.isDirectory()) Object.assign(entries, htmlEntries(path, `${prefix}${entry.name}/`));
    else if (entry.name.endsWith(".html")) entries[`${prefix}${entry.name.replace(/\.html$/, "")}`] = path;
  }
  return entries;
}

export default defineConfig({
  root,
  // The same public directory the reader shell serves its icon and logo
  // from: Logo.svelte points at /assets/librepaper-icon.svg and
  // .../librepaper-logo.svg unconditionally, and those absolute paths only
  // resolve if this build carries the same files at the same place.
  publicDir: resolve(import.meta.dirname, "public"),
  plugins: [pointAtApp, copyImages, tailwindcss(), svelte()],
  define: { __APP_ORIGIN__: JSON.stringify(appOrigin) },
  base: "/",
  build: {
    outDir,
    emptyOutDir: true,
    // web/tools/build-site.mjs discovers these the same way -- by walking
    // site/.build -- and also puts the landing page there (copied in from
    // web/src/site/index.html): an entry outside this build's root cannot be
    // given a stable output filename, so it has to live inside it too.
    rollupOptions: {
      input: htmlEntries(root),
    },
    target: "es2022",
  },
});
