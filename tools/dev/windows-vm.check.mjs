import assert from 'node:assert/strict';
import { chmod, mkdtemp, mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { spawn, spawnSync } from 'node:child_process';
import { createServer } from 'node:net';
import test from 'node:test';
import { fileURLToPath, pathToFileURL } from 'node:url';

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
    assert.deepEqual(lines.filter((line) => line.startsWith('keyboard=')), ['keyboard="ps2"']);
    assert.deepEqual(lines.filter((line) => line.startsWith('gl=')), ['gl="off"']);
    await assert.rejects(readFile(env.quickgetLog), { code: 'ENOENT' });
  } finally {
    await rm(env.root, { recursive: true, force: true });
  }
});

test('create adds the Setup language to autounattend.xml once and rebuilds the ISO', async () => {
  const env = await setup();
  try {
    await writeFile(join(env.vm, 'windows-11.conf'), 'ram="4G"\n');
    await mkdir(join(env.vm, 'windows-11', 'unattended'), { recursive: true });
    const xmlPath = join(env.vm, 'windows-11', 'unattended', 'autounattend.xml');
    await writeFile(xmlPath, '<unattend>\n  <settings pass="windowsPE">\n  </settings>\n</unattend>\n');
    const iso = Buffer.alloc(32774);
    iso.write('CD001', 32769);
    await writeFile(join(env.vm, 'windows-11', 'virtio-win.iso'), iso);
    await fakeTool(env.bin, 'mkisofs', 'FAKE_MKISOFS_LOG');
    const mkisofsLog = join(env.root, 'mkisofs.log');

    for (let i = 0; i < 2; i++) {
      const result = run({ ...env, extra: { FAKE_MKISOFS_LOG: mkisofsLog } }, ['create']);
      assert.equal(result.status, 0, result.stderr);
    }

    const xml = await readFile(xmlPath, 'utf8');
    assert.equal(xml.split('International-Core-WinPE').length - 1, 1);
    const windowsPe = xml.indexOf('<settings pass="windowsPE">');
    assert.ok(xml.indexOf('International-Core-WinPE') > windowsPe);

    const calls = await readCalls(mkisofsLog);
    assert.equal(calls.length, 1);
    assert.ok(calls[0].includes('-o'));
    assert.ok(calls[0].includes('unattended.iso'));
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

async function loadQga() {
  return import(pathToFileURL(join(repo, 'tools/dev/windows-vm.qga.mjs')).href);
}

// Like run, but async, so a fake agent in this process can answer.
function runAsync(env, args) {
  return new Promise((resolve, reject) => {
    const child = spawn(windowsVm, args, {
      cwd: repo,
      env: {
        ...process.env,
        PATH: `${env.bin}:${process.env.PATH}`,
        WINDOWS_VM_DIR: env.vm,
      },
    });
    let stdout = '';
    let stderr = '';
    child.stdout.setEncoding('utf8');
    child.stderr.setEncoding('utf8');
    child.stdout.on('data', (chunk) => {
      stdout += chunk;
    });
    child.stderr.on('data', (chunk) => {
      stderr += chunk;
    });
    child.on('error', reject);
    child.on('close', (status) => resolve({ status, stdout, stderr }));
  });
}

test('guestScript builds the installer line for each mode', async () => {
  const { guestScript } = await loadQga();
  const latest = guestScript('latest');
  assert.ok(
    latest.includes('https://github.com/LibrePaper/librepaper/releases/latest/download/librepaper-installer.ps1'),
  );
  const tagged = guestScript('v0.0.39');
  assert.ok(tagged.includes('/releases/download/v0.0.39/librepaper-installer.ps1'));
  const local = guestScript('local');
  assert.ok(local.includes(String.raw`\\10.0.2.4\qemu\install-local.ps1`));
  for (const script of [latest, tagged, local]) {
    assert.ok(script.includes('\r\n'));
    assert.ok(script.endsWith('\r\n'));
  }
});

test('bootstrap writes the guest script and starts it as a scheduled task', async () => {
  const env = await setup();
  const sock = join(env.vm, 'windows-11', 'windows-11-agent.sock');
  const requests = [];
  const replies = {
    'guest-sync': (args) => args.id,
    'guest-ping': () => ({}),
    'guest-file-open': () => 1000,
    'guest-file-write': () => ({ count: 1, eof: false }),
    'guest-file-close': () => ({}),
    'guest-exec': () => ({ pid: 7 }),
    'guest-exec-status': () => ({ exited: true, exitcode: 0 }),
  };
  const server = createServer((socket) => {
    socket.setEncoding('utf8');
    let buffer = '';
    socket.on('data', (chunk) => {
      buffer += chunk;
      let end;
      while ((end = buffer.indexOf('\n')) !== -1) {
        const line = buffer.slice(0, end);
        buffer = buffer.slice(end + 1);
        if (line.trim() === '') continue;
        const request = JSON.parse(line);
        requests.push(request);
        const value = replies[request.execute](request.arguments);
        socket.write(`${JSON.stringify({ return: value })}\n`);
      }
    });
  });
  try {
    await mkdir(join(env.vm, 'windows-11'));
    await writeFile(join(env.vm, 'windows-11.conf'), 'ram="4G"\n');
    await new Promise((resolve) => server.listen(sock, resolve));

    const result = await runAsync(env, ['bootstrap', 'v0.0.39']);
    assert.equal(result.status, 0, result.stderr);

    const open = requests.find((request) => request.execute === 'guest-file-open');
    assert.equal(open.arguments.path, 'C:\\Users\\Public\\librepaper-bootstrap.ps1');
    const write = requests.find((request) => request.execute === 'guest-file-write');
    const { guestScript } = await loadQga();
    assert.equal(
      Buffer.from(write.arguments['buf-b64'], 'base64').toString('utf8'),
      guestScript('v0.0.39'),
    );

    const execs = requests.filter((request) => request.execute === 'guest-exec');
    assert.equal(execs.length, 1);
    assert.equal(execs[0].arguments.path, 'powershell.exe');
    const { startTaskCommand } = await loadQga();
    assert.deepEqual(execs[0].arguments.arg.slice(-2), ['-Command', startTaskCommand()]);
    assert.match(startTaskCommand(), /-UserId Quickemu -LogonType Interactive/);
    assert.match(startTaskCommand(), /Start-ScheduledTask -TaskName LibrePaperBootstrap/);
  } finally {
    server.close();
    await rm(env.root, { recursive: true, force: true });
  }
});

test('bootstrap rejects a target that is not latest, local or a tag', async () => {
  const env = await setup();
  try {
    const result = run(env, ['bootstrap', '../x']);
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /bootstrap takes latest, local or a release tag/);
  } finally {
    await rm(env.root, { recursive: true, force: true });
  }
});

test('bootstrap without a guest agent socket asks for start first', async () => {
  const env = await setup();
  try {
    await writeFile(join(env.vm, 'windows-11.conf'), 'ram="4G"\n');
    const result = run(env, ['bootstrap', 'v0.0.39']);
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /run tools\/dev\/windows-vm start first/);
  } finally {
    await rm(env.root, { recursive: true, force: true });
  }
});

test('.gitignore ignores the VM directory', async () => {
  const gitignore = await readFile(join(repo, '.gitignore'), 'utf8');
  assert.ok(gitignore.split(/\r?\n/).includes('/.windows-vm/'));
});
