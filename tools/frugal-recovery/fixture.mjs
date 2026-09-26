#!/usr/bin/env node

// Populate a fresh fixture database by starting the server and publishing
// the five tutorial documents through the HTTP API. This replaces the admin
// seed command which is being removed.

import { spawn, execFileSync } from "node:child_process";
import { createHmac, randomUUID } from "node:crypto";
import { readdirSync, readFileSync, statSync } from "node:fs";
import { createServer } from "node:net";
import { join, resolve } from "node:path";
import { fileURLToPath } from "url";

const __dirname = fileURLToPath(new URL(".", import.meta.url));
const repository = resolve(__dirname, "../../..");

function logError(message) {
  console.error(`fixture.mjs: ${message}`);
  process.exit(1);
}

function psqlCommand() {
  const override = (process.env.LIBREPAPER_TEST_PSQL || "").trim();
  const argv = override ? override.split(/\s+/) : ["psql"];
  return { program: argv[0], prefix: argv.slice(1) };
}

function sqlLiteral(value) {
  if (value === null || value === undefined) return "null";
  return `'${String(value).replaceAll("'", "''")}'`;
}

function sessionCookie({ dataDirectory, accountId, handle, name, generation = "1", seconds = 3600 }) {
  const key = Buffer.from(readFileSync(join(dataDirectory, "secrets", "session.key"), "utf8").trim(), "hex");
  const expires = Math.floor(Date.now() / 1000) + seconds;
  const payload = Buffer.from(`github|${handle}|${accountId}|${generation}||${name}|${expires}`).toString("base64url");
  const signature = createHmac("sha256", key).update(`session-v2\0${payload}`).digest("base64url");
  return `librepaper_session=v2.${payload}.${signature}`;
}

async function until(what, predicate, timeout = 30000) {
  const deadline = Date.now() + timeout;
  let last = "";
  while (Date.now() < deadline) {
    try {
      const value = await predicate();
      if (value) return value;
    } catch (error) {
      if (error?.fatal) throw error;
      last = String(error);
    }
    await new Promise((done) => setTimeout(done, 100));
  }
  throw new Error(`timed out waiting for ${what}${last ? `: ${last}` : ""}`);
}

async function findFreePort() {
  const listener = createServer();
  return new Promise((resolve, reject) => {
    listener.once("error", reject);
    listener.listen(0, "127.0.0.1", () => {
      const port = listener.address().port;
      listener.close(() => resolve(port));
    });
  });
}

function listTutorialFiles(tutorialDir) {
  const files = {};
  const entries = readdirSync(tutorialDir, { withFileTypes: true });
  for (const entry of entries) {
    if (entry.isFile()) {
      const path = join(tutorialDir, entry.name);
      files[entry.name] = readFileSync(path);
    }
  }
  return files;
}

async function publishTutorial(base, cookie, tutorialDir, title) {
  const form = new FormData();
  const files = listTutorialFiles(tutorialDir);
  for (const [name, content] of Object.entries(files)) {
    // Files are appended with their path as the filename
    form.append("file", new Blob([content]), name);
  }
  form.append("title", title);

  const response = await fetch(`${base}/api/documents`, {
    method: "POST",
    headers: {
      "x-librepaper-client": "1",
      "cookie": cookie,
    },
    body: form,
  });

  if (response.status !== 201) {
    const payload = await response.json().catch(() => ({}));
    throw new Error(`publish failed: ${response.status} ${JSON.stringify(payload)}`);
  }

  const result = await response.json();
  return result;
}

async function main() {
  const binary = process.argv[2];
  const databaseUrl = process.argv[3];
  const dataDirectory = process.argv[4];

  if (!binary || !databaseUrl || !dataDirectory) {
    logError("usage: fixture.mjs <binary> <database-url> <data-directory>");
  }

  const port = await findFreePort();
  const base = `http://127.0.0.1:${port}`;

  const args = [
    "admin", "serve",
    "--port", String(port),
    "--data-directory", dataDirectory,
    "--fsync", "false",
    "--publishers", "any",
    "--commenters", "anyone",
    "--github-client-id", "fixture-provision",
    "--database-url", databaseUrl,
    "--no-local",
  ];

  let log = "";
  let server = null;
  let launchError = null;
  let serverClosed;

  function launch() {
    launchError = null;
    server = spawn(binary, args, {
      stdio: ["ignore", "pipe", "pipe"],
      env: { ...process.env, LIBREPAPER_GITHUB_CLIENT_SECRET: "fixture-provision" },
    });
    server.once("error", (error) => { launchError = error; });
    serverClosed = new Promise((done) => server.once("close", done));
    server.stdout.on("data", (bytes) => { log += bytes; });
    server.stderr.on("data", (bytes) => { log += bytes; });
  }

  async function halt() {
    if (!server) return;
    const dying = server;
    server = null;
    if (launchError || dying.exitCode !== null || dying.signalCode !== null) {
      await serverClosed;
      return;
    }
    const waitForClose = async (milliseconds) => {
      let timer;
      try {
        await Promise.race([
          serverClosed,
          new Promise((done) => { timer = setTimeout(done, milliseconds); }),
        ]);
      } finally {
        clearTimeout(timer);
      }
    };
    dying.kill("SIGTERM");
    await waitForClose(5000);
    if (dying.exitCode === null && dying.signalCode === null) {
      dying.kill("SIGKILL");
      await waitForClose(1000);
    }
  }

  async function ready() {
    await until("the deployment to answer", async () => {
      if (launchError) {
        launchError.fatal = true;
        throw launchError;
      }
      if (server?.exitCode != null || server?.signalCode != null) {
        throw Object.assign(new Error(log || "deployment exited during startup"), { fatal: true });
      }
      return (await fetch(`${base}/api/config`, { signal: AbortSignal.timeout(2000) })).ok;
    });
  }

  launch();
  try {
    await ready();
  } catch (error) {
    await halt();
    if (log) console.error("Server log:\n" + log);
    logError(error.message);
  }

  try {
    // Create account directly in database
    const accountId = randomUUID();
    const handle = "fixture";
    const displayName = "Fixture";

    const { program, prefix } = psqlCommand();
    execFileSync(program, [
      ...prefix,
      databaseUrl,
      "-XAt",
      "-v", "ON_ERROR_STOP=1",
      "-c",
      `INSERT INTO accounts (id, kind, provider, provider_subject, handle, display_name, status, session_generation) VALUES (${sqlLiteral(accountId)}, 'registered', ${sqlLiteral("github")}, ${sqlLiteral("github:fixture")}, ${sqlLiteral(handle)}, ${sqlLiteral(displayName)}, 'active', 1)`,
    ], { stdio: "ignore" });

    // Create session cookie
    const cookie = sessionCookie({ dataDirectory, accountId, handle, name: displayName });

    // Publish the five tutorial documents
    const tutorials = [
      { dir: join(repository, "docs/examples/tutorial-markdown"), title: "Learn LibrePaper with Markdown" },
      { dir: join(repository, "docs/examples/tutorial-typst"), title: "Learn LibrePaper with Typst" },
      { dir: join(repository, "docs/examples/tutorial-latex"), title: "Learn LibrePaper with LaTeX" },
      { dir: join(repository, "docs/examples/tutorial-html"), title: "Learn LibrePaper with HTML" },
      { dir: join(repository, "docs/examples/tutorial-quarto"), title: "Learn LibrePaper with Quarto" },
    ];

    for (const tutorial of tutorials) {
      await publishTutorial(base, cookie, tutorial.dir, tutorial.title);
    }

    console.log("Tutorial documents published successfully");
  } catch (error) {
    await halt();
    if (log) console.error("Server log:\n" + log);
    logError(error.message);
  }

  await halt();
}

main().catch((error) => {
  logError(error.message);
});
