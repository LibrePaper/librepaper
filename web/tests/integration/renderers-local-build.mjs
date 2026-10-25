// Renders set to this computer reach the desktop app as one protocol 3 job
// per format and output, and what comes back is shaped like a browser render:
// the page, the SyncTeX bytes, the log, and diagnostics read out of the log.
import assert from "node:assert/strict";
import * as client from "../../src/lib/desktop/client.js";
import * as renderers from "../../src/lib/renderers.js";
import { formOf } from "../helpers/form-body.mjs";

globalThis.location = { origin: "https://app.test" };

const values = new Map();
const storage = {
  getItem: (key) => values.get(key) || null,
  setItem: (key, value) => values.set(key, String(value)),
  removeItem: (key) => values.delete(key),
};

function response(status, body, bytes = null) {
  return {
    ok: status >= 200 && status < 300,
    status,
    json: async () => body,
    text: async () => new TextDecoder().decode(bytes || new Uint8Array()),
    arrayBuffer: async () => (bytes || new Uint8Array()).buffer,
    clone() { return response(status, body, bytes); },
  };
}
async function descriptor(bytes) {
  const hash = await crypto.subtle.digest("SHA-256", bytes);
  return { size: bytes.byteLength, sha256: [...new Uint8Array(hash)].map((byte) => byte.toString(16).padStart(2, "0")).join("") };
}
const text = (value) => new TextEncoder().encode(value);

globalThis.window = { __LIBREPAPER_DESKTOP__: { base: "lp-local://localhost/", version: "test" } };
client._testing.reset();
client._testing.inject({ storage, wait: () => Promise.resolve() });
client.configure({ project: "paper", origin: "https://app.test" });

// A fake desktop app that answers one render with the given files.
let submitted = null;
function answer({ status = "done", exit = 0, error, files = {} }) {
  client._testing.inject({ fetch: async (url, init = {}) => {
    if (url.endsWith("/jobs") && init.method === "POST") {
      submitted = JSON.parse(await (await formOf(init)).get("job").text());
      return response(202, { id: "render-1" });
    }
    if (url.endsWith("/jobs/render-1")) {
      const outputs = {};
      for (const [name, bytes] of Object.entries(files)) outputs[name] = await descriptor(bytes);
      return response(200, { status, exit, ...(error ? { error } : {}), outputs });
    }
    const name = decodeURIComponent(url.split("/files/")[1] || "");
    if (files[name]) return response(200, {}, files[name]);
    throw new Error(`unexpected request ${url}`);
  } });
}

const pdf = text("%PDF-local");
const typst = { main: "main.typ", texts: { "main.typ": "= Test" }, assets: {} };

answer({ files: { artifact: pdf, log: text("compiled") } });
const compiled = await renderers.render(typst, "Test", { project: "paper", buildPreferences: { backend: "local", output: "pdf" } });
assert.equal(compiled.ok, true);
assert.equal(submitted.protocol, 3);
assert.equal(submitted.format, "typst");
assert.equal(submitted.output, "pdf");
assert.equal(submitted.entrypoint, "main.typ");
assert.equal(submitted.binding_id, "hosted");
assert.deepEqual([...compiled.pdf], [...pdf]);
assert.equal(compiled.html, null);
assert.equal(compiled.log, "compiled");
assert.deepEqual(compiled.diagnostics, []);

// A failed Typst run is read into one diagnostic. A location counts only when
// it names a file of the project.
answer({ status: "failed", exit: 1, error: "typst failed", files: { log: text("error: unknown function\n  \u250c\u2500 main.typ:4:7\n") } });
const failed = await renderers.render(typst, "Broken", { project: "paper", buildPreferences: { backend: "local", output: "pdf" } });
assert.equal(failed.ok, false);
assert.equal(failed.failure.message, "typst failed");
assert.equal(failed.pdf, null);
assert.equal(failed.diagnostics.length, 1);
assert.equal(failed.diagnostics[0].message, "unknown function");
assert.equal(failed.diagnostics[0].file, "main.typ");
assert.equal(failed.diagnostics[0].line, 4);
assert.equal(failed.diagnostics[0].column, 7);

answer({ status: "failed", exit: 1, error: "typst failed", files: { log: text("error: failed\n  \u250c\u2500 /home/me/private.typ:1:1\n") } });
const outside = await renderers.render(typst, "Broken", { project: "paper", buildPreferences: { backend: "local", output: "pdf" } });
assert.equal(outside.diagnostics[0].file, "");
assert.equal(outside.diagnostics[0].line, 0);

// LaTeX is no longer browser only. Its PDF, SyncTeX bytes and diagnostics all
// come back, the diagnostics from the TeX log parser.
const synctex = new Uint8Array([0x1f, 0x8b, 0x08]);
answer({ files: { artifact: pdf, synctex, log: text("! Undefined control sequence.\nl.3 \\foo\n") } });
const latex = await renderers.render({ main: "chapters/main.tex", texts: { "chapters/main.tex": "\\documentclass{article}" }, assets: {} }, "Paper", { project: "paper", buildPreferences: { backend: "local", output: "html" } });
assert.equal(submitted.format, "latex");
assert.equal(submitted.output, "pdf");
assert.equal(submitted.entrypoint, "chapters/main.tex");
assert.deepEqual([...latex.pdf], [...pdf]);
assert.deepEqual([...latex.synctex], [...synctex]);
assert.equal(latex.diagnostics[0].severity, "error");
assert.equal(latex.diagnostics[0].line, 3);

// Quarto and Markdown take the output the preview asks for.
const page = text("<h1>ok</h1>");
answer({ files: { artifact: page } });
const quarto = await renderers.render({ main: "paper.qmd", texts: { "paper.qmd": "# Test" }, assets: {} }, "Test", { project: "paper", buildPreferences: { backend: "local", output: "html" } });
assert.equal(submitted.format, "quarto");
assert.equal(submitted.output, "html");
assert.equal(quarto.html, "<h1>ok</h1>");
assert.equal(quarto.pdf, null);

answer({ files: { artifact: pdf } });
const markdown = await renderers.render({ main: "notes.md", texts: { "notes.md": "# Test" }, assets: {} }, "Test", { project: "paper", buildPreferences: { backend: "local", output: "pdf" } });
assert.equal(submitted.format, "markdown");
assert.equal(submitted.output, "pdf");
assert.deepEqual([...markdown.pdf], [...pdf]);

answer({ files: { artifact: text("docx") } });
const word = await renderers.render({ main: "notes.md", texts: { "notes.md": "# Test" }, assets: {} }, "Test", { format: "docx", project: "paper", buildPreferences: { backend: "local", output: "docx" } });
assert.equal(submitted.output, "docx");
assert.equal(word.artifactKind, "docx");
assert.equal(new TextDecoder().decode(word.artifact), "docx");

// A failed Quarto run keeps its log and has no diagnostics of its own.
answer({ status: "failed", exit: 1, error: "quarto failed", files: { log: text("ERROR: no R") } });
const quartoFailed = await renderers.render({ main: "paper.qmd", texts: { "paper.qmd": "# Test" }, assets: {} }, "Test", { project: "paper", buildPreferences: { backend: "local", output: "html" } });
assert.equal(quartoFailed.ok, false);
assert.equal(quartoFailed.log, "ERROR: no R");
assert.deepEqual(quartoFailed.diagnostics, []);

client._testing.reset();
console.log("renderers-local-build: local renders send protocol 3 jobs and read back pages, SyncTeX and logs");
