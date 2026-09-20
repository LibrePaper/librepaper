import assert from "node:assert/strict";
import test from "node:test";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { startDeployment } from "../helpers/deployment.mjs";

const databaseVariables = [
  "LIBREPAPER_TEST_POSTGRES_URL",
  "LIBREPAPER_DATABASE_URL",
  "LIBREPAPER_TEST_PSQL",
  "LIBREPAPER_TEST_PSQL_URL",
  "LIBREPAPER_TEST_BINARY",
];

function withEnvironment(values, callback) {
  const previous = Object.fromEntries(databaseVariables.map((name) => [name, process.env[name]]));
  for (const name of databaseVariables) {
    if (name in values) process.env[name] = values[name];
    else delete process.env[name];
  }
  return Promise.resolve(callback()).finally(() => {
    for (const name of databaseVariables) {
      if (previous[name] === undefined) delete process.env[name];
      else process.env[name] = previous[name];
    }
  });
}

test("deployment helper reports unavailable only when PostgreSQL is not configured", async () => {
  await withEnvironment({ LIBREPAPER_TEST_BINARY: process.execPath }, async () => {
    const deployment = await startDeployment({ label: "missing_database_configuration" });
    assert.match(deployment.unavailable, /LIBREPAPER_TEST_POSTGRES_URL/);
  });
});

test("deployment helper propagates an explicitly configured psql failure", async () => {
  await withEnvironment({
    LIBREPAPER_TEST_BINARY: process.execPath,
    LIBREPAPER_TEST_POSTGRES_URL: "postgresql://postgres@example.invalid/librepaper",
    LIBREPAPER_TEST_PSQL: "librepaper-test-command-that-does-not-exist",
  }, async () => {
    await assert.rejects(
      startDeployment({ label: "invalid_psql_configuration" }),
      /ENOENT|librepaper-test-command-that-does-not-exist/,
    );
  });
});

test("a failed server launch rejects and drops its temporary database", { timeout: 10000 }, async () => {
  const directory = await mkdtemp(join(tmpdir(), "librepaper-launch-failure-"));
  try {
    const binary = join(directory, "not-executable");
    const client = join(directory, "psql.mjs");
    const queries = join(directory, "queries.jsonl");
    await writeFile(binary, "not an executable", { mode: 0o600 });
    await writeFile(client, `import { appendFileSync } from "node:fs";
appendFileSync(${JSON.stringify(queries)}, JSON.stringify(process.argv.at(-1)) + "\\n");
`);
    await withEnvironment({
      LIBREPAPER_TEST_BINARY: binary,
      LIBREPAPER_TEST_POSTGRES_URL: "postgresql://postgres@example.invalid/librepaper",
      LIBREPAPER_TEST_PSQL: `${process.execPath} ${client}`,
    }, async () => {
      await assert.rejects(startDeployment({ label: "failed_launch" }), { code: "EACCES" });
    });
    const statements = (await readFile(queries, "utf8")).trim().split("\n").map(JSON.parse);
    assert.equal(statements.length, 2);
    const database = statements[0].match(/^CREATE DATABASE (\w+)$/)?.[1];
    assert.ok(database);
    assert.equal(statements[1], `DROP DATABASE IF EXISTS ${database} WITH (FORCE)`);
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});
