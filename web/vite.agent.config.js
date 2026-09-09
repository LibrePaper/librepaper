// The in-frame agent, built on its own.
//
// It runs inside the document, on the documents origin, injected by the server
// as a plain <script src>. That is a different world from the shell: no module
// graph, no imports the browser has to resolve, and a fixed name the server
// can write into every document it serves. So it is one classic script, built
// separately from the pages.
import { defineConfig } from "vite";
import { resolve } from "node:path";

export default defineConfig({
  build: {
    outDir: resolve(import.meta.dirname, "dist"),
    emptyOutDir: false,
    lib: {
      entry: resolve(import.meta.dirname, "src/agent/agent.js"),
      formats: ["iife"],
      name: "librepaperAgent",
      fileName: () => "agent.js",
    },
    target: "es2022",
  },
});
