// SPEC-transport sections 5, 6, 8 and 9, end to end: a real server, a real
// Chromium, a project of many notes and some figures.
//
//   1. opening the project (on a note that shows no figure) requests every
//      figure exactly once;
//   2. switching files requests nothing;
//   3. a second load of the project (empty memory, warm IndexedDB) makes no
//      byte-route request;
//   4. signing out deletes `librepaper-assets-v1`;
//   5. a reader on a read link, in a fresh profile, warms every figure too;
//   6. (TRANSPORT_TEST_MINIO=1, needs docker) on an S3 store the figures come
//      from the bucket through signed links and never through the byte route.
//
// Needs: LIBREPAPER_BIN (or LIBREPAPER_TEST_BINARY, or target/debug/librepaper),
// a Postgres (LIBREPAPER_TEST_POSTGRES_URL), a built web app in the binary,
// and Chromium. Skips politely when the binary or Postgres is missing.
//
// Requests are counted from inside the page, through the Resource Timing
// buffer (enlarged before any script runs), so only requests the top-level
// document makes are seen. The browser driver has no network events.
import assert from "node:assert/strict";
import { createHash, createHmac, randomBytes } from "node:crypto";
import { spawnSync } from "node:child_process";
import { chmodSync, existsSync, mkdirSync, mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { deflateSync } from "node:zlib";
import { browser, pause, removeTemporary, until } from "../helpers/browser-driver.mjs";
import { deploymentBinary, startDeployment } from "../helpers/deployment.mjs";

const NOTES = Number(process.env.TRANSPORT_TEST_NOTES || 150); // the server admits 200 files by default
const FIGURES = 20;
const WARM_LIMIT = 60000; // ms to wait for the background warm-up
const SETTLE = 2500; // ms to watch for a request that should not happen
const name = "transport-browser";
const wantMinio = process.env.TRANSPORT_TEST_MINIO === "1";
const log = (line) => console.log(`${name}: ${line}`);

const binary = process.env.LIBREPAPER_BIN ? resolve(process.env.LIBREPAPER_BIN) : deploymentBinary();
if (!existsSync(binary)) {
  log(`no librepaper binary at ${binary}; skipping (set LIBREPAPER_BIN)`);
  process.exit(0);
}

// --- the fixture ----------------------------------------------------------

const noteName = (i) => `n${String(i).padStart(3, "0")}.md`;
const figureName = (i) => `fig${String(i).padStart(2, "0")}.png`;

function crc32(bytes) {
  let crc = ~0;
  for (const byte of bytes) {
    crc ^= byte;
    for (let k = 0; k < 8; k++) crc = (crc >>> 1) ^ (0xedb88320 & -(crc & 1));
  }
  return ~crc >>> 0;
}
function chunk(type, data) {
  const head = Buffer.alloc(8);
  head.writeUInt32BE(data.length, 0);
  head.write(type, 4, "ascii");
  const tail = Buffer.alloc(4);
  tail.writeUInt32BE(crc32(Buffer.concat([head.subarray(4), data])), 0);
  return Buffer.concat([head, data, tail]);
}
/// A valid 1x1 PNG whose one pixel is different for every `i`.
function png(i) {
  const header = Buffer.alloc(13);
  header.writeUInt32BE(1, 0); header.writeUInt32BE(1, 4);
  header[8] = 8; header[9] = 2; // 8-bit RGB
  const pixel = Buffer.from([0, i * 12 % 256, 255 - i * 7, (i * 31) % 256]); // filter byte + RGB
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk("IHDR", header), chunk("IDAT", deflateSync(pixel)), chunk("IEND", Buffer.alloc(0)),
  ]);
}

const figureBytes = Array.from({ length: FIGURES }, (_, i) => png(i));
const sha256 = (bytes) => createHash("sha256").update(bytes).digest("hex");
const figureDigests = figureBytes.map(sha256);
assert.equal(new Set(figureDigests).size, FIGURES, "fixture figures must have distinct digests");
const expectedDigests = new Set(figureDigests);

const projectFiles = [
  ...Array.from({ length: NOTES }, (_, i) => [noteName(i), Buffer.from(i === 0
    ? `# Note 000\n\n![first](${figureName(0)})\n\n![second](${figureName(1)})\n`
    : `# Note ${String(i).padStart(3, "0")}\n\nPlain words for note ${i}.\n`)]),
  ...figureBytes.map((bytes, i) => [figureName(i), bytes]),
];

/// One multipart request: each file is a `file` part named by its path, which
/// is how the CLI and the web app publish a directory (`handle_upload`).
function multipart(title, files) {
  const boundary = `----lp${randomBytes(8).toString("hex")}`;
  const parts = [Buffer.from(`--${boundary}\r\nContent-Disposition: form-data; name="title"\r\n\r\n${title}\r\n`)];
  for (const [path, bytes] of files) {
    parts.push(Buffer.from(`--${boundary}\r\nContent-Disposition: form-data; name="file"; filename="${path}"\r\nContent-Type: application/octet-stream\r\n\r\n`), bytes, Buffer.from("\r\n"));
  }
  parts.push(Buffer.from(`--${boundary}--\r\n`));
  return { body: Buffer.concat(parts), type: `multipart/form-data; boundary=${boundary}` };
}

async function createProject(deployment, title) {
  const { body, type } = multipart(title, projectFiles);
  const response = await fetch(`${deployment.base}/api/documents`, {
    method: "POST",
    headers: { "content-type": type, "x-librepaper-client": "1", cookie: deployment.cookie },
    body,
  });
  const payload = await response.json().catch(() => ({}));
  if (response.status !== 201) throw new Error(`project upload failed: ${response.status} ${JSON.stringify(payload)}`);
  return payload.slug;
}

async function readLink(deployment, slug) {
  const response = await fetch(`${deployment.base}/api/documents/${slug}/share`, {
    method: "POST",
    headers: { "content-type": "application/json", "x-librepaper-client": "1", cookie: deployment.cookie },
    body: JSON.stringify({ create: { kind: "link", role: "reader", until: "1d", name: "Reader", anonymous: true } }),
  });
  const payload = await response.json().catch(() => ({}));
  if (!response.ok || !payload.created?.url) throw new Error(`read link failed: ${response.status} ${JSON.stringify(payload)}`);
  return payload.created.url;
}

// --- watching requests ----------------------------------------------------

const BUFFER_SCRIPT = "try { performance.setResourceTimingBufferSize(20000); } catch {}";
const randomPort = () => 19000 + Math.floor(Math.random() * 3000);

async function openBrowser(directory) {
  const tab = await browser(process.env.BROWSER || "chromium", directory, randomPort());
  await tab.command("Page.addScriptToEvaluateOnNewDocument", { source: BUFFER_SCRIPT });
  return tab;
}

const resourceNames = (tab) => tab.evaluate(`performance.getEntriesByType("resource").map((entry) => entry.name)`);

/// What a list of request URLs holds, for one document and (optionally) one
/// bucket host: byte-route digests, link-route digests, bucket requests.
function classify(urls, slug, bucketHost = null) {
  const route = new RegExp(`/api/documents/${slug}/assets/([0-9a-f]{64})(/link)?(?:[?#]|$)`);
  const out = { bytes: [], links: [], bucket: [] };
  for (const url of urls) {
    const hit = route.exec(url);
    if (hit) (hit[2] ? out.links : out.bytes).push(hit[1]);
    else if (bucketHost && new URL(url).host === bucketHost) out.bucket.push(url);
  }
  return out;
}
const duplicates = (list) => list.filter((one, at) => list.indexOf(one) !== at);
const describe = (seen) => `${seen.links.length} link, ${seen.bytes.length} byte-route, ${seen.bucket.length} bucket request(s)`;

function fail(scenario, expected, seen) {
  throw new Error(`${scenario}: expected ${expected}; saw ${typeof seen === "string" ? seen : describe(seen)}`);
}

/// Each figure's bytes arrive once, whichever route carries them.
function assertEveryFigureOnce(scenario, seen, { route }) {
  const dup = duplicates(seen.bytes);
  if (dup.length) fail(scenario, "no digest requested twice on the byte route", `${dup.length} repeated (${describe(seen)})`);
  const dupLinks = duplicates(seen.links);
  if (dupLinks.length) fail(scenario, "no digest asked for a link twice", `${dupLinks.length} repeated (${describe(seen)})`);
  if (route === "bytes") {
    const missing = [...expectedDigests].filter((digest) => !seen.bytes.includes(digest));
    if (missing.length || seen.bytes.length !== FIGURES) fail(scenario, `${FIGURES} figures through the byte route, each once`, `${seen.bytes.length} (missing ${missing.length}; ${describe(seen)})`);
    if (seen.links.length < 1 || seen.links.length > FIGURES) fail(scenario, `between 1 and ${FIGURES} link requests on a file store`, describe(seen));
  }
}

const storeCount = (tab) => tab.evaluate(`(async () => {
  const open = indexedDB.open("librepaper-assets-v1");
  const db = await new Promise((done, fail) => { open.onsuccess = () => done(open.result); open.onerror = () => fail(open.error); });
  try {
    if (!db.objectStoreNames.contains("bytes")) return -1;
    return await new Promise((done, fail) => {
      const request = db.transaction("bytes").objectStore("bytes").count();
      request.onsuccess = () => done(request.result);
      request.onerror = () => fail(request.error);
    });
  } finally { db.close(); }
})()`);

const databases = (tab) => tab.evaluate(`indexedDB.databases().then((all) => all.map((one) => one.name))`);

async function signIn(tab, deployment) {
  const value = deployment.cookie.slice(deployment.cookie.indexOf("=") + 1);
  await tab.setCookie("librepaper_session", value, deployment.browserBase);
}

async function openProject(tab, deployment, slug, { file = noteName(5), path = null } = {}) {
  await tab.resize(1300, 900);
  await tab.navigate(path ? `${deployment.browserBase}${path}` : `${deployment.browserBase}/docs/${slug}?file=${encodeURIComponent(file)}`);
  // An editor gets the source editor; a reader gets the project's file list
  // and the preview, with no editor.
  await until("the project", () => tab.evaluate(`Boolean(document.querySelector(".cm-content") || document.querySelector(".explorer-name"))`), 45000);
}

const settle = () => pause(SETTLE);

/// A real pointer click on the first element the expression finds.
async function click(tab, expression, what) {
  const point = await tab.evaluate(`(() => {
    const element = (${expression});
    if (!element) return null;
    element.scrollIntoView({ block: "center" });
    const box = element.getBoundingClientRect();
    return { x: box.x + box.width / 2, y: box.y + box.height / 2 };
  })()`);
  if (!point) throw new Error(`could not find ${what}`);
  await tab.command("Input.dispatchMouseEvent", { type: "mouseMoved", ...point });
  await tab.command("Input.dispatchMouseEvent", { type: "mousePressed", ...point, button: "left", clickCount: 1 });
  await tab.command("Input.dispatchMouseEvent", { type: "mouseReleased", ...point, button: "left", clickCount: 1 });
}

const editorText = (tab) => tab.evaluate(`document.querySelector(".cm-content")?.innerText || ""`);

async function switchToNote(tab, index) {
  const wanted = noteName(index);
  const heading = `Note ${String(index).padStart(3, "0")}`;
  const row = `[...document.querySelectorAll(".explorer-name")].find((one) => one.textContent.trim() === ${JSON.stringify(wanted)})`;
  await click(tab, row, `the file row ${wanted} in the explorer`);
  try {
    await until(`${wanted} in the editor`, async () => (await editorText(tab)).includes(heading), 5000);
  } catch {
    // Some builds open on a double click only.
    await click(tab, row, `the file row ${wanted} in the explorer`);
    await click(tab, row, `the file row ${wanted} in the explorer`);
    await until(`${wanted} in the editor`, async () => (await editorText(tab)).includes(heading), 10000);
  }
}

async function warmedBytes(tab, slug, scenario, mark = 0) {
  // Wait until every figure has arrived by some route, then watch a while
  // longer for anything that comes twice.
  let seen;
  await until(`${scenario}: every figure requested`, async () => {
    seen = classify((await resourceNames(tab)).slice(mark), slug);
    return seen.bytes.length >= FIGURES;
  }, WARM_LIMIT).catch(() => {});
  await settle();
  return classify((await resourceNames(tab)).slice(mark), slug);
}

// --- scenarios on a file store -------------------------------------------

async function fileStoreScenarios(scratch) {
  // Scenario 5 opens a read link in a fresh profile, without signing in.
  const deployment = await startDeployment({ label: "transport_files", binary, advanced: "anonymous_reads = true\n" }); // appended inside the helper's [access] table
  if (!deployment) return log("no binary; skipping") ?? false;
  if (deployment.unavailable) return log(`${deployment.unavailable}; skipping`) ?? false;
  const tabs = [];
  try {
    const slug = await createProject(deployment, "Transport project");
    log(`project ${slug}: ${NOTES} notes, ${FIGURES} figures`);
    const tab = await openBrowser(join(scratch, "owner"));
    tabs.push(tab);
    await signIn(tab, deployment);

    // 1. Open on a note without figures.
    await openProject(tab, deployment, slug, { file: noteName(5) });
    const first = await warmedBytes(tab, slug, "scenario 1");
    log(`1 open on note 5: ${describe(first)}`);
    assertEveryFigureOnce("scenario 1 (open the project)", first, { route: "bytes" });
    const stored = await storeCount(tab);
    if (stored !== FIGURES) fail("scenario 1 (persistent store)", `${FIGURES} entries in librepaper-assets-v1`, `${stored}`);

    // 2. File switches cost nothing.
    const mark = (await resourceNames(tab)).length;
    await switchToNote(tab, 0);
    await settle();
    await switchToNote(tab, 7);
    await settle();
    const switched = classify((await resourceNames(tab)).slice(mark), slug);
    log(`2 switch to note 0, note 7: ${describe(switched)}`);
    if (switched.bytes.length || switched.links.length) fail("scenario 2 (switch files)", "no asset request", switched);

    // 3. A second load: new memory, warm IndexedDB.
    await openProject(tab, deployment, slug, { file: noteName(5) });
    await settle();
    const second = classify(await resourceNames(tab), slug);
    log(`3 second load: ${describe(second)}`);
    if (second.bytes.length) fail("scenario 3 (second load)", "no byte-route request", second);

    // 4. Sign-out deletes the asset database.
    if (!(await databases(tab)).includes("librepaper-assets-v1")) fail("scenario 4 (before sign-out)", "librepaper-assets-v1 to exist", JSON.stringify(await databases(tab)));
    await tab.navigate(deployment.browserBase);
    await until("the account menu", () => tab.evaluate(`Boolean(document.querySelector("button.account"))`), 30000);
    await click(tab, `document.querySelector("button.account")`, "the account menu");
    await until("the sign-out item", () => tab.evaluate(`[...document.querySelectorAll('[role="menuitem"]')].some((one) => /sign out/i.test(one.textContent) && one.getClientRects().length > 0)`), 5000);
    // The menu (Skeleton, zag.js) selects on pointer events after the item is
    // highlighted, so hover it first, then press and release.
    const item = `[...document.querySelectorAll('[role="menuitem"]')].find((one) => /sign out/i.test(one.textContent) && one.getClientRects().length > 0)`;
    await pause(300);
    const at = await tab.evaluate(`(() => { const box = (${item}).getBoundingClientRect(); return { x: box.x + box.width / 2, y: box.y + box.height / 2 }; })()`);
    await tab.command("Input.dispatchMouseEvent", { type: "mouseMoved", ...at });
    await pause(200);
    await tab.command("Input.dispatchMouseEvent", { type: "mousePressed", ...at, button: "left", clickCount: 1 });
    await pause(50);
    await tab.command("Input.dispatchMouseEvent", { type: "mouseReleased", ...at, button: "left", clickCount: 1 });
    await until("the sign-in page", () => tab.evaluate(`location.pathname.startsWith("/auth/")`), 15000);
    const left = await databases(tab);
    log(`4 sign out: databases left ${JSON.stringify(left)}`);
    if (left.includes("librepaper-assets-v1")) fail("scenario 4 (sign out)", "librepaper-assets-v1 deleted", JSON.stringify(left));

    // 5. A reader, in a fresh profile.
    const link = await readLink(deployment, slug);
    const reader = await openBrowser(join(scratch, "reader"));
    tabs.push(reader);
    await openProject(reader, deployment, slug, { path: link });
    const read = await warmedBytes(reader, slug, "scenario 5");
    log(`5 reader on a read link: ${describe(read)}`);
    assertEveryFigureOnce("scenario 5 (reader)", read, { route: "bytes" });
    return true;
  } catch (error) {
    if (deployment.log) console.error("Application log:\n", deployment.log.slice(-4000));
    for (const open of tabs) {
      console.error("Page at failure:", await open.evaluate("JSON.stringify({url:location.href,text:document.body.innerText.slice(0,500)})").catch(() => "unavailable"));
    }
    throw error;
  } finally {
    for (const open of tabs) await open.close().catch(() => {});
    await deployment.stop();
  }
}

// --- scenario 6: an S3 emulator ------------------------------------------

const docker = (...args) => spawnSync("docker", args, { encoding: "utf8" });
const hmac = (key, data) => createHmac("sha256", key).update(data).digest();

/// Creates a bucket with a SigV4-signed PUT, so no client tool is needed.
async function createBucket(endpoint, bucket, access, secret) {
  const url = new URL(`${endpoint}/${bucket}`);
  const now = new Date().toISOString().replace(/[:-]|\.\d{3}/g, "");
  const day = now.slice(0, 8);
  const empty = sha256("");
  const signed = "host;x-amz-content-sha256;x-amz-date";
  const canonical = `PUT\n/${bucket}\n\nhost:${url.host}\nx-amz-content-sha256:${empty}\nx-amz-date:${now}\n\n${signed}\n${empty}`;
  const scope = `${day}/us-east-1/s3/aws4_request`;
  const toSign = `AWS4-HMAC-SHA256\n${now}\n${scope}\n${sha256(canonical)}`;
  const key = hmac(hmac(hmac(hmac(`AWS4${secret}`, day), "us-east-1"), "s3"), "aws4_request");
  const signature = createHmac("sha256", key).update(toSign).digest("hex");
  const response = await fetch(url, {
    method: "PUT",
    headers: {
      "x-amz-date": now, "x-amz-content-sha256": empty,
      authorization: `AWS4-HMAC-SHA256 Credential=${access}/${scope}, SignedHeaders=${signed}, Signature=${signature}`,
    },
  });
  if (!response.ok) throw new Error(`creating the bucket failed: ${response.status} ${await response.text()}`);
}

async function s3Scenario(scratch) {
  if (!wantMinio) return log("6 S3: skipped (set TRANSPORT_TEST_MINIO=1 to run it against MinIO in docker)");
  if (docker("version").status !== 0) return log("6 S3: skipped (docker is not available)");
  const access = "transport-access";
  const secret = "transport-secret-password";
  const bucket = "transport-bucket";
  const container = `lp-transport-minio-${randomBytes(4).toString("hex")}`;
  let deployment = null;
  const tabs = [];
  try {
    const started = docker("run", "-d", "--name", container, "-p", "127.0.0.1::9000",
      "-e", `MINIO_ROOT_USER=${access}`, "-e", `MINIO_ROOT_PASSWORD=${secret}`,
      process.env.TRANSPORT_TEST_MINIO_IMAGE || process.env.BACKUP_TEST_MINIO_IMAGE || "quay.io/minio/minio:latest",
      "server", "/data", "--address", ":9000");
    if (started.status !== 0) throw new Error(`could not start MinIO: ${started.stderr}`);
    const port = docker("port", container, "9000/tcp").stdout.split("\n")[0].split(":").pop().trim();
    const endpoint = `http://127.0.0.1:${port}`;
    await until("MinIO", async () => (await fetch(`${endpoint}/minio/health/live`)).ok, 30000);
    // MinIO accepts connections before it has finished starting and answers
    // 503 XMinioServerNotInitialized meanwhile: retry until it is ready.
    await until("MinIO ready for the bucket", async () => {
      try { await createBucket(endpoint, bucket, access, secret); return true; }
      catch (error) { if (/XMinioServerNotInitialized|ECONNREFUSED|fetch failed/.test(String(error?.message || error) + String(error?.cause?.code || ""))) return false; throw error; }
    }, 60000);
    // MinIO answers every origin by default, so the CORS rule the spec asks
    // of a real bucket is already satisfied here.

    // startDeployment writes a filesystem store and takes no override, so a
    // wrapper turns that line into "s3" before the real binary reads it.
    const wrapper = join(scratch, "serve-s3.sh");
    writeFileSync(wrapper, `#!/bin/sh\nsed -i 's/^object_store = "filesystem"$/object_store = "s3"/' "$4"\nexec ${JSON.stringify(binary)} "$@"\n`);
    chmodSync(wrapper, 0o755);
    deployment = await startDeployment({
      label: "transport_s3", binary: wrapper,
      advanced: ["[storage.s3]", `endpoint = "${endpoint}"`, 'region = "us-east-1"', `bucket = "${bucket}"`,
        `access_key_id = "${access}"`, `secret_access_key = "${secret}"`, "allow_http = true", ""].join("\n"),
    });
    if (!deployment || deployment.unavailable) return log("6 S3: skipped (no deployment)");
    const slug = await createProject(deployment, "Transport S3 project");
    const tab = await openBrowser(join(scratch, "s3"));
    tabs.push(tab);
    await signIn(tab, deployment);
    await openProject(tab, deployment, slug, { file: noteName(5) });
    const host = `127.0.0.1:${port}`;
    let seen;
    await until("figures from the bucket", async () => {
      seen = classify(await resourceNames(tab), slug, host);
      return seen.bucket.length >= FIGURES;
    }, WARM_LIMIT).catch(() => {});
    await settle();
    seen = classify(await resourceNames(tab), slug, host);
    log(`6 S3 open: ${describe(seen)}`);
    if (seen.bytes.length) fail("scenario 6 (S3)", "no byte-route request", seen);
    if (seen.links.length !== FIGURES || duplicates(seen.links).length) fail("scenario 6 (S3)", `${FIGURES} link requests, one per digest`, seen);
    if (seen.bucket.length !== FIGURES) fail("scenario 6 (S3)", `${FIGURES} fetches from the bucket host ${host}`, seen);
  } catch (error) {
    if (deployment?.log) console.error("Application log:\n", deployment.log.slice(-4000));
    throw error;
  } finally {
    for (const open of tabs) await open.close().catch(() => {});
    await deployment?.stop();
    docker("rm", "-f", "-v", container);
  }
}

// --- run -------------------------------------------------------------------

const scratch = mkdtempSync(join(tmpdir(), "librepaper-transport-"));
try {
  mkdirSync(scratch, { recursive: true });
  const ran = await fileStoreScenarios(scratch);
  if (ran) await s3Scenario(scratch);
  if (ran) log("passed");
} finally {
  removeTemporary(scratch);
}
