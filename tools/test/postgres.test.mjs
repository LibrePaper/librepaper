import assert from "node:assert/strict";
import { chmod, mkdtemp, mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { spawnSync } from "node:child_process";
import test from "node:test";
import { fileURLToPath } from "node:url";

const helper = fileURLToPath(new URL("./postgres.sh", import.meta.url));

test("disposable PostgreSQL rejects non-loopback mappings and removes the container", async () => {
  const root = await mkdtemp(join(tmpdir(), "librepaper-postgres-helper-"));
  const bin = join(root, "bin");
  const log = join(root, "docker.log");
  await mkdir(bin);
  const docker = join(bin, "docker");
  await writeFile(docker, `#!${process.execPath}
const fs = require('node:fs');
fs.appendFileSync(process.env.DOCKER_LOG, process.argv.slice(2).join(' ') + String.fromCharCode(10));
if (process.argv[2] === 'port') process.stdout.write('0.0.0.0:45432' + String.fromCharCode(10));
`);
  await chmod(docker, 0o755);
  try {
    const script = `source "$1"; postgres_start_disposable fake-pg test`;
    const result = spawnSync("bash", ["-c", script, "postgres-test", helper], {
      encoding: "utf8",
      env: { ...process.env, PATH: `${bin}:${process.env.PATH}`, DOCKER_LOG: log },
    });
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /did not publish PostgreSQL on a loopback port/);
    assert.match(await readFile(log, "utf8"), /rm -f fake-pg/);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});
