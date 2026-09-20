import { execFileSync } from "node:child_process";
import { randomBytes, randomUUID } from "node:crypto";

// How `psql` is reached. A machine with the client on PATH needs nothing;
// one whose PostgreSQL is a container has no client binary at all, so
// LIBREPAPER_TEST_PSQL names the command that does have one (for example
// "docker exec -i lp-pg psql") and LIBREPAPER_TEST_PSQL_URL the URL that
// command should connect with, which is not the URL the application uses
// when the daemon is only published to the host on a different port.
function psqlCommand() {
  const override = (process.env.LIBREPAPER_TEST_PSQL || "").trim();
  const argv = override ? override.split(/\s+/) : ["psql"];
  return { program: argv[0], prefix: argv.slice(1) };
}

function run(url, sql) {
  const { program, prefix } = psqlCommand();
  execFileSync(program, [...prefix, url, "-v", "ON_ERROR_STOP=1", "-c", sql], { stdio: "ignore" });
}

function sqlLiteral(value) {
  return `'${String(value).replaceAll("'", "''")}'`;
}

export class MissingPostgresConfigurationError extends Error {
  constructor() {
    super("set LIBREPAPER_TEST_POSTGRES_URL to run an integration test against a real deployment");
    this.name = "MissingPostgresConfigurationError";
  }
}

/** Create an isolated PostgreSQL database for an application integration test. */
export function postgresTestDatabase(label) {
  const administrativeUrl = process.env.LIBREPAPER_TEST_POSTGRES_URL || process.env.LIBREPAPER_DATABASE_URL;
  if (!administrativeUrl?.trim()) throw new MissingPostgresConfigurationError();
  const database = `librepaper_${label.replaceAll(/[^a-z0-9]/gi, "_").toLowerCase()}_${randomBytes(6).toString("hex")}`;
  // What psql connects with, and what the application connects with, are the
  // same URL unless the daemon runs somewhere the client binary does not.
  const clientAdministrativeUrl = process.env.LIBREPAPER_TEST_PSQL_URL || administrativeUrl;
  const url = new URL(administrativeUrl);
  const clientUrl = new URL(clientAdministrativeUrl);
  run(clientAdministrativeUrl, `CREATE DATABASE ${database}`);
  url.pathname = `/${database}`;
  clientUrl.pathname = `/${database}`;
  return {
    url: url.toString(),
    seedRegisteredAccount({ provider, subject, handle, displayName }) {
      const id = randomUUID();
      run(clientUrl.toString(), `INSERT INTO accounts
        (id, kind, provider, provider_subject, handle, display_name, status, session_generation)
        VALUES (${sqlLiteral(id)}, 'registered', ${sqlLiteral(provider)}, ${sqlLiteral(subject)},
          ${sqlLiteral(handle)}, ${sqlLiteral(displayName)}, 'active', 1)`);
      return id;
    },
    drop() {
      run(clientAdministrativeUrl, `DROP DATABASE IF EXISTS ${database} WITH (FORCE)`);
    },
  };
}
