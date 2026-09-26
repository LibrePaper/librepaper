#!/usr/bin/env node

// Populate a fresh frugal_fixture database with starter documents and simulated
// activity. Starts the server, then populates the database using admin seed,
// then stops the server cleanly. The server is started so the deployment is
// ready for use after this script completes.

import { spawn, execFileSync } from "node:child_process";
import { createServer } from "node:net";

function logError(message) {
  console.error(`fixture.mjs: ${message}`);
  process.exit(1);
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

async function main() {
  const binary = process.argv[2];
  const databaseUrl = process.argv[3];
  const dataDirectory = process.argv[4];
  const simulateActivityDays = process.argv[5] ? parseInt(process.argv[5], 10) : 0;

  if (!binary || !databaseUrl || !dataDirectory) {
    logError("usage: fixture.mjs <binary> <database-url> <data-directory> [simulate-activity-days]");
  }

  // Start the server with --simulate-activity so it's ready for later use
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

  if (simulateActivityDays > 0) {
    args.push("--simulate-activity", String(simulateActivityDays));
  }

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
    logError(error.message);
  }

  try {
    // Populate the database with starter documents and simulated activity
    // using the seed command, then the server is ready to serve them.
    const seedArgs = [
      "seed",
      "--database-url", databaseUrl,
      "--data-directory", dataDirectory,
    ];

    if (simulateActivityDays > 0) {
      seedArgs.push("--simulate-activity", String(simulateActivityDays));
    }

    execFileSync(binary, seedArgs, {
      stdio: "inherit",
      env: process.env,
    });

    console.log("Documents populated successfully");
  } catch (error) {
    await halt();
    logError(error.message);
  }

  await halt();
}

main().catch((error) => {
  logError(error.message);
});
