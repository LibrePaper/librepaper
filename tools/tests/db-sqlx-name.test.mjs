import assert from 'node:assert/strict';
import { chmod, mkdtemp, mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { spawnSync } from 'node:child_process';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const repo = fileURLToPath(new URL('../..', import.meta.url));
const db = join(repo, 'tools/db');

test('SQLX database rejects invalid names and the persistent dev database before Docker', async () => {
  const root = await mkdtemp(join(tmpdir(), 'librepaper-db-name-'));
  const bin = join(root, 'bin');
  const marker = join(root, 'docker-called');
  await mkdir(bin);
  const docker = join(bin, 'docker');
  await writeFile(docker, `#!${process.execPath}\nrequire('node:fs').writeFileSync(process.env.DOCKER_MARKER, 'called');\n`);
  await chmod(docker, 0o755);

  try {
    for (const [database, message] of [
      ['librepaper', /must not match the persistent development database/],
      ['bad; DROP DATABASE librepaper', /must be a PostgreSQL identifier/],
      ['name-with-dash', /must be a PostgreSQL identifier/],
      ['a'.repeat(64), /must be a PostgreSQL identifier/],
    ]) {
      const result = spawnSync(db, ['sqlx-prepare'], {
        cwd: repo,
        encoding: 'utf8',
        env: { ...process.env, PATH: `${bin}:${process.env.PATH}`, DOCKER_MARKER: marker, SQLX_POSTGRES_DB: database },
      });
      assert.notEqual(result.status, 0, `${database} unexpectedly passed validation`);
      assert.match(result.stderr, message);
      await assert.rejects(readFile(marker), { code: 'ENOENT' });
    }
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});
