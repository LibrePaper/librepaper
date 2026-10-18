// The vendored KaTeX, for a test whose in-test Vite build mounts the Symbols
// or Math palette. The build names the path with the define the real build
// uses, and the test's server answers that path from the package.
import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

export const KATEX_DEFINE = { __KATEX__: JSON.stringify("/assets/katex/") };

const dist = fileURLToPath(new URL("../../node_modules/katex/dist/", import.meta.url));
const TYPES = { js: "text/javascript", css: "text/css", woff2: "font/woff2" };

// Answers a request under /assets/katex/ and returns true. Any other request
// returns false and is the caller's to answer.
export function serveKatex(request, response) {
  const pathname = new URL(request.url, "http://localhost").pathname;
  if (!pathname.startsWith("/assets/katex/")) return false;
  const file = join(dist, pathname.slice("/assets/katex/".length));
  const type = TYPES[file.split(".").pop()];
  if (type && existsSync(file)) {
    response.setHeader("Content-Type", type);
    response.end(readFileSync(file));
  } else {
    response.statusCode = 404;
    response.end();
  }
  return true;
}
