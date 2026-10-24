// End-to-end browser check for the no-retained-renderings contract.
//
//   node web/tests/e2e/latex-e2e.mjs <binary> <mode> <fixture> [seconds] [mirror]
//
// `mode` is `browser` (the reader's browser compiler), the only mode: a
// browser no longer reaches the desktop app, so there is no local variant. A
// reader opens the published read link, so its only document authority is the
// fragment key. Publishing itself uses a locally minted, signed GitHub test
// session; no OAuth service is contacted.
import { existsSync, mkdtempSync, readFileSync, readdirSync, statSync } from "node:fs";
import { tmpdir } from "node:os";
import { basename, dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { browser, until, removeTemporary } from "../helpers/browser-driver.mjs";
import { ephemeralMirror } from "../helpers/ephemeral-mirror.mjs";
import { deploymentBinary, sessionCookie, startDeployment } from "../helpers/deployment.mjs";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..", "..");
const BINARY = resolve(process.argv[2] || deploymentBinary());
const MODE = process.argv[3] || "browser";
const REQUESTED_FIXTURE = resolve(process.argv[4] || join(ROOT, "tools", "test", "latex", "corpus", "e2e", "biber"));
const WAIT = Number(process.argv[5] || 300);
const MIRROR_ARG = process.argv[6] || process.env.MIRROR || join(ROOT, "..", "wasm-latex", "mirror");
let BASE;
const scratch = mkdtempSync(join(tmpdir(), "librepaper-latex-e2e-"));
let mirror = null;
let deployment = null;

const wait = (ms) => new Promise((done) => setTimeout(done, ms));
const shellHeaders = { "x-librepaper-client": "shell" };

function fixtureTree(directory, main) {
  const files = (dir, prefix = "") => readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    if (entry.name.startsWith(".") || entry.name === "logs") return [];
    const relative = prefix + entry.name;
    if (entry.isSymbolicLink()) throw new Error(`fixture symlink: ${relative}`);
    return entry.isDirectory() ? files(join(dir, entry.name), `${relative}/`) : [relative];
  });
  const form = new FormData();
  form.set("title", "LaTeX browser smoke");
  form.set("main", main);
  for (const path of files(directory).sort()) {
    form.append("file", new Blob([readFileSync(join(directory, path))]), path);
  }
  return form;
}

async function publish(cookie, fixture) {
  // A directory is published with main.tex as its main file; a .tex file is
  // published with everything beside it (its \input files, figures and
  // bibliography), as an author uploading that project would.
  const body = statSync(fixture).isDirectory()
    ? fixtureTree(fixture, "main.tex")
    : fixtureTree(dirname(fixture), basename(fixture));
  const headers = { ...shellHeaders, cookie };
  const response = await fetch(`${BASE}/api/documents`, { method: "POST", headers, body });
  const value = await response.json();
  if (!response.ok || typeof value.share_url !== "string") throw new Error(`publish failed (${response.status}): ${JSON.stringify(value)}`);
  const link = new URL(value.share_url, BASE);
  const key = new URLSearchParams(link.hash.slice(1)).get("k");
  if (!key) throw new Error(`publish returned no reader key: ${link}`);
  return { url: link.href, slug: link.pathname.split("/").filter(Boolean).pop(), key };
}

async function main() {
  if (MODE !== "browser") throw new Error(`unknown mode ${MODE}; use browser`);
  const fixture = process.argv[4]
    ? REQUESTED_FIXTURE
    : existsSync(REQUESTED_FIXTURE)
      ? REQUESTED_FIXTURE
      : join(ROOT, "tools", "test", "latex", "corpus", "e2e", "native");
  if (!existsSync(fixture) || !(statSync(fixture).isDirectory() || statSync(fixture).isFile())) {
    throw new Error(`fixture file or directory does not exist: ${fixture}`);
  }

  const mirrorUrl = /^https:\/\//i.test(MIRROR_ARG)
    ? MIRROR_ARG.replace(/\/?$/, "/")
    : (mirror = await ephemeralMirror(MIRROR_ARG)).latexUrl;
  // The server takes the asset mirror; the pinned release is beneath it at latex/<id>/.
  const assetMirror = new URL("../../", mirrorUrl).href;
  deployment = await startDeployment({
    label: "latex_e2e",
    binary: BINARY,
    advanced: `[assets]\nmirror = ${JSON.stringify(assetMirror)}\n`,
  });
  if (!deployment || deployment.unavailable) {
    throw new Error(deployment?.unavailable || `no librepaper binary at ${BINARY}; build it first`);
  }
  BASE = deployment.browserBase;
  console.log("origin baseline " + JSON.stringify(await (await fetch(`${BASE}/api/status`)).json()));
  const published = await publish(deployment.cookie, fixture);
  console.log(`published ${published.slug}; reader key only; mirror ${mirrorUrl}`);
  const readerId = deployment.postgres.seedRegisteredAccount({
    provider: "github", subject: "github:latex-reader", handle: "latex-reader", displayName: "LaTeX Reader",
  });
  const readerCookie = sessionCookie({
    dataDirectory: deployment.data, accountId: readerId, handle: "latex-reader", name: "LaTeX Reader",
  });

  const targetUrl = published.url;
  process.env.LIBREPAPER_BROWSER_IGNORE_CERT_ERRORS = "1";
  const tab = await browser(process.env.BROWSER || "chromium", join(scratch, "profile"), 9500 + Math.floor(Math.random() * 500));
  try {
    await tab.resize(1300, 900);
    // Any same-origin page will do for the cookie. Not the landing page: signed
    // out, it can redirect to the sign-in provider before the load completes.
    await tab.navigate(`${BASE}/theme.js`);
    await tab.setCookie("librepaper_session", readerCookie.slice(readerCookie.indexOf("=") + 1), BASE);
    // A wait that, when it runs out, says what the reader was showing and
    // which mirror files it fetched, rather than only that time ran out.
    let seen = "";
    const explained = (label, check) => until(label, check, WAIT * 1000).catch(async (error) => {
      const page = await tab.evaluate("document.body.innerText").catch(() => "");
      const fetched = await tab.evaluate(`performance.getEntriesByType("resource")
        .filter((entry) => /latex|wasm/.test(entry.name))
        .map((entry) => entry.responseStatus + " " + entry.name.replace(/^https?:\\/\\/[^/]+/, ""))
        .join("\\n")`).catch(() => "");
      const squash = (value) => String(value).replace(/\s+/g, " ").slice(0, 600);
      throw new Error(`${error.message}; document: ${squash(seen)}; reader page: ${squash(page)}\nmirror requests:\n${fetched}`);
    });

    // LaTeX opens as the LaTeXML HTML preview.
    await tab.navigate(targetUrl);
    const started = Date.now();
    await explained("reader HTML preview", async () => {
      const text = seen = await tab.text().catch(() => "");
      return /Knuth|LaTeX|browser|paragraph/i.test(text) && !/not yet rendered|rendering from source/i.test(text);
    });
    console.log(`reader HTML preview drawn in ${((Date.now() - started) / 1000).toFixed(1)}s`);

    // A reader is shown LaTeX as HTML; choosing pages is an editor's View menu.
    if ((fixture.endsWith("/biber") || fixture.endsWith("\\biber")) && !/Knuth/i.test(seen)) {
      throw new Error("Biber fixture rendered without its resolved Knuth citation");
    }

    const resources = await tab.evaluate("performance.getEntriesByType('resource').map(e => e.name)");
    const origin = new URL(BASE).origin;
    if (resources.some((url) => new URL(url).origin === origin && new URL(url).pathname.startsWith("/latex/"))) throw new Error("reader fetched compiler bytes through the origin /latex route");
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
  await deployment?.stop();
  mirror?.server.close();
  await wait(250);
  for (const directory of [scratch, mirror?.tls].filter(Boolean)) removeTemporary(directory);
}
