#!/usr/bin/env node
// Saves the system clipboard's HTML and plain text as a paste fixture.
//
// Usage:
//   node web/tools/paste-capture.mjs <name> [--app "Google Docs"] [--browser "Chrome 142"] [--note "..."] [--force]
//
// Writes web/tests/fixtures/paste/captured/<name>.{html,txt,json}. The captures
// are real clipboards and are never edited by hand. See
// web/tests/fixtures/paste/README.md for the procedure.

import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
export const capturedDir = join(here, "..", "tests", "fixtures", "paste", "captured");

const NAME_PATTERN = /^[a-z0-9]+(-[a-z0-9]+)*$/;
const VALUE_OPTIONS = new Set(["--app", "--browser", "--note"]);
const MAC_HTML_TYPE = "«class HTML»";

export function isValidName(name) {
  return typeof name === "string" && NAME_PATTERN.test(name);
}

export function parseArgs(argv) {
  const options = { name: undefined, app: null, browser: null, note: null, force: false };
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    if (arg === "--force") {
      options.force = true;
    } else if (VALUE_OPTIONS.has(arg)) {
      const value = argv[i + 1];
      if (value === undefined || value.startsWith("--")) {
        throw new Error(`${arg} needs a value`);
      }
      options[arg.slice(2)] = value;
      i++;
    } else if (arg.startsWith("--")) {
      throw new Error(`unknown option ${arg}`);
    } else if (options.name === undefined) {
      options.name = arg;
    } else {
      throw new Error(`unexpected argument ${arg}`);
    }
  }
  if (options.name === undefined) {
    throw new Error("missing <name>, e.g. google-docs-chrome");
  }
  return options;
}

// osascript prints the HTML clipboard as «data HTML<hex>».
export function decodeClipboardHex(text) {
  const match = /«data HTML([0-9A-Fa-f]*)»/.exec(text);
  if (!match || match[1].length % 2 !== 0) {
    throw new Error(`could not decode the macOS HTML clipboard: ${text.slice(0, 80)}`);
  }
  return Buffer.from(match[1], "hex");
}

export function decodeUtf8(buffer) {
  const hasBom = buffer.length >= 3 && buffer[0] === 0xef && buffer[1] === 0xbb && buffer[2] === 0xbf;
  return new TextDecoder("utf-8").decode(hasBom ? buffer.subarray(3) : buffer);
}

export function buildMetadata(info, now = new Date()) {
  return {
    name: info.name,
    app: info.app ?? null,
    browser: info.browser ?? null,
    note: info.note ?? null,
    platform: info.platform,
    session: info.session,
    types: [...info.types],
    capturedAt: now.toISOString().slice(0, 10),
    htmlBytes: info.htmlBytes,
    textBytes: info.textBytes,
  };
}

function run(command, args) {
  return execFileSync(command, args, { encoding: "buffer", stdio: ["ignore", "pipe", "pipe"] });
}

// Output of the command, or null when it fails (for example, a MIME type the
// clipboard does not offer).
function tryRun(command, args) {
  try {
    return run(command, args);
  } catch {
    return null;
  }
}

function splitLines(buffer) {
  return decodeUtf8(buffer)
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter(Boolean);
}

// Each reader reports the clipboard's types and offers read functions. The
// read functions return a Buffer, or null when that format is unavailable.
function waylandReader() {
  const types = splitLines(run("wl-paste", ["--list-types"]));
  return {
    session: "wayland",
    types,
    hasHtml: types.includes("text/html"),
    readHtml: () => run("wl-paste", ["--no-newline", "-t", "text/html"]),
    textTypes: ["text/plain;charset=utf-8", "text/plain", "UTF8_STRING"],
    readText: (type) => tryRun("wl-paste", ["--no-newline", "-t", type]),
  };
}

function x11Reader() {
  const types = splitLines(run("xclip", ["-selection", "clipboard", "-t", "TARGETS", "-o"]));
  return {
    session: "x11",
    types,
    hasHtml: types.includes("text/html"),
    readHtml: () => run("xclip", ["-selection", "clipboard", "-t", "text/html", "-o"]),
    textTypes: ["UTF8_STRING", "text/plain;charset=utf-8", "text/plain"],
    readText: (type) => tryRun("xclip", ["-selection", "clipboard", "-t", type, "-o"]),
  };
}

function darwinReader() {
  const info = decodeUtf8(run("osascript", ["-e", "clipboard info"]));
  const types = [...info.matchAll(/«[^»]+»/g)].map((match) => match[0]);
  return {
    session: "darwin",
    types,
    hasHtml: types.includes(MAC_HTML_TYPE),
    readHtml: () => decodeClipboardHex(decodeUtf8(run("osascript", ["-e", "the clipboard as «class HTML»"]))),
    textTypes: ["plain"],
    readText: () => run("pbpaste", []),
  };
}

function openClipboard() {
  if (process.platform === "darwin") return darwinReader();
  if (process.env.WAYLAND_DISPLAY) return waylandReader();
  return x11Reader();
}

function readPlainText(clipboard) {
  for (const type of clipboard.textTypes) {
    const buffer = clipboard.readText(type);
    if (buffer !== null) return decodeUtf8(buffer);
  }
  return "";
}

export function captureFiles(name, dir = capturedDir) {
  return {
    html: join(dir, `${name}.html`),
    txt: join(dir, `${name}.txt`),
    json: join(dir, `${name}.json`),
  };
}

function main(argv) {
  const options = parseArgs(argv);
  if (!isValidName(options.name)) {
    throw new Error(
      `invalid name "${options.name}": use lowercase letters and digits joined by single hyphens, e.g. google-docs-chrome`,
    );
  }
  const files = captureFiles(options.name);
  if (!options.force) {
    const existing = Object.values(files).filter((file) => existsSync(file));
    if (existing.length > 0) {
      throw new Error(`refusing to overwrite ${existing.join(", ")} (pass --force to replace)`);
    }
  }

  const clipboard = openClipboard();
  if (!clipboard.hasHtml) {
    throw new Error(
      `the clipboard has no text/html (types found: ${clipboard.types.join(", ") || "none"}). ` +
        "Select all in the source app, copy, and run again.",
    );
  }
  const html = clipboard.readHtml();

  const text = readPlainText(clipboard);
  if (text === "") {
    console.warn("warning: no plain text on the clipboard; the .txt file is empty");
  }

  const metadata = buildMetadata({
    name: options.name,
    app: options.app,
    browser: options.browser,
    note: options.note,
    platform: process.platform,
    session: clipboard.session,
    types: clipboard.types,
    htmlBytes: html.length,
    textBytes: Buffer.byteLength(text, "utf8"),
  });

  mkdirSync(capturedDir, { recursive: true });
  writeFileSync(files.html, html);
  writeFileSync(files.txt, text, "utf8");
  writeFileSync(files.json, `${JSON.stringify(metadata, null, 2)}\n`, "utf8");

  console.log(`wrote ${files.html}`);
  console.log(`wrote ${files.txt}`);
  console.log(`wrote ${files.json}`);
  console.log("");
  console.log("Next, write the expected outputs and review them:");
  console.log("  cd web && UPDATE_PASTE_GOLDENS=1 node --test --experimental-transform-types tests/unit/paste-captured.mjs");
}

if (import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    main(process.argv.slice(2));
  } catch (error) {
    console.error(`paste-capture: ${error.message}`);
    process.exit(1);
  }
}
