// Continuous compile against the real LaTeXML engine: with the preference on
// the HTML preview updates while typing, without layout shift; with it off the
// preview waits for a pause. Setup follows latex-html-browser.mjs; skips when
// no engine build is present.
import assert from "node:assert/strict";
import { createHash, createHmac } from "node:crypto";
import { spawn, spawnSync } from "node:child_process";
import { existsSync, readFileSync, writeFileSync, readdirSync, symlinkSync, mkdirSync, mkdtempSync, rmSync } from "node:fs";
import { resolve, join } from "node:path";
import { tmpdir } from "node:os";
import { browser, until, removeTemporary } from "../helpers/browser-driver.mjs";
import { ephemeralMirror } from "../helpers/ephemeral-mirror.mjs";
import { postgresTestDatabase } from "../helpers/postgres-test.mjs";

const root = resolve(import.meta.dirname, "../../..");
const engineRoot = resolve(process.env.LATEXML_DIST || join(root, "../wasm-latex/wasm-build/dist"));
const mirror = resolve(process.env.MIRROR || join(root, "../wasm-latex/mirror"));
const hostedMirrorUrl = process.env.LATEXML_MIRROR_URL?.replace(/\/?$/, "/");
if (hostedMirrorUrl && !/^https:\/\//i.test(hostedMirrorUrl)) throw new Error("LATEXML_MIRROR_URL must use HTTPS");
if (!existsSync(engineRoot) || (!hostedMirrorUrl && !existsSync(mirror))) {
  console.log("live-preview-browser: no wasm-latex engine build or mirror next to this checkout; skipping (build wasm-latex, or set LATEXML_DIST and LATEXML_MIRROR_URL)");
  process.exit(0);
}
// The local mirror is served over HTTPS from a throwaway self-signed cert,
// which openssl mints; a hosted mirror needs none.
if (!hostedMirrorUrl && spawnSync("openssl", ["version"], { stdio: "ignore" }).error) {
  console.log("live-preview-browser: no openssl for the HTTPS mirror; skipping (install openssl, or set LATEXML_MIRROR_URL)");
  process.exit(0);
}
if (!process.env.LIBREPAPER_BIN) {
  console.log("live-preview-browser: LIBREPAPER_BIN is not set; skipping (set it to the librepaper binary)");
  process.exit(0);
}
// A hosted mirror is a release URL (`<asset-mirror>latex/<id>/`); a local one is
// a directory holding exactly one `<sha256>` release directory.
const localIds = hostedMirrorUrl ? [] : readdirSync(mirror, { withFileTypes: true })
  .filter((entry) => entry.isDirectory() && /^[a-f0-9]{64}$/.test(entry.name)).map((entry) => entry.name);
if (!hostedMirrorUrl && localIds.length !== 1) throw new Error(`expected exactly one <sha256> release directory in ${mirror}, found ${localIds.length}`);
const release = structuredClone(hostedMirrorUrl
  ? await (await fetch(new URL("release.json", hostedMirrorUrl))).json()
  : JSON.parse(readFileSync(join(mirror, localIds[0], "release.json"), "utf8")));
// Exercise the actual deployment release when checking an assembled mirror.
// Otherwise overlay freshly built artifacts for engine development.
const legacyRelease = "legacy-release";
const useMirror = Boolean(hostedMirrorUrl) || process.env.LATEXML_USE_MIRROR === "1";
const engineFiles = [
  "latexml.worker.js", "latexml.js", "latexml.wasm", "kpse-resolve.js", "bundle-mode.js", "latexml.css",
  "LaTeXML.css", "LaTeXML-blue.css", "LaTeXML-marginpar.css", "LaTeXML-navbar-left.css", "LaTeXML-navbar-right.css",
  "ltx-amsart.css", "ltx-apj.css", "ltx-article.css", "ltx-book.css", "ltx-listings.css",
  "ltx-report.css", "ltx-svjour.css", "ltx-ulem.css", "latexml.build.json",
];
if (useMirror) {
  assert.ok(release.engines.latexml, "the deployment mirror must advertise LaTeXML");
} else {
  // The synthetic release lists the freshly built engine files by their bare
  // names, as a published release does: `createHtmlCompiler` requires the worker
  // to be one of `spec.files`, and every path resolves against the release
  // directory. The overlay below symlinks the engine files into that directory,
  // so this is the same layout as a published release without copying its
  // large WASM files.
  release.engines.latexml = { worker: "latexml.worker.js", files: engineFiles };
  release.files = { ...release.files, ...Object.fromEntries(engineFiles.map((name) => {
    const bytes = readFileSync(join(engineRoot, name));
    return [name, { url: name, size: bytes.length, sha256: createHash("sha256").update(bytes).digest("hex") }];
  })) };
}
let mirrorRoot = mirror;
let mirrorServer = null;
let mirrorBase = hostedMirrorUrl;
let overlay = null;
if (!hostedMirrorUrl) {
  // Keep development overlays isolated from the assembled mirror. Symlinks
  // avoid copying several gigabytes while the HTTPS server still exposes a
  // normal static mirror layout to a cross-origin browser.
  if (!useMirror) {
    overlay = mkdtempSync(join(tmpdir(), "librepaper-latexml-mirror-"));
    const overlayRelease = join(overlay, localIds[0]);
    mkdirSync(overlayRelease);
    for (const name of readdirSync(join(mirror, localIds[0]))) {
      if (name !== "release.json" && !engineFiles.includes(name)) symlinkSync(join(mirror, localIds[0], name), join(overlayRelease, name));
    }
    for (const name of engineFiles) symlinkSync(join(engineRoot, name), join(overlayRelease, name));
    writeFileSync(join(overlayRelease, "release.json"), JSON.stringify(release));
    mirrorRoot = overlay;
  }
  mirrorServer = await ephemeralMirror(mirrorRoot);
  mirrorBase = mirrorServer.latexUrl;
}const scratch = mkdtempSync(join(tmpdir(), "librepaper-live-preview-"));
const appData = join(scratch, "app-data");
mkdirSync(appData, { recursive: true });
let tab;
let app;
let postgres;
let appLog = "";

function appSession(accountId) {
  const key = Buffer.from(readFileSync(join(appData, "secrets", "session.key"), "utf8").trim(), "hex");
  const generation = "1";
  const expires = Math.floor(Date.now() / 1000) + 3600;
  const payload = Buffer.from(`github|browser-test|${accountId}|${generation}||Browser Test|${expires}`).toString("base64url");
  const signature = createHmac("sha256", key).update(`session-v2\0${payload}`).digest("base64url");
  return { value: `v2.${payload}.${signature}`, generation };
}

const COMPILE_LIMIT = 245000;
const pause = (ms) => new Promise((done) => setTimeout(done, ms));

try {
  process.env.LIBREPAPER_BROWSER_IGNORE_CERT_ERRORS = "1";
  tab = await browser(process.env.BROWSER || "chromium", join(scratch, "profile"), 19000 + Math.floor(Math.random() * 1000));
  postgres = postgresTestDatabase("live_preview_browser");
  const port = 22000 + Math.floor(Math.random() * 1000);
  const appBase = `http://localhost:${port}`;
  const configPath = join(appData, "config.toml");
  writeFileSync(configPath, [
    "[server]", `address = "127.0.0.1:${port}"`, "",
    "[storage]", `directory = ${JSON.stringify(appData)}`, 'database_url = { env = "LIBREPAPER_DATABASE_URL" }', "fsync = false", 'object_store = "filesystem"', "",
    "[auth.github]", 'client_id = { env = "DEV_GITHUB_CLIENT_ID" }', 'client_secret = { env = "DEV_GITHUB_CLIENT_SECRET" }', 'publishers = ["*"]', "",
    "[access]", "anonymous_comments = true", "",
    "[assets]", `mirror = ${JSON.stringify(new URL("../../", mirrorBase).href)}`, "",
  ].join("\n"));
  const env = { ...process.env, LIBREPAPER_DATABASE_URL: postgres.url, DEV_GITHUB_CLIENT_ID: "test-client", DEV_GITHUB_CLIENT_SECRET: "test-secret" };
  app = spawn(resolve(process.env.LIBREPAPER_BIN), ["admin", "serve", "--config", configPath], { env, stdio: ["ignore", "ignore", "pipe"] });
  app.stderr.on("data", (bytes) => { appLog += bytes; });
  await until("app startup", async () => {
    if (app.exitCode !== null) throw new Error(appLog);
    return (await fetch(`${appBase}/api/config`)).ok;
  });
  const accountId = postgres.seedRegisteredAccount({ provider: "github", subject: "github:browser-test", handle: "browser-test", displayName: "Browser Test" });
  const session = appSession(accountId);
  await tab.setCookie("librepaper_session", session.value, appBase);
  await tab.navigate(appBase);
  await until("app page", () => tab.evaluate(`location.origin === ${JSON.stringify(appBase)} && document.readyState === 'complete'`));

  // The document has no image: assets live in the collaborative document, and
  // the project-creation endpoint takes only the main source.
  const appSource = String.raw`\documentclass{article}
\usepackage{amsmath}
\begin{document}

First paragraph has some plain words in it.

Second paragraph has several middle words to type in.

Third paragraph mentions inline math $x^2$ here.

\begin{equation} y = x^2 \end{equation}

Last paragraph closes the document.
\end{document}
`;
  const body = JSON.stringify({ title: "Live preview", source_format: "latex", source: appSource });
  const created = JSON.parse(await tab.evaluate(`(async () => {
    const response = await fetch('/api/documents', {method:'POST',headers:{'content-type':'application/json','x-librepaper-client':'1'},body:${JSON.stringify(body)}});
    return JSON.stringify({status:response.status,body:await response.json()});
  })()`));
  assert.equal(created.status, 201, JSON.stringify(created.body));
  const { slug } = created.body;
  const liveCompileKey = `librepaper-build-v3:${JSON.stringify([appBase, "github:browser-test", "live-compile:latex"])}`;
  const setPreference = (on) => tab.evaluate(`localStorage.setItem(${JSON.stringify(liveCompileKey)}, ${JSON.stringify(JSON.stringify(on))})`);
  await setPreference(true);
  await tab.resize(1300, 900);
  await tab.navigate(`${appBase}/docs/${slug}`);
  await until("HTML frame", () => tab.evaluate(`document.querySelector('iframe')?.src.includes('/raw/')`));
  await until("first compile", async () => (await tab.text()).includes("Second paragraph"), COMPILE_LIMIT);
  await until("editor", () => tab.evaluate(`Boolean(document.querySelector('.cm-content'))`));

  const frame = (expression) => tab.frameEvaluate(expression);
  const frameText = async () => (await frame("document.body.innerText")) || "";
  const elapsedUntil = async (label, test, limit) => {
    const started = Date.now();
    for (;;) {
      if (await test()) return Date.now() - started;
      if (Date.now() - started > limit) throw new Error(`${label}: not within ${limit} ms`);
      await pause(15);
    }
  };

  // Put the editor's selection over [from, to) of the line holding `marker`,
  // then replace it with `text`. Returns after the insertion was made.
  const placeSelection = (marker, from, to) => tab.evaluate(`(() => {
    const line = [...document.querySelectorAll('.cm-line')].find((l) => l.textContent.includes(${JSON.stringify(marker)}));
    if (!line) throw new Error('no editor line with ' + ${JSON.stringify(marker)});
    const base = line.textContent.indexOf(${JSON.stringify(marker)});
    const walker = document.createTreeWalker(line, NodeFilter.SHOW_TEXT);
    const locate = (offset) => {
      let seen = 0;
      for (let node = walker.nextNode(); node; node = walker.nextNode()) {
        if (offset <= seen + node.data.length) return [node, offset - seen];
        seen += node.data.length;
      }
      throw new Error('offset past the line');
    };
    document.querySelector('.cm-content').focus();
    walker.currentNode = line;
    const [startNode, startOffset] = locate(base + ${from});
    walker.currentNode = line;
    const [endNode, endOffset] = locate(base + ${to});
    const range = document.createRange();
    range.setStart(startNode, startOffset);
    range.setEnd(endNode, endOffset);
    const selection = getSelection();
    selection.removeAllRanges();
    selection.addRange(range);
    return true;
  })()`);
  const typeAt = async (marker, from, to, text) => {
    await placeSelection(marker, from, to);
    await pause(150); // CodeMirror adopts the DOM selection on selectionchange
    return tab.evaluate(`document.execCommand('insertText', false, ${JSON.stringify(text)})`);
  };

  // Layout shift is summed over the frame's life; typing must not add to it.
  await frame(`(() => {
    window.__lpShift = 0;
    new PerformanceObserver((list) => {
      for (const entry of list.getEntries()) if (!entry.hadRecentInput) window.__lpShift += entry.value;
    }).observe({ type: 'layout-shift', buffered: true });
    return true;
  })()`);
  await pause(400); // buffered shifts from the page load arrive first
  await frame(`(() => { window.__lpShift = 0; return true; })()`);

  // One character every 150 ms for 8 s into the middle of a paragraph, noting
  // each distinct preview text seen while typing.
  const typeFor = async (marker, letters) => {
    const at = marker.length;
    assert.ok(await typeAt(marker, at, at, letters[0]), "the first character was typed");
    const started = Date.now();
    const seen = [await frameText()];
    const times = [];
    let index = 1;
    let next = started + 150;
    while (Date.now() - started < 8000) {
      if (Date.now() >= next) {
        await tab.evaluate(`document.execCommand('insertText', false, ${JSON.stringify(letters[index % letters.length])})`);
        index += 1;
        next += 150;
      }
      const text = await frameText();
      if (text !== seen.at(-1)) { seen.push(text); times.push(Date.now() - started); }
      await pause(20);
    }
    return { seen, times, typed: index, started };
  };
  const marker = "Second paragraph has several";
  const alphabet = "abcdefghijklmnopqrstuvwxyz";

  // (a) Preference on: the preview changes while typing.
  const on = await typeFor(marker, alphabet);
  const updates = on.seen.length - 1;
  console.log(`live-preview-browser: preference on, ${updates} preview updates during ${on.typed} keystrokes, at ${JSON.stringify(on.times)} ms`);
  assert.ok(updates >= 2, `expected at least 2 preview updates while typing, saw ${updates}`);
  const shift = Number(await frame("window.__lpShift"));
  console.log(`live-preview-browser: layout shift while typing ${shift.toFixed(4)}`);
  assert.ok(shift < 0.01, `layout shift ${shift}`);
  // Let the last compile settle before the second half.
  await elapsedUntil("settled preview", async () => (await frameText()).includes(alphabet.slice(0, 3)), COMPILE_LIMIT);

  // (b) Preference off: nothing changes while typing, the text arrives after.
  await setPreference(false);
  await tab.navigate(`${appBase}/docs/${slug}`);
  await until("HTML frame again", () => tab.evaluate(`document.querySelector('iframe')?.src.includes('/raw/')`));
  await until("compile again", async () => (await tab.text()).includes("Second paragraph"), COMPILE_LIMIT);
  await until("editor again", () => tab.evaluate(`Boolean(document.querySelector('.cm-content'))`));
  await pause(500);
  const off = await typeFor("First paragraph has", "QWERTYUIOP");
  console.log(`live-preview-browser: preference off, ${off.seen.length - 1} preview updates during ${off.typed} keystrokes`);
  assert.equal(off.seen.length, 1, `preference off: the preview must not change while typing, saw ${off.seen.length - 1} changes`);
  const stopped = Date.now();
  await elapsedUntil("update after typing stops", async () => (await frameText()) !== off.seen[0], 10000);
  console.log(`live-preview-browser: preference off, preview updated ${Date.now() - stopped} ms after typing stopped`);
} catch (error) {
  if (appLog) console.error("Application log:\n", appLog);
  if (tab) {
    console.error("Page at failure:", await tab.evaluate("JSON.stringify({text:document.body.innerText,frames:Array.from(document.querySelectorAll('iframe')).map(f=>f.src),storage:{...localStorage}})").catch(() => "unavailable"));
  }
  throw error;
} finally {
  await tab?.close();
  app?.kill();
  postgres?.drop();
  if (mirrorServer) {
    mirrorServer.server.closeAllConnections();
    await new Promise((done) => mirrorServer.server.close(done));
    rmSync(mirrorServer.tls, { recursive: true, force: true });
  }
  if (overlay) rmSync(overlay, { recursive: true, force: true });
  removeTemporary(scratch);
}
