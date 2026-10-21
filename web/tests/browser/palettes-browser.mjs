// A contrast check for the colour palettes, measured on the real styles.
//
// Each palette and mode is applied to the page, and every colour pair the
// palette contract names is resolved to the colour the browser paints. The
// page does the resolving, because var() and color-mix() only settle there.
// With LP_PALETTE_SHOTS set to a directory, the editor is also captured for
// each palette and mode.

import assert from "node:assert/strict";
import { build } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import tailwindcss from "@tailwindcss/vite";
import { createServer } from "node:http";
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";
import { dirname, join, resolve } from "node:path";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { contentType, loroAlias } from "../helpers/loro.mjs";
import { requireChromiumExecutable } from "../helpers/browser-executable.mjs";
import { stopBrowserProcess, removeTemporary } from "../helpers/browser-driver.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const root = dirname(dirname(dirname(here)));
const temporary = mkdtempSync(join(tmpdir(), "librepaper-palettes-check-"));
const output = join(temporary, "build");
const profile = join(temporary, "chrome");
const entry = join(temporary, "entry.js");
const port = 19000 + Math.floor(Math.random() * 1000);
const shots = process.env.LP_PALETTE_SHOTS ? resolve(process.env.LP_PALETTE_SHOTS) : null;

const palettes = ["librepaper", "flexoki", "solarized", "gruvbox", "catppuccin", "nord"];
const modes = ["light", "dark"];

const sample = [
  "\\documentclass{article}",
  "\\usepackage{amsmath}",
  "",
  "\\section{Introduction}",
  "Prose with \\emph{emphasis} and inline math $x^2 + 1$. \\label{sec:intro}",
  "% A comment about the draft, which should read as quiet text.",
  "See Section~\\ref{sec:intro} and the source at \\url{https://example.org/paper}.",
  "",
  "\\begin{equation}",
  "  e^{i\\pi} + 1 = 0",
  "\\end{equation}",
].join("\n");

// Runs in the page, so it is self-contained: its source is copied into the
// generated entry, and nothing it uses may live out here in Node.
function contrastCheck(palette, mode) {
  const html = document.documentElement;
  html.dataset.theme = "librepaper";
  html.dataset.palette = palette;
  html.dataset.mode = mode;

  const probe = document.createElement("span");
  document.body.append(probe);
  const canvas = document.createElement("canvas");
  canvas.width = 1;
  canvas.height = 1;
  const context = canvas.getContext("2d", { willReadFrequently: true });

  // Returns the sRGB channels of a token, or null when the palette does not set it.
  const toRgb = (token) => {
    if (!getComputedStyle(html).getPropertyValue(token).trim()) return null;
    probe.style.color = "var(" + token + ")";
    const computed = getComputedStyle(probe).color;
    const match = computed.match(/^rgba?\((.+)\)$/);
    if (match) return match[1].split(/[\s,\/]+/).filter(Boolean).slice(0, 3).map(Number);
    // Other colour spaces come back as written, so the canvas converts them.
    context.clearRect(0, 0, 1, 1);
    context.fillStyle = "#000000";
    context.fillStyle = computed;
    context.fillRect(0, 0, 1, 1);
    const [r, g, b] = context.getImageData(0, 0, 1, 1).data;
    return [r, g, b];
  };

  const luminance = ([r, g, b]) => {
    const [R, G, B] = [r, g, b].map((channel) => {
      const c = channel / 255;
      return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
    });
    return 0.2126 * R + 0.7152 * G + 0.0722 * B;
  };
  const ratio = (a, b) => {
    const [lighter, darker] = [luminance(a), luminance(b)].sort((x, y) => y - x);
    return (lighter + 0.05) / (darker + 0.05);
  };

  const surfaces = ["--color-shell", "--color-sidebar", "--color-raised"];
  const text = ["--color-text", "--color-text-secondary", "--color-text-muted", "--color-line-number", "--color-link"];
  const brands = ["--color-brand", "--color-brand-hover", "--color-brand-pressed"];
  const highlights = [
    "keyword", "string", "constant", "comment", "tag", "property", "function",
    "variable", "heading", "link", "monospace", "punctuation", "meta", "invalid",
  ];

  // Each pair is [foreground, background, floor].
  const pairs = [];
  for (const foreground of text) for (const background of surfaces) pairs.push([foreground, background, 4.5]);
  for (const background of brands) pairs.push(["--color-on-brand", background, 4.5]);
  pairs.push(["--color-success-text", "--color-success-bg", 4.5]);
  pairs.push(["--color-warning-text", "--color-warning-bg", 4.5]);
  pairs.push(["--color-error-text", "--color-error-bg", 4.5]);
  pairs.push(["--color-error-text", "--color-shell", 4.5]);
  for (const name of highlights) pairs.push(["--source-highlight-" + name, "--color-shell", 3]);
  pairs.push(["--color-focus", "--color-shell", 3]);
  // The secondary button: its label at rest, hovered and pressed.
  for (const background of ["--color-row-selected", "--color-tonal-hover", "--color-tonal-pressed"]) pairs.push(["--color-text", background, 4.5]);
  // Avatars carry white initials at normal text size.
  for (let index = 1; index <= 6; index += 1) pairs.push(["--color-text-inverse", "--color-avatar-" + index, 4.5]);

  const failures = [];
  for (const [foreground, background, floor] of pairs) {
    const label = palette + " " + mode + ": " + foreground + " on " + background;
    const a = toRgb(foreground);
    const b = toRgb(background);
    if (!a || !b) {
      failures.push(label + " is not set");
      continue;
    }
    const value = ratio(a, b);
    if (value < floor) failures.push(label + " " + (Math.round(value * 10) / 10).toFixed(1) + " < " + floor.toFixed(1));
  }
  probe.remove();
  return failures;
}

const source = `
import ${JSON.stringify(join(root, "web/src/styles/app.css"))};
import { join as joinSession } from ${JSON.stringify(join(root, "web/src/lib/collab.js"))};
import Editor from ${JSON.stringify(join(root, "web/src/components/Editor.svelte"))};
import { createClassComponent } from ${JSON.stringify(join(root, "web/node_modules/svelte/src/legacy/legacy-client.js"))};

window.contrastCheck = ${contrastCheck.toString()};

const html = document.documentElement;
html.dataset.theme = "librepaper";
html.dataset.palette = "librepaper";
html.dataset.mode = "light";
document.body.style.margin = "0";

// A full-window host, so the editor has room to be seen in a screenshot.
const host = document.createElement("main");
host.style.cssText = "height: 100vh; width: 100vw; display: flex; flex-direction: column; overflow: hidden;";
document.body.append(host);

const session = joinSession({ send: () => {}, mayEdit: true });
const file = session.addText("paper.tex", ${JSON.stringify(sample)});
session.setMain(file);
createClassComponent({
  component: Editor,
  target: host,
  props: { session, format: "latex", file },
});

window.paletteCheckReady = true;
`;

writeFileSync(entry, source);
let server;
let browser;
let socket;
try {
  await build({
    configFile: false,
    root: join(root, "web"),
    resolve: { alias: loroAlias },
    plugins: [tailwindcss(), svelte()],
    logLevel: "error",
    build: {
      outDir: output,
      emptyOutDir: true,
      lib: { entry, formats: ["es"], fileName: () => "palette-check.js", cssFileName: "palette-check" },
    },
  });

  server = createServer((request, response) => {
    const file = join(output, request.url.slice(1));
    if (request.url !== "/" && existsSync(file)) {
      response.setHeader("Content-Type", contentType(file));
      response.end(readFileSync(file));
      return;
    }
    response.setHeader("Content-Type", "text/html");
    response.end('<body><link rel="stylesheet" href="/palette-check.css"><script type="module" src="/palette-check.js"></script></body>');
  });
  await new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", resolve);
  });
  const address = server.address();
  const httpPort = typeof address === "object" && address ? address.port : 0;
  if (!httpPort) throw new Error("local palette server did not start");

  browser = spawn(requireChromiumExecutable(), [
    "--headless=new",
    "--no-sandbox",
    "--disable-gpu",
    `--user-data-dir=${profile}`,
    `--remote-debugging-port=${port}`,
    "about:blank",
  ], { stdio: "ignore", detached: true });
  const wait = (milliseconds) => new Promise((resolve) => setTimeout(resolve, milliseconds));
  let browserInfo;
  for (let attempt = 0; attempt < 100; attempt++) {
    try {
      browserInfo = await fetch(`http://127.0.0.1:${port}/json`).then((answer) => answer.json());
      break;
    } catch {
      await wait(100);
    }
  }
  const page = browserInfo?.find((target) => target.type === "page");
  if (!page) throw new Error("Chromium did not expose a page target");
  socket = new WebSocket(page.webSocketDebuggerUrl);
  await new Promise((resolve, reject) => {
    socket.addEventListener("open", resolve, { once: true });
    socket.addEventListener("error", reject, { once: true });
  });

  let sequence = 0;
  const pending = new Map();
  socket.addEventListener("message", (event) => {
    const message = JSON.parse(event.data);
    const resolve = pending.get(message.id);
    if (resolve) {
      pending.delete(message.id);
      resolve(message);
    }
  });
  const send = (method, params = {}) => new Promise((resolve) => {
    const id = ++sequence;
    pending.set(id, resolve);
    socket.send(JSON.stringify({ id, method, params }));
  });
  const evaluate = async (expression) => {
    const message = await send("Runtime.evaluate", {
      expression,
      awaitPromise: true,
      returnByValue: true,
    });
    if (message.result?.exceptionDetails) throw new Error(JSON.stringify(message.result.exceptionDetails));
    return message.result.result.value;
  };

  await send("Page.navigate", { url: `http://127.0.0.1:${httpPort}/` });
  let ready = false;
  for (let attempt = 0; attempt < 100; attempt++) {
    ready = await evaluate("Boolean(window.paletteCheckReady && document.querySelector('.cm-editor'))");
    if (ready) break;
    await wait(100);
  }
  assert.equal(ready, true, "Editor did not mount in Chromium");
  await send("Emulation.setDeviceMetricsOverride", { width: 1000, height: 700, deviceScaleFactor: 1, mobile: false });
  if (shots) mkdirSync(shots, { recursive: true });

  const failures = [];
  for (const palette of palettes) {
    for (const mode of modes) {
      failures.push(...(await evaluate(`contrastCheck(${JSON.stringify(palette)}, ${JSON.stringify(mode)})`)));
      if (!shots) continue;
      // Setting the attributes here, then waiting two frames, lets the new
      // palette paint before the capture.
      await evaluate(`(() => {
        document.documentElement.dataset.palette = ${JSON.stringify(palette)};
        document.documentElement.dataset.mode = ${JSON.stringify(mode)};
        return true;
      })()`);
      await evaluate("new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(() => done(true))))");
      const shot = await send("Page.captureScreenshot", { format: "png" });
      assert.ok(shot.result?.data, `no screenshot for ${palette} ${mode}`);
      writeFileSync(join(shots, `${palette}-${mode}.png`), Buffer.from(shot.result.data, "base64"));
    }
  }
  assert.equal(failures.length, 0, ["Palette contrast floors missed:", ...failures].join("\n"));
  console.log("palettes-browser: every palette reaches its contrast floor in light and dark");
} finally {
  socket?.close();
  await stopBrowserProcess(browser);
  if (server?.listening) await new Promise((resolve) => server.close(resolve));
  removeTemporary(temporary);
}
