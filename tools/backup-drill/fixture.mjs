#!/usr/bin/env node
// Populate the drill's source deployment through the real server: start
// `admin serve` on the given database and data directory, sign one account in,
// publish the five tutorial projects, compact each to a snapshot, republish two
// deliberate source edits per project, then stop. This exercises snapshot,
// update-log, and history-label recovery through the real application paths.
//
// usage: fixture.mjs <binary> <database-url> <data-directory>

import { execFileSync, spawn } from "node:child_process";
import { randomUUID } from "node:crypto";
import { readdirSync, readFileSync, writeFileSync } from "node:fs";
import { createServer } from "node:net";
import { join, relative, resolve } from "node:path";
import { sessionCookie, until } from "../../web/tests/helpers/deployment.mjs";

const repository = resolve(import.meta.dirname, "../..");
const [binary, databaseUrl, dataDirectory] = process.argv.slice(2);
if (!binary || !databaseUrl || !dataDirectory) {
  console.error("usage: fixture.mjs <binary> <database-url> <data-directory>");
  process.exit(2);
}

// The main file of each tutorial is `librepaper.<ext>`; everything else in the
// directory, subdirectories included, is sent beside it.
const TUTORIALS = { markdown: "Markdown", typst: "Typst", latex: "LaTeX", html: "HTML", quarto: "Quarto" };
const REVISION_COMMENT = {
  markdown: (n) => `\n\n<!-- Recovery drill revision ${n} -->\n`,
  typst: (n) => `\n\n// Recovery drill revision ${n}\n`,
  latex: (n) => `\n\n% Recovery drill revision ${n}\n`,
  html: (n) => `\n<!-- Recovery drill revision ${n} -->\n`,
  quarto: (n) => `\n\n<!-- Recovery drill revision ${n} -->\n`,
};

function filesUnder(root, directory = root) {
  return readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    const path = join(directory, entry.name);
    if (entry.isDirectory()) return filesUnder(root, path);
    return entry.isFile() ? [relative(root, path)] : [];
  });
}

async function freePort() {
  const listener = createServer();
  await new Promise((done, fail) => {
    listener.once("error", fail);
    listener.listen(0, "127.0.0.1", done);
  });
  const { port } = listener.address();
  await new Promise((done) => listener.close(done));
  return port;
}

const sql = (value) => `'${String(value).replaceAll("'", "''")}'`;

async function publishProject(format, name, cookie, { slug = "", revision = 0 } = {}) {
  const root = join(repository, "docs/examples", `tutorial-${format}`);
  const paths = filesUnder(root);
  const main = paths.find((path) => /^librepaper\.[a-z]+$/.test(path) && !path.endsWith(".png"));
  if (!main) throw new Error(`no main source file found for ${format}`);
  const form = new FormData();
  form.append("main", main);
  form.append("title", `Learn LibrePaper with ${name}`);
  if (slug) form.append("slug", slug);
  for (const path of paths) {
    const source = readFileSync(join(root, path));
    const bytes = path === main && revision
      ? Buffer.concat([source, Buffer.from(REVISION_COMMENT[format](revision))])
      : source;
    form.append("file", new Blob([bytes]), path);
  }
  const response = await fetch(`${base}/api/documents`, {
    method: "POST",
    headers: { "x-librepaper-client": "1", cookie },
    body: form,
  });
  if (response.status !== 201) {
    throw new Error(`publishing ${format}${revision ? ` revision ${revision}` : ""} failed: ${response.status} ${await response.text()}`);
  }
  return response.json();
}

async function trimToSnapshot(slug, cookie) {
  const response = await fetch(`${base}/api/documents/${slug}/history/trim`, {
    method: "POST",
    headers: { "content-type": "application/json", "x-librepaper-client": "1", cookie },
    body: "{}",
  });
  if (!response.ok) throw new Error(`compacting ${slug} failed: ${response.status} ${await response.text()}`);
}

async function assertRevision(slug, revision, format, cookie) {
  const response = await fetch(`${base}/api/documents/${slug}/source`, {
    headers: { "x-librepaper-client": "1", cookie },
  });
  if (!response.ok) throw new Error(`reading source for ${slug} failed: ${response.status}`);
  const result = await response.json();
  if (!result.source?.includes(`Recovery drill revision ${revision}`)) {
    throw new Error(`revision ${revision} of ${format} was not visible in source`);
  }
}

const port = await freePort();
const base = `http://127.0.0.1:${port}`;
const configPath = join(dataDirectory, "config.toml");
writeFileSync(configPath, [
  "[server]", `address = "127.0.0.1:${port}"`, "local_companion = false", "",
  "[storage]", `directory = ${JSON.stringify(resolve(dataDirectory))}`, 'database_url = { env = "LIBREPAPER_SOURCE_URL" }', "fsync = false", 'object_store = "filesystem"', "",
  "[auth.github]", 'client_id = { env = "LIBREPAPER_GITHUB_CLIENT_ID" }', 'client_secret = { env = "LIBREPAPER_GITHUB_CLIENT_SECRET" }', "",
  "[access]", 'publishers = ["any"]', 'commenters = ["anyone"]', "",
].join("\n"));
let log = "";
const server = spawn(binary, [
  "admin", "serve", "--config", configPath,
], {
  stdio: ["ignore", "pipe", "pipe"],
  env: { ...process.env, LIBREPAPER_SOURCE_URL: databaseUrl, LIBREPAPER_GITHUB_CLIENT_ID: "fixture", LIBREPAPER_GITHUB_CLIENT_SECRET: "fixture" },
});
server.stdout.on("data", (bytes) => { log += bytes; });
server.stderr.on("data", (bytes) => { log += bytes; });
const closed = new Promise((done) => server.once("close", done));

async function stop() {
  if (server.exitCode !== null || server.signalCode !== null) return;
  server.kill("SIGTERM");
  const timer = setTimeout(() => server.kill("SIGKILL"), 5000);
  await closed;
  clearTimeout(timer);
}

try {
  await until("the deployment to answer", async () => {
    if (server.exitCode !== null) {
      throw Object.assign(new Error("the deployment exited during startup"), { fatal: true });
    }
    return (await fetch(`${base}/api/config`, { signal: AbortSignal.timeout(2000) })).ok;
  });

  const accountId = randomUUID();
  execFileSync("psql", [databaseUrl, "-XAt", "-v", "ON_ERROR_STOP=1", "-c",
    `INSERT INTO accounts
       (id, kind, provider, provider_subject, handle, display_name, status, session_generation)
     VALUES (${sql(accountId)}, 'registered', 'github', 'github:fixture', 'fixture', 'Fixture', 'active', 1)`,
  ], { stdio: ["ignore", "ignore", "inherit"] });
  const cookie = sessionCookie({ dataDirectory, accountId, handle: "fixture", name: "Fixture" });

  for (const [format, name] of Object.entries(TUTORIALS)) {
    const { slug } = await publishProject(format, name, cookie);
    await trimToSnapshot(slug, cookie);
    for (const revision of [1, 2]) {
      await publishProject(format, name, cookie, { slug, revision });
      await assertRevision(slug, revision, format, cookie);
    }
  }
  console.log(`published, snapshotted, and recorded two source revisions for ${Object.keys(TUTORIALS).length} tutorial projects`);
} catch (error) {
  console.error(`fixture.mjs: ${error.message}\n${log}`);
  process.exitCode = 1;
} finally {
  await stop();
}
