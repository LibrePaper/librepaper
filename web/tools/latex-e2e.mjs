// End-to-end browser check for the no-retained-renderings contract.
//
//   node web/tools/latex-e2e.mjs <binary> <mode> <fixture> [seconds] [mirror]
//
// `mode` is `browser` (the reader's browser compiler) or `local` (the same
// reader check, with an optional local companion available for fallback). A
// reader opens the published read link, so its only document authority is the
// fragment key. Publishing itself uses a locally minted, signed GitHub test
// session; no OAuth service is contacted.
import { createHmac } from "node:crypto";
import { spawn, execFileSync } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, readdirSync, rmSync, statSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { browser, until } from "./browser-driver.mjs";
import { ephemeralMirror } from "./ephemeral-mirror.mjs";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..");
const BINARY = resolve(process.argv[2] || "target/debug/librepaper");
const MODE = process.argv[3] || "browser";
const REQUESTED_FIXTURE = resolve(process.argv[4] || join(ROOT, "tools", "latex", "corpus", "e2e", "biber"));
const WAIT = Number(process.argv[5] || 300);
const MIRROR_ARG = process.argv[6] || process.env.MIRROR || join(ROOT, "..", "wasm-latex", "mirror");
const PORT = 8600 + Math.floor(Math.random() * 200);
const BASE = `http://localhost:${PORT}`;
const scratch = mkdtempSync(join(tmpdir(), "librepaper-latex-e2e-"));
const data = mkdtempSync(join(tmpdir(), "librepaper-latex-data-"));
const config = mkdtempSync(join(tmpdir(), "librepaper-latex-config-"));
let mirror = null;
let server = null;
let local = null;

const wait = (ms) => new Promise((done) => setTimeout(done, ms));
const shellHeaders = { "x-librepaper-client": "shell" };

function sql(value) {
  return `'${String(value).replaceAll("'", "''")}'`;
}

// This is the same v2 envelope as the OAuth callback writes. The catalogue
// row is seeded below so the configured provider and session generation are
// checked by the real binary before it accepts the publication.
function signedGithubSession() {
  const key = Buffer.from(readFileSync(join(data, "secrets", "session.key"), "utf8").trim(), "hex");
  const generation = "latex-browser-test-generation";
  const expires = Math.floor(Date.now() / 1000) + 3600;
  const raw = `github|browser-test|github:browser-test|${generation}||Browser Test|${expires}`;
  const payload = Buffer.from(raw).toString("base64url");
  const signature = createHmac("sha256", key).update(`session-v2\0${payload}`).digest("base64url");
  return { cookie: `librepaper_session=v2.${payload}.${signature}`, generation };
}

function seedAccount(generation) {
  const db = join(data, "catalog.db");
  const now = new Date().toISOString();
  const statement = `INSERT INTO accounts
    (id, provider, handle, name, email, first_seen, last_seen, plan, status, session_generation, erasure_cursor)
    VALUES (${sql("github:browser-test")}, ${sql("github")}, ${sql("browser-test")}, ${sql("Browser Test")}, '',
      ${sql(now)}, ${sql(now)}, ${sql("test")}, 'active', ${sql(generation)}, NULL);`;
  execFileSync("sqlite3", [db, statement], { stdio: "ignore" });
}

function fixtureTree(directory) {
  const files = (dir, prefix = "") => readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    if (entry.name.startsWith(".") || entry.name === "logs") return [];
    const relative = prefix + entry.name;
    if (entry.isSymbolicLink()) throw new Error(`fixture symlink: ${relative}`);
    return entry.isDirectory() ? files(join(dir, entry.name), `${relative}/`) : [relative];
  });
  const form = new FormData();
  form.set("title", "LaTeX browser smoke");
  form.set("main", "main.tex");
  for (const path of files(directory).sort()) {
    form.append("file", new Blob([readFileSync(join(directory, path))]), path);
  }
  return form;
}

async function publish(cookie, fixture) {
  const body = statSync(fixture).isDirectory()
    ? fixtureTree(fixture)
    : JSON.stringify({ title: "LaTeX browser smoke", source_format: "latex", source: readFileSync(fixture, "utf8") });
  const headers = { ...shellHeaders, cookie };
  if (typeof body === "string") headers["content-type"] = "application/json";
  const response = await fetch(`${BASE}/api/documents`, { method: "POST", headers, body });
  const value = await response.json();
  if (!response.ok || typeof value.share_url !== "string") throw new Error(`publish failed (${response.status}): ${JSON.stringify(value)}`);
  const link = new URL(value.share_url, BASE);
  const key = new URLSearchParams(link.hash.slice(1)).get("k");
  if (!key) throw new Error(`publish returned no reader key: ${link}`);
  return { url: link.href, slug: link.pathname.split("/").filter(Boolean).pop(), key };
}

async function main() {
  if (!(MODE === "browser" || MODE === "local")) throw new Error(`unknown mode ${MODE}; use browser or local`);
  const fixture = existsSync(REQUESTED_FIXTURE)
    ? REQUESTED_FIXTURE
    : join(ROOT, "tools", "latex", "corpus", "e2e", "native");
  if (!statSync(fixture)) throw new Error(`fixture does not exist: ${fixture}`);

  const mirrorUrl = /^https:\/\//i.test(MIRROR_ARG)
    ? MIRROR_ARG.replace(/\/?$/, "/")
    : (mirror = await ephemeralMirror(MIRROR_ARG)).url;
  const args = [
    "admin", "serve", "--port", String(PORT), "--data-directory", data,
    "--publishers", "any", "--commenters", "anyone", "--latex-mirror", mirrorUrl,
  ];
  server = spawn(BINARY, args, {
    stdio: ["ignore", "ignore", "pipe"],
    env: { ...process.env, LIBREPAPER_GITHUB_CLIENT_ID: "test-client", LIBREPAPER_GITHUB_CLIENT_SECRET: "test-secret" },
  });
  let serverLog = "";
  server.stderr.on("data", (bytes) => { serverLog += String(bytes); });
  await until("serve startup", async () => {
    if (server.exitCode !== null) throw new Error(serverLog);
    return (await fetch(`${BASE}/api/config`)).ok;
  }, 30000);
  console.log("origin baseline " + JSON.stringify(await (await fetch(`${BASE}/api/status`)).json()));
  const session = signedGithubSession();
  seedAccount(session.generation);
  const published = await publish(session.cookie, fixture);
  console.log(`published ${published.slug}; reader key only; mirror ${mirrorUrl}`);

  const targetUrl = published.url;
  let localPairing = null;
  if (MODE === "local") {
    local = spawn(BINARY, ["local", "start", "--port", "8763"], {
      stdio: "ignore", env: { ...process.env, XDG_CONFIG_HOME: config, XDG_CACHE_HOME: join(config, "cache") },
    });
    let code = null;
    await until("local pairing code", async () => {
      try { code = JSON.parse(readFileSync(join(config, "librepaper", "local", "service.json"), "utf8")).code; } catch {}
      return Boolean(code);
    }, 30000);
    const health = await (await fetch("http://127.0.0.1:8763/librepaper/local/v1/health")).json();
    const connected = await fetch("http://127.0.0.1:8763/librepaper/local/v1/connect", {
      method: "POST", headers: { "content-type": "application/json", origin: BASE },
      body: JSON.stringify({ origin: BASE, project: published.slug, code }),
    });
    const pairing = await connected.json();
    if (!connected.ok || !pairing.token) throw new Error(`local pairing failed (${connected.status}): ${JSON.stringify(pairing)}`);
    localPairing = { token: pairing.token, expires: pairing.expires, instance: health.instance };
    // The local mode keeps the companion fallback reachable for the Biber
    // fixture. Browser Biber remains preferred by the renderer; the local
    // pairing is selected only after browser resource failure.

  }

  process.env.LIBREPAPER_BROWSER_IGNORE_CERT_ERRORS = "1";
  const tab = await browser(process.env.BROWSER || "chromium", join(scratch, "profile"), 9500 + Math.floor(Math.random() * 500));
  try {
    await tab.resize(1300, 900);
    if (localPairing) {
      await tab.navigate(BASE);
      await tab.evaluate(`localStorage.setItem("librepaper-local-pairings", JSON.stringify({ [location.origin + "|" + ${JSON.stringify(published.slug)}]: ${JSON.stringify(localPairing)} }))`);
    }
    await tab.navigate(targetUrl);
    const started = Date.now();
    await until("reader source compile", async () => {
      const text = await tab.text().catch(() => "");
      return /Knuth|LaTeX|browser|paragraph/i.test(text) && !/not yet rendered|rendering from source/i.test(text);
    }, WAIT * 1000);
    const elapsed = ((Date.now() - started) / 1000).toFixed(1);
    const frame = await until("PDF frame", async () => {
      const url = await tab.evaluate("document.querySelector('iframe[title=Document]')?.src || ''");
      return /\/pdf\//.test(url);
    }, 30000).then(() => tab.evaluate("document.querySelector('iframe[title=Document]').src"));
    const drawn = await until("PDF pages and text layer", async () => {
      const value = await tab.frameEvaluate("({pages:document.querySelectorAll('.page canvas').length, text:document.querySelector('.textLayer')?.textContent || ''})").catch(() => null);
      return value?.pages > 0 && value.text.trim().length > 0;
    }, 30000).then(() => tab.frameEvaluate("({pages:document.querySelectorAll('.page canvas').length, text:document.querySelector('.textLayer')?.textContent || ''})"));
    if (fixture.endsWith("/biber") || fixture.endsWith("\\biber")) {
      if (!/Knuth/i.test(drawn.text)) throw new Error("Biber fixture rendered without its resolved Knuth citation");
    }
    console.log(`reader PDF drawn in ${elapsed}s: frame=${frame}, pages=${drawn.pages}`);

    // Capture the download Blob created by the File -> Download PDF command.
    // This proves the active tab still owns the transient bytes and exports
    // them locally; it never asks the origin for a generated object.
    await tab.evaluate(`(() => {
      const original = URL.createObjectURL;
      window.__librepaperExport = null;
      URL.createObjectURL = (blob) => {
        if (blob?.type === 'application/pdf') blob.arrayBuffer().then(buffer => {
          const bytes = new Uint8Array(buffer);
          window.__librepaperExport = { type: blob.type, size: bytes.length, header: String.fromCharCode(...bytes.slice(0, 5)) };
        });
        return original(blob);
      };
    })()`);
    await tab.evaluate("Array.from(document.querySelectorAll('button')).find(b => b.textContent.trim() === 'File')?.click()");
    await until("PDF export menu", () => tab.evaluate("Array.from(document.querySelectorAll('[role=menuitem]')).some(e => e.textContent.trim() === 'Download PDF')"), 10000);
    await tab.evaluate(`(() => {
      const item = Array.from(document.querySelectorAll('[role=menuitem]')).find(e => e.textContent.trim() === 'Download PDF' && e.getClientRects().length);
      if (!item || item.getAttribute('aria-disabled') === 'true') throw new Error('PDF download is unavailable after rendering');
      item.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true, pointerType: 'mouse', button: 0 }));
      item.click();
    })()`);
    const exported = await until("transient PDF export", async () => tab.evaluate("window.__librepaperExport"), 10000).then(() => tab.evaluate("window.__librepaperExport"));
    if (exported?.header !== "%PDF-" || !exported.size) throw new Error(`PDF export was not a real transient PDF: ${JSON.stringify(exported)}`);
    console.log(`transient PDF export ${exported.size} bytes verified`);

    const resources = await tab.evaluate("performance.getEntriesByType('resource').map(e => e.name)");
    if (resources.some((url) => new URL(url).pathname.startsWith("/latex/"))) throw new Error("reader fetched compiler bytes through the origin /latex route");
    if (!resources.some((url) => url.startsWith(mirrorUrl))) throw new Error(`reader did not fetch compiler assets directly from ${mirrorUrl}`);
    console.log("cold reader used direct HTTPS mirror and no origin /latex route");

    const readHeaders = { ...shellHeaders, "x-librepaper-key": published.key };
    for (const path of [`/api/documents/${published.slug}/renderings/latest`, `/api/documents/${published.slug}/renderings/${"0".repeat(64)}`]) {
      const response = await fetch(`${BASE}${path}`, { headers: readHeaders });
      if (response.status !== 404) throw new Error(`rendered output route ${path} returned ${response.status}, expected 404`);
    }
    console.log("origin has no retained rendering route");
    console.log("origin after cold compile " + JSON.stringify(await (await fetch(`${BASE}/api/status`)).json()));
  } finally {
    await tab.close();
  }
}

try {
  await main();
} catch (error) {
  console.error(`E2E FAILED: ${error.message}`);
  process.exitCode = 1;
} finally {
  server?.kill();
  local?.kill("SIGINT");
  mirror?.server.close();
  await wait(250);
  for (const directory of [data, config, scratch, mirror?.tls].filter(Boolean)) rmSync(directory, { recursive: true, force: true, maxRetries: 3 });
}
