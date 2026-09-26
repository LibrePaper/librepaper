// Puts KaTeX where a document can fetch it from its own origin.
//
// Markdown carries its math as TeX, in spans the renderer tags and leaves
// alone (see wasm-markdown). Typesetting is the reader's: the agent inside
// the document frame loads KaTeX and renders every span it finds. That frame
// is on the documents origin, whose CSP allows scripts from itself and nowhere
// in particular, so KaTeX is served from here rather than from a CDN -- the
// same reason the LaTeX distribution is.
//
// The files land under /assets/, named for the version, so they are cached
// for a year like everything else the bundler puts there, and a new KaTeX is
// a new path rather than a cache to invalidate. Only the woff2 faces are
// copied: every browser that runs the agent takes them, and the stylesheet
// lists them first.
//
// Run after the vite builds, which empty dist/ before writing to it.
import { copyFileSync, mkdirSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";

const root = new URL("..", import.meta.url).pathname;
const source = join(root, "node_modules/katex/dist");
const version = JSON.parse(readFileSync(join(root, "node_modules/katex/package.json"), "utf8")).version;
const target = join(root, "dist/assets", `katex-${version}`);

mkdirSync(join(target, "fonts"), { recursive: true });
copyFileSync(join(source, "katex.min.js"), join(target, "katex.min.js"));
// The stylesheet names a woff and a ttf behind every woff2, for browsers that
// no longer exist. Those files are not copied, so the fallbacks go too: what
// the stylesheet names, the build serves, and a test holds it to that.
const css = readFileSync(join(source, "katex.min.css"), "utf8").replace(
  /,url\(fonts\/[^)]+\.(?:woff|ttf)\) format\("(?:woff|truetype)"\)/g,
  "",
);
writeFileSync(join(target, "katex.min.css"), css);
let fonts = 0;
for (const name of readdirSync(join(source, "fonts"))) {
  if (!name.endsWith(".woff2")) continue;
  copyFileSync(join(source, "fonts", name), join(target, "fonts", name));
  fonts++;
}
console.log(`katex ${version}: katex.min.js, katex.min.css and ${fonts} fonts -> dist/assets/katex-${version}/`);
