// Puts Mermaid where a document can fetch it from its own origin.
//
// The preview frame draws ```{mermaid} blocks, and Quarto's <pre class="mermaid">
// outputs, with this copy. The frame loads it lazily, only when a document
// has a diagram to draw, from the documents origin, so the application pins
// the diagram renderer with its own release instead of depending on a CDN.
//
// Only the one file is copied: the IIFE build, which sets window.mermaid. The
// ES module builds are for bundlers and are not served.
//
// The file lands under /assets/, named for the version, so it is cached for a
// year like everything else the bundler puts there, and a new Mermaid is a new
// path rather than a cache to invalidate.
//
// Run after the vite builds, which empty dist/ before writing to it.
import { copyFileSync, mkdirSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { MERMAID_VERSION as version } from "./mermaid-path.mjs";

const root = fileURLToPath(new URL("..", import.meta.url));
const source = join(root, "node_modules/mermaid/dist");
const target = join(root, "dist/assets", `mermaid-${version}`);

mkdirSync(target, { recursive: true });
copyFileSync(join(source, "mermaid.min.js"), join(target, "mermaid.min.js"));
console.log(`mermaid ${version}: mermaid.min.js -> dist/assets/mermaid-${version}/`);
