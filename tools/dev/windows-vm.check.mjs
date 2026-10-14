import assert from 'node:assert/strict';
import { chmod, mkdtemp, mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { spawnSync } from 'node:child_process';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const repo = fileURLToPath(new URL('../..', import.meta.url));
const windowsVm = join(repo, 'tools/dev/windows-vm');

// A fake tool that appends its argv as one JSON line to the file named by env var LOG_VAR.
async function fakeTool(bin, name, logVar) {
  const path = join(bin, name);
  await writeFile(path, `#!${process.execPath}
require('node:fs').appendFileSync(process.env.${logVar}, JSON.stringify(process.argv.slice(2)) + '\\n');
`);
  await chmod(path, 0o755);
}

async function setup() {
  const root = await mkdtemp(join(tmpdir(), 'librepaper-windows-vm-'));
  const bin = join(root, 'bin');
  const vm = join(root, 'vm');
  await mkdir(bin);
  await mkdir(vm);
  await fakeTool(bin, 'quickemu', 'FAKE_LOG');
  await fakeTool(bin, 'quickget', 'FAKE_QUICKGET_LOG');
  return {
    root,
    bin,
    vm,
    quickemuLog: join(root, 'quickemu.log'),
    quickgetLog: join(root, 'quickget.log'),
  };
}

function run(env, args) {
  return spawnSync(windowsVm, args, {
    cwd: repo,
    encoding: 'utf8',
    env: {
      ...process.env,
      PATH: `${env.bin}:${process.env.PATH}`,
      WINDOWS_VM_DIR: env.vm,
      FAKE_LOG: env.quickemuLog,
      FAKE_QUICKGET_LOG: env.quickgetLog,
      ...env.extra,
    },
  });
}

async function readCalls(logPath) {
  const text = await readFile(logPath, 'utf8');
  return text
    .split('\n')
    .filter((line) => line !== '')
    .map((line) => JSON.parse(line));
}

test('help lists the commands and exits 0', async () => {
  const env = await setup();
  try {
    const result = run(env, ['help']);
    assert.equal(result.status, 0, result.stderr);
    for (const word of ['create', 'start', 'snapshot', 'restore', 'build', 'reset']) {
      assert.match(result.stdout, new RegExp(word));
    }
  } finally {
    await rm(env.root, { recursive: true, force: true });
  }
});

test('an unknown command exits non zero', async () => {
  const env = await setup();
  try {
    const result = run(env, ['bogus']);
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /unknown command/);
  } finally {
    await rm(env.root, { recursive: true, force: true });
  }
});

test('invalid snapshot names are rejected before any tool runs', async () => {
  const env = await setup();
  try {
    for (const args of [['snapshot', 'bad name'], ['restore', '../x']]) {
      const result = run(env, args);
      assert.notEqual(result.status, 0, `${args.join(' ')} unexpectedly passed`);
      assert.match(result.stderr, /snapshot name must match/);
      await assert.rejects(readFile(env.quickemuLog), { code: 'ENOENT' });
    }
  } finally {
    await rm(env.root, { recursive: true, force: true });
  }
});

test('reset without --yes exits 1 and calls nothing', async () => {
  const env = await setup();
  try {
    await writeFile(join(env.vm, 'windows-11.conf'), 'ram="4G"\n');
    const result = run(env, ['reset']);
    assert.equal(result.status, 1);
    await assert.rejects(readFile(env.quickemuLog), { code: 'ENOENT' });
  } finally {
    await rm(env.root, { recursive: true, force: true });
  }
});

test('create with an existing conf reuses the download and sets the keys idempotently', async () => {
  const env = await setup();
  try {
    await writeFile(
      join(env.vm, 'windows-11.conf'),
      'ram="4G"\ncpu_cores=2\ndisk_img="windows-11/disk.qcow2"\n',
    );
    // A cached virtio ISO keeps create off the network; the Windows ISO stays missing.
    await mkdir(join(env.vm, 'windows-11'));
    const iso = Buffer.alloc(32774);
    iso.write('CD001', 32769);
    await writeFile(join(env.vm, 'windows-11', 'virtio-win.iso'), iso);
    const extra = { WINDOWS_VM_RAM: '6G', WINDOWS_VM_CPUS: '3' };

    const first = run({ ...env, extra }, ['create']);
    assert.equal(first.status, 0, first.stderr);
    const afterFirst = await readFile(join(env.vm, 'windows-11.conf'), 'utf8');

    const second = run({ ...env, extra }, ['create']);
    assert.equal(second.status, 0, second.stderr);
    const conf = await readFile(join(env.vm, 'windows-11.conf'), 'utf8');
    assert.equal(conf, afterFirst);

    const lines = conf.split('\n');
    assert.deepEqual(lines.filter((line) => line.startsWith('ram=')), ['ram="6G"']);
    assert.deepEqual(lines.filter((line) => line.startsWith('cpu_cores=')), ['cpu_cores="3"']);
    assert.deepEqual(lines.filter((line) => line.startsWith('public_dir=')), [
      `public_dir="${join(env.vm, 'share')}"`,
    ]);
    assert.ok(lines.includes('disk_img="windows-11/disk.qcow2"'));
    await assert.rejects(readFile(env.quickgetLog), { code: 'ENOENT' });
  } finally {
    await rm(env.root, { recursive: true, force: true });
  }
});

test('snapshot clean calls quickemu with the snapshot arguments', async () => {
  const env = await setup();
  try {
    await writeFile(join(env.vm, 'windows-11.conf'), 'ram="4G"\n');
    const result = run(env, ['snapshot', 'clean']);
    assert.equal(result.status, 0, result.stderr);
    assert.deepEqual(await readCalls(env.quickemuLog), [
      ['--vm', 'windows-11.conf', '--snapshot', 'create', 'clean'],
    ]);
  } finally {
    await rm(env.root, { recursive: true, force: true });
  }
});

test('.gitignore ignores the VM directory', async () => {
  const gitignore = await readFile(join(repo, '.gitignore'), 'utf8');
  assert.ok(gitignore.split(/\r?\n/).includes('/.windows-vm/'));
});
