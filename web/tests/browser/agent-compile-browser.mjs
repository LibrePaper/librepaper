// document_compile end to end: an agent token asks the owner's open editor tab
// to compile a file, and the tab answers through the real LaTeX engines. Setup
// follows live-preview-browser.mjs; skips when no engine build is present.
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
  console.log("agent-compile-browser: no wasm-latex engine build or mirror next to this checkout; skipping (build wasm-latex, or set LATEXML_DIST and LATEXML_MIRROR_URL)");
  process.exit(0);
}
// The local mirror is served over HTTPS from a throwaway self-signed cert,
// which openssl mints; a hosted mirror needs none.
if (!hostedMirrorUrl && spawnSync("openssl", ["version"], { stdio: "ignore" }).error) {
  console.log("agent-compile-browser: no openssl for the HTTPS mirror; skipping (install openssl, or set LATEXML_MIRROR_URL)");
  process.exit(0);
}
if (!process.env.LIBREPAPER_BIN) {
  console.log("agent-compile-browser: LIBREPAPER_BIN is not set; skipping (set it to the librepaper binary)");
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
}
const scratch = mkdtempSync(join(tmpdir(), "librepaper-agent-compile-"));
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
const GOOD = String.raw`\documentclass{article}
\begin{document}
Hello.
\end{document}
`;
const BAD = String.raw`\documentclass{article}
\begin{document}
\nosuchmacro
Hello.
\end{document}
`;

try {
  process.env.LIBREPAPER_BROWSER_IGNORE_CERT_ERRORS = "1";
  tab = await browser(process.env.BROWSER || "chromium", join(scratch, "profile"), 19000 + Math.floor(Math.random() * 1000));
  postgres = postgresTestDatabase("agent_compile_browser");
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

  // A two-file project, uploaded as the owner (the command line's multipart form).
  const ownerHeaders = { "x-librepaper-client": "1", cookie: `librepaper_session=${session.value}` };
  const form = new FormData();
  form.append("title", "Agent compile");
  form.append("file", new Blob([GOOD]), "good.tex");
  form.append("file", new Blob([BAD]), "bad.tex");
  const created = await fetch(`${appBase}/api/documents`, { method: "POST", headers: ownerHeaders, body: form });
  const createdBody = await created.json();
  assert.equal(created.status, 201, JSON.stringify(createdBody));
  const { slug } = createdBody;

  const mint = async (role, name) => {
    const response = await fetch(`${appBase}/api/documents/${slug}/share`, {
      method: "POST", headers: { ...ownerHeaders, "content-type": "application/json" },
      body: JSON.stringify({ create: { kind: "token", role, name, until: "1d", budget: null } }),
    });
    const payload = await response.json();
    assert.equal(response.status, 200, `share refused: ${JSON.stringify(payload)}`);
    assert.match(payload.created?.token || "", /^lpt_/, JSON.stringify(payload));
    return payload.created.token;
  };
  const readToken = await mint("read", "Compile reader");

  // A tool call as an agent makes it: bearer token, arguments as the body.
  // Success is {result}, failure is {error: {code, message}}.
  const callTool = async (tool, args) => {
    const response = await fetch(`${appBase}/api/documents/${slug}/tools/${tool}`, {
      method: "POST",
      headers: { "content-type": "application/json", "x-librepaper-client": "1", authorization: `Bearer ${readToken}` },
      body: JSON.stringify(args),
    });
    const body = await response.json().catch(() => null);
    return { http: response.status, result: body?.result, error: body?.error, body };
  };
  const show = (label, call) => {
    const r = call.result;
    console.log(r
      ? `agent-compile-browser: ${label}: status=${r.status} output=${r.output} current=${r.current} diagnostics=${(r.diagnostics || []).length} error=${r.error ?? "-"}`
      : `agent-compile-browser: ${label}: failure ${call.error?.code} (HTTP ${call.http})`);
  };

  // (2) No tab is open yet.
  const early = await callTool("document_compile", { path: "good.tex", output: "html", wait_ms: 1000 });
  show("no tab", early);
  assert.equal(early.error?.code, "renderer_unavailable", JSON.stringify(early.body));

  // (3) Open the project as the owner and wait for the editor.
  await tab.resize(1300, 900);
  await tab.navigate(`${appBase}/docs/${slug}`);
  await until("editor", () => tab.evaluate(`Boolean(document.querySelector('.cm-content'))`), COMPILE_LIMIT);

  // A compile, polled to its end. The tab's room socket may join a moment
  // after the editor shows, so renderer_unavailable is retried briefly.
  const compile = async (label, args) => {
    let call;
    const started = Date.now();
    for (;;) {
      call = await callTool("document_compile", { wait_ms: 25000, ...args });
      if (call.error?.code !== "renderer_unavailable" || Date.now() - started > 30000) break;
      await pause(1000);
    }
    show(label, call);
    assert.ok(call.result, `${label}: expected a result, got ${JSON.stringify(call.body)}`);
    const id = call.result.compile_id;
    assert.match(id || "", /^compile_[0-9a-f]{32}$/);
    const polled = Date.now();
    while (call.result.status === "pending" && Date.now() - polled < 60000) {
      call = await callTool("document_result", { kind: "compile", id, wait_ms: 10000 });
      show(`${label} (polled)`, call);
      assert.ok(call.result, `${label}: poll failed: ${JSON.stringify(call.body)}`);
    }
    return call.result;
  };

  // PDF only when the mirror ships the PDF engine.
  const outputs = release.engines.pdftex ? ["html", "pdf"] : ["html"];
  if (outputs.length === 1) console.log("agent-compile-browser: the mirror has no pdftex engine; covering LaTeXML HTML only");
  for (const output of outputs) {
    const good = await compile(`good.tex ${output}`, { path: "good.tex", output });
    const diagnosticsOf = JSON.stringify(good.diagnostics);
    assert.equal(good.status, "succeeded", `good.tex ${output}: ${JSON.stringify(good).slice(0, 2000)}`);
    assert.equal(good.output, output);
    assert.ok(!(good.diagnostics || []).some((d) => d.severity === "error"), `good.tex ${output} diagnostics: ${diagnosticsOf}`);
    assert.ok(good.revision, "a revision is reported");
    if (good.current !== true) console.log(`agent-compile-browser: current is ${good.current}, revision ${good.revision}`);
    assert.equal(good.current, true, `good.tex ${output}: current must be true; revision ${good.revision}`);

    const bad = await compile(`bad.tex ${output}`, { path: "bad.tex", output });
    assert.equal(bad.status, "failed", `bad.tex ${output}: ${JSON.stringify(bad).slice(0, 2000)}`);
    const errors = (bad.diagnostics || []).filter((d) => d.severity === "error");
    assert.ok(errors.length >= 1, `bad.tex ${output}: expected an error diagnostic: ${JSON.stringify(bad.diagnostics)}`);
    assert.ok(
      errors.some((d) => d.file === "bad.tex" && d.line === 3),
      `bad.tex ${output}: expected an error at bad.tex line 3: ${JSON.stringify(bad.diagnostics)}`,
    );
  }

  // (6) A path that is not in the project.
  const missing = await compile("missing.tex", { path: "missing.tex" });
  assert.equal(missing.status, "unavailable", JSON.stringify(missing));
  assert.equal(missing.error, "failed", JSON.stringify(missing));

  // (7) Running code needs an Editor token.
  const forbidden = await callTool("document_compile", { path: "good.tex", run: true, wait_ms: 1000 });
  show("run with a read token", forbidden);
  assert.equal(forbidden.error?.code, "forbidden", JSON.stringify(forbidden.body));

  // (8) An agent token cannot answer a request.
  const forged = await fetch(`${appBase}/api/documents/${slug}/compile/compile_${"0".repeat(32)}`, {
    method: "POST",
    headers: { "content-type": "application/json", "x-librepaper-client": "1", authorization: `Bearer ${readToken}` },
    body: JSON.stringify({ revision: "x", ok: true, output: "html", diagnostics: [], log: "", provenance: {} }),
  });
  console.log(`agent-compile-browser: forged reply with the agent token: HTTP ${forged.status}`);
  // Refused either before the route (401: agent tokens are not accepted here) or by it (404).
  assert.ok([401, 404].includes(forged.status), `a forged reply got ${forged.status}`);

  console.log(`agent-compile-browser: document_compile verified end to end through the owner's tab (${outputs.join(" and ")}; success, failure at line 3, missing file, run forbidden, forged reply refused)`);
} catch (error) {
  if (appLog) console.error("Application log:\n", appLog);
  if (tab) {
    console.error("Page at failure:", await tab.evaluate("JSON.stringify({text:document.body.innerText,frames:Array.from(document.querySelectorAll('iframe')).map(f=>f.src)})").catch(() => "unavailable"));
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
