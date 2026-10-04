// A real `librepaper admin serve`, on a throwaway PostgreSQL database, with a
// real signed-in account. For integration tests that need the whole server --
// the HTTP routes, the websocket transport, the log and its durable writes --
// rather than one module of it.
//
// Nothing here touches a deployment or any storage but the database it
// creates and the temporary directory it is given.

import { spawn } from "node:child_process";
import { createHmac } from "node:crypto";
import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { createServer } from "node:net";
import { MissingPostgresConfigurationError, postgresTestDatabase } from "../../tools/postgres-test.mjs";

const repository = resolve(import.meta.dirname, "../../..");

export function deploymentBinary() {
  return resolve(process.env.LIBREPAPER_TEST_BINARY || join(repository, "target/debug/librepaper"));
}

/// The same session envelope the OAuth callback writes: the deployment's own
/// session key, the account row's generation, and the v2 purpose-separated
/// MAC. The real binary verifies all three before it treats the caller as
/// signed in, so this is a genuine sign-in and not a bypass of one.
export function sessionCookie({ dataDirectory, accountId, handle, name, generation = "1", seconds = 3600 }) {
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

/// Starts the deployment and signs one account in. `binary` defaults to the
/// debug build; the browser smoke test passes the one it was handed.
///
/// `advanced` contains TOML settings for a test-specific guardrail change.
export async function startDeployment({ label, binary = deploymentBinary(), advanced = null, handle = "tester", name = "Tester" }) {
  if (!existsSync(binary)) return null;
  let postgres;
  try {
    postgres = postgresTestDatabase(label);
  } catch (error) {
    if (error instanceof MissingPostgresConfigurationError) return { unavailable: error.message };
    throw error;
  }
  let data;
  try {
    data = await mkdtemp(join(tmpdir(), `librepaper-${label}-`));
  } catch (error) {
    try { postgres.drop(); } catch {}
    throw error;
  }
  // Asked for rather than guessed, so two of these can run at once and a
  // rerun never lands on a port something else has just taken.
  const listener = createServer();
  let port;
  try {
    await new Promise((done, fail) => {
      listener.once("error", fail);
      listener.listen(0, "127.0.0.1", done);
    });
    port = listener.address().port;
    await new Promise((done, fail) => listener.close((error) => error ? fail(error) : done()));
  } catch (error) {
    try { listener.close(); } catch {}
    try { postgres.drop(); } catch {}
    await rm(data, { recursive: true, force: true });
    throw error;
  }
  const base = `http://127.0.0.1:${port}`;
  // Publishing needs a provider, but these tests create their own signed
  // session and never send a request to GitHub.
  const configPath = join(data, "config.toml");
  const config = [
    "[server]", `bind = "127.0.0.1"`, `port = ${port}`, "local_companion = false", "",
    "[storage]", `directory = ${JSON.stringify(data)}`, 'database_url = { env = "LIBREPAPER_DATABASE_URL" }', "fsync = false", "object_store = \"filesystem\"", "",
    "[auth.github]", 'client_id = { env = "LIBREPAPER_GITHUB_CLIENT_ID" }', 'client_secret = { env = "LIBREPAPER_GITHUB_CLIENT_SECRET" }', "",
    "[access]", 'publishers = ["any"]', 'commenters = ["anyone"]', "",
    advanced || "",
  ].join("\n");
  try {
    writeFileSync(configPath, config);
  } catch (error) {
    try { postgres.drop(); } catch {}
    await rm(data, { recursive: true, force: true });
    throw error;
  }
  const args = ["admin", "serve", "--config", configPath];
  let log = "";
  let server = null;
  let launchError = null;
  let serverClosed;

  function launch() {
    launchError = null;
    server = spawn(binary, args, {
      stdio: ["ignore", "pipe", "pipe"],
      env: { ...process.env, LIBREPAPER_DATABASE_URL: postgres.url, LIBREPAPER_GITHUB_CLIENT_ID: "integration-test", LIBREPAPER_GITHUB_CLIENT_SECRET: "integration-test" },
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

  const stop = async () => {
    await halt();
    try { postgres.drop(); } catch {}
    await rm(data, { recursive: true, force: true });
  };

  launch();
  try {
    await ready();
  } catch (error) {
    await stop();
    throw error;
  }

  let accountId;
  let cookie;
  try {
    accountId = postgres.seedRegisteredAccount({
      provider: "github",
      subject: `github:${handle}`,
      handle,
      displayName: name,
    });
    cookie = sessionCookie({ dataDirectory: data, accountId, handle, name });
  } catch (error) {
    await stop();
    throw error;
  }

  return {
    base,
    cookie,
    data,
    accountId,
    /// The throwaway database itself, for a test that has to reach it
    /// directly rather than through the deployment -- holding a table lock to
    /// stall the write path, for instance.
    postgres,
    get log() { return log; },
    stop,
    /// Stop the process and start it again on the same database, object
    /// store and port. What survives this is what PostgreSQL holds, not what
    /// the previous process happened to have in memory.
    async restart() {
      await halt();
      launch();
      await ready();
    },
    async publish(body) {
      const response = await fetch(`${base}/api/documents`, {
        method: "POST",
        headers: { "content-type": "application/json", "x-librepaper-client": "1", cookie },
        body: JSON.stringify(body),
      });
      const payload = await response.json();
      if (response.status !== 201) throw new Error(`publish failed: ${response.status} ${JSON.stringify(payload)}`);
      return payload;
    },
  };
}

export { until };
