#!/usr/bin/env node
// Populate the drill's source deployment through the real server: start
// `admin serve` on the given database and data directory, sign one account in,
// publish the five tutorial projects, then stop. Every document the drill backs
// up is written the way a person's would be.
//
// usage: fixture.mjs <binary> <database-url> <data-directory>

import { execFileSync, spawn } from "node:child_process";
import { randomUUID } from "node:crypto";
import { readdirSync, readFileSync } from "node:fs";
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

const port = await freePort();
const base = `http://127.0.0.1:${port}`;
let log = "";
const server = spawn(binary, [
  "admin", "serve",
  "--port", String(port),
  "--data-directory", dataDirectory,
  "--database-url", databaseUrl,
  "--fsync", "false",
  "--publishers", "any",
  // Publishing needs a configured provider; nothing here reaches GitHub, the
  // session cookie below is this deployment's own credential.
  "--github-client-id", "fixture",
  "--no-local",
], {
  stdio: ["ignore", "pipe", "pipe"],
  env: { ...process.env, LIBREPAPER_GITHUB_CLIENT_SECRET: "fixture" },
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
    const root = join(repository, "docs/examples", `tutorial-${format}`);
    const paths = filesUnder(root);
    const main = paths.find((path) => /^librepaper\.[a-z]+$/.test(path) && !path.endsWith(".png"));
    const form = new FormData();
    form.append("main", main);
    form.append("title", `Learn LibrePaper with ${name}`);
    for (const path of paths) form.append("file", new Blob([readFileSync(join(root, path))]), path);
    const response = await fetch(`${base}/api/documents`, {
      method: "POST",
      headers: { "x-librepaper-client": "1", cookie },
      body: form,
    });
    if (response.status !== 201) {
      throw new Error(`publishing ${format} failed: ${response.status} ${await response.text()}`);
    }
  }
  console.log(`published ${Object.keys(TUTORIALS).length} tutorial projects`);
} catch (error) {
  console.error(`fixture.mjs: ${error.message}\n${log}`);
  process.exitCode = 1;
} finally {
  await stop();
}
