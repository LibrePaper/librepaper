// Instant preview updates against the real LaTeXML engine: a small edit shows
// in the preview frame at once, before the compile, and the compile then
// confirms it without recreating the page's nodes. Setup follows
// latex-html-browser.mjs; skips when no engine build is present.
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

const PATCH_LIMIT = 400; // ms for prose; the first math patch also loads KaTeX
const MATH_LIMIT = 800;
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
  const livePatchKey = `librepaper-build-v3:${JSON.stringify([appBase, "github:browser-test", "live-patch:latex"])}`;
  const setPreference = (on) => tab.evaluate(`localStorage.setItem(${JSON.stringify(livePatchKey)}, ${JSON.stringify(JSON.stringify(on))})`);
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

  // A mark the compile removes: the morph restores a paragraph's id from the
  // new page, so an id that is no longer the mark means a compile landed.
  const probe = () => frame(`(() => { const ps = document.querySelectorAll('p'); const last = ps[ps.length - 1]; window.__lpProbe = last; last.setAttribute('id', 'lp-probe'); return true; })()`);
  const compileLanded = async () => frame(`window.__lpProbe?.getAttribute('id') !== 'lp-probe'`);

  // (a) Watch the frame and tag the nodes that must survive.
  await frame(`(() => {
    window.__lpTagged = [...document.querySelectorAll('p, img, math')];
    window.__lpTagged.forEach((element, index) => { element.__lpTag = index; });
    window.__lpMutations = [];
    new MutationObserver((records) => {
      for (const record of records) {
        const node = record.target.nodeType === 3 ? record.target.parentElement : record.target;
        const paragraphs = [...document.querySelectorAll('p')];
        const owner = paragraphs.findIndex((p) => p === node || p.contains(node));
        window.__lpMutations.push({ type: record.type, owner });
      }
    }).observe(document.body, { subtree: true, childList: true, characterData: true, attributes: true });
    window.__lpShift = 0;
    new PerformanceObserver((list) => {
      for (const entry of list.getEntries()) if (!entry.hadRecentInput) window.__lpShift += entry.value;
    }).observe({ type: 'layout-shift', buffered: true });
    return true;
  })()`);
  await pause(400); // buffered shifts from the page load arrive first
  await probe();
  await frame(`(() => { window.__lpMutations.length = 0; window.__lpShift = 0; return true; })()`);
  const taggedCount = await frame("window.__lpTagged.length");
  assert.ok(taggedCount >= 4, `expected paragraphs and math in the page, found ${taggedCount} tagged nodes`);
  const secondParagraph = await frame(`[...document.querySelectorAll('p')].findIndex((p) => p.textContent.includes('Second paragraph'))`);
  assert.ok(secondParagraph >= 0);

  // (b) A word in the middle of the second paragraph.
  const wordAt = "Second paragraph has several";
  assert.ok(await typeAt(wordAt, wordAt.length, wordAt.length, " banana"), "the word was typed");
  const typedAt = Date.now();
  const toPatch = await elapsedUntil("patched word", async () => /several\s+banana\s+middle/.test(await frameText()), PATCH_LIMIT);
  const mutations = await frame("JSON.stringify(window.__lpMutations)").then(JSON.parse);
  assert.ok(mutations.length > 0, "the patch changed the frame");
  assert.deepEqual(
    mutations.filter((record) => record.type !== "characterData" || record.owner !== secondParagraph),
    [], `before the compile only text in the second paragraph changed: ${JSON.stringify(mutations)}`);
  console.log(`live-preview-browser: word patched in ${Date.now() - typedAt} ms (polled ${toPatch} ms)`);

  // (c) The compile lands and confirms it without recreating anything.
  const toCompile = await elapsedUntil("compile", compileLanded, COMPILE_LIMIT);
  assert.match(await frameText(), /several\s+banana\s+middle/, "the compiled page has the word");
  const intact = JSON.parse(await frame(`JSON.stringify({
    count: document.querySelectorAll('p, img, math').length,
    kept: window.__lpTagged.every((element, index) => element.__lpTag === index && element.isConnected),
    shift: window.__lpShift,
  })`));
  assert.equal(intact.count, taggedCount, "no paragraph, image or equation was added or lost");
  assert.equal(intact.kept, true, "every paragraph, image and equation is still the same element");
  assert.ok(intact.shift < 0.01, `layout shift ${intact.shift}`);
  console.log(`live-preview-browser: compile landed ${Date.now() - typedAt} ms after typing (${toCompile} ms after the patch); layout shift ${intact.shift.toFixed(4)}`);

  // (d) Edit inside $x^2$: a placeholder at once, a compiled <math> later.
  const mathAt = "inline math $x^";
  await probe();
  assert.ok(await typeAt(mathAt, mathAt.length, mathAt.length + 1, "3"), "the exponent was typed");
  const mathTyped = Date.now();
  await elapsedUntil("live math", () => frame(`document.querySelector('.lp-live-math') !== null`), MATH_LIMIT);
  const mathPatchMs = Date.now() - mathTyped;
  await elapsedUntil("compiled math", async () => await compileLanded()
    && await frame(`document.querySelector('.lp-live-math') === null
      && [...document.querySelectorAll('math')].some((math) => (math.getAttribute('alttext') || '').includes('3'))`), COMPILE_LIMIT);
  console.log(`live-preview-browser: math patched in ${mathPatchMs} ms, compiled math ${Date.now() - mathTyped} ms after typing`);

  // (e) With the preference off nothing changes until the compile lands.
  await setPreference(false);
  await probe();
  const offAt = "First paragraph has";
  assert.ok(await typeAt(offAt, offAt.length, offAt.length, " kiwi"), "the word was typed");
  const offTyped = Date.now();
  await pause(1500);
  assert.doesNotMatch(await frameText(), /kiwi/, "preference off: the frame waits for the compile");
  assert.equal(await compileLanded(), false, "the compile cannot have landed yet");
  await elapsedUntil("compile with the preference off", async () => /has\s+kiwi\s+some/.test(await frameText()), COMPILE_LIMIT);
  console.log(`live-preview-browser: preference off, text appeared ${Date.now() - offTyped} ms after typing (compile only)`);
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
