import { execFileSync } from "node:child_process";
import { randomBytes, randomUUID } from "node:crypto";

function sqlLiteral(value) {
  return `'${String(value).replaceAll("'", "''")}'`;
}

/** Create an isolated PostgreSQL database for an application integration test. */
export function postgresTestDatabase(label) {
  const administrativeUrl = process.env.LIBREPAPER_TEST_POSTGRES_URL || process.env.LIBREPAPER_DATABASE_URL;
  if (!administrativeUrl) {
    throw new Error("set LIBREPAPER_TEST_POSTGRES_URL to run the application browser integration test");
  }
  const database = `librepaper_${label.replaceAll(/[^a-z0-9]/gi, "_").toLowerCase()}_${randomBytes(6).toString("hex")}`;
  execFileSync("psql", [administrativeUrl, "-v", "ON_ERROR_STOP=1", "-c", `CREATE DATABASE ${database}`], { stdio: "ignore" });
  const url = new URL(administrativeUrl);
  url.pathname = `/${database}`;
  return {
    url: url.toString(),
    seedRegisteredAccount({ provider, subject, handle, displayName }) {
      execFileSync("psql", [url.toString(), "-v", "ON_ERROR_STOP=1", "-c", `INSERT INTO accounts
        (id, kind, provider, provider_subject, handle, display_name, status, session_generation)
        VALUES (${sqlLiteral(randomUUID())}, 'registered', ${sqlLiteral(provider)}, ${sqlLiteral(subject)},
          ${sqlLiteral(handle)}, ${sqlLiteral(displayName)}, 'active', 1)`], { stdio: "ignore" });
    },
    drop() {
      execFileSync("psql", [administrativeUrl, "-v", "ON_ERROR_STOP=1", "-c",
        `DROP DATABASE IF EXISTS ${database} WITH (FORCE)`], { stdio: "ignore" });
    },
  };
}
