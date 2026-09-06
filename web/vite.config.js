// The reader shell, built into dist/, which the binary embeds.
//
// Two pages rather than one application with a router: the server already
// routes -- "/" is the landing page and "/docs/<slug>" is the reader -- and a
// document's own address is the thing readers pass around. Nothing is gained
// by taking that over in JavaScript.
import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import tailwindcss from "@tailwindcss/vite";
import { resolve } from "node:path";
import { rmSync } from "node:fs";

// What this build owns in the output directory, and clears before it writes.
// The directory is shared -- the renderers and the README are put there by
// other parts of the build -- so it cannot simply be emptied; but leaving what
// this build wrote last time means a stale bundle is embedded in the binary
// alongside the fresh one, and a rule from a page that no longer exists still
// applies to the pages that do.
const clearOwnOutput = {
  name: "komodoc-clear-own-output",
  buildStart() {
    const out = resolve(import.meta.dirname, "dist");
    rmSync(resolve(out, "assets"), { recursive: true, force: true });
  },
};

export default defineConfig({
  root: resolve(import.meta.dirname, "pages"),
  publicDir: resolve(import.meta.dirname, "public"),
  plugins: [clearOwnOutput, tailwindcss(), svelte()],
  // The pages are served from the site root by the Go-free Rust server, which
  // knows nothing about this build beyond where the files are.
  base: "/",
  build: {
    outDir: resolve(import.meta.dirname, "dist"),
    emptyOutDir: false, // the wasm modules and the README live there too
    // A stale bundle behind a fresh page is the failure this avoids: every
    // asset is named for a digest of its own bytes, so a browser holding the
    // previous build cannot be handed it, and the server can cache them for a
    // year without ever serving one that has moved on.
    assetsDir: "assets",
    rollupOptions: {
      input: {
        index: resolve(import.meta.dirname, "pages/index.html"),
        reader: resolve(import.meta.dirname, "pages/reader.html"),
        documentation: resolve(import.meta.dirname, "pages/documentation.html"),
        notfound: resolve(import.meta.dirname, "pages/404.html"),
        signin: resolve(import.meta.dirname, "pages/signin.html"),
        device: resolve(import.meta.dirname, "pages/device.html"),
        // The PDF frame, served on the documents origin. It is a page of its
        // own rather than a mode of the reader because it is not the reader:
        // it lives with the document, behind the document's CSP, and the
        // pdf.js it pulls in must never end up in the shell's bundle.
        viewer: resolve(import.meta.dirname, "pages/viewer.html"),
      },
    },
    // The typst module is thirty megabytes; nothing here comes close, and a
    // warning about a 600 KB chunk would be noise.
    chunkSizeWarningLimit: 2000,
    target: "es2022",
  },
});
