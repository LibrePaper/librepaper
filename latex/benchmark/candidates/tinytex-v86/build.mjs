import { execFileSync } from 'node:child_process';
import { mkdirSync, rmSync, writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
const root = fileURLToPath(new URL('.', import.meta.url));
const image = 'librepaper-tinytex-v86:experiment';
const container = 'librepaper-tinytex-v86-export-' + process.pid;
const run = (cmd, args) => execFileSync(cmd, args, { cwd: root, stdio: 'inherit' });
if (!process.argv.includes('--export-only')) run('docker', ['build', '--platform', 'linux/386', '--progress', 'plain', '-t', image, '.']);
const info = JSON.parse(execFileSync('docker', ['image', 'inspect', image], { encoding: 'utf8' }))[0];
writeFileSync(root + 'assets/image-receipt.json', JSON.stringify({ id: info.Id, architecture: info.Architecture, created: info.Created, size: info.Size }, null, 2) + '\n');
run('docker', ['create', '--name', container, '--platform', 'linux/386', image, '/bin/true']);
try { run('docker', ['export', container, '-o', root + 'assets/rootfs.tar']); }
finally { run('docker', ['rm', container]); }
rmSync(root + 'assets/rootfs', { recursive: true, force: true });
mkdirSync(root + 'assets/rootfs', { recursive: true });
run('tar', ['--extract', '--file', root + 'assets/rootfs.tar', '--directory', root + 'assets/rootfs', '--no-same-owner', '--exclude=dev/*']);
run(process.execPath, [root + 'pack.mjs']);
