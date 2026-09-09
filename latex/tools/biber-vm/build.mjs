// Builds the minimal Biber guest image, exports its rootfs, packs it into
// v86's fs.json + objects, copies the pinned v86 runtime files, and writes
// latex/mirror/biber-vm/<vmRelease>/ per docs/specs/wasmtex-interfaces.md
// section 6. Idempotent: re-running with unchanged inputs reproduces the
// same <vmRelease> and does not rewrite unchanged bytes.
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { copyFileSync, existsSync, mkdirSync, readFileSync, readdirSync, rmSync, statSync, writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { pack } from './pack.mjs';

const root = fileURLToPath(new URL('.', import.meta.url));
const mirrorRoot = fileURLToPath(new URL('../../mirror/', import.meta.url));
const image = 'librepaper-biber-vm:build';
const container = 'librepaper-biber-vm-export-' + process.pid;

const run = (cmd, args) => execFileSync(cmd, args, { cwd: root, stdio: 'inherit' });
const sha256hex = (data) => createHash('sha256').update(data).digest('hex');

function dirSize(dir) {
  let bytes = 0, files = 0;
  for (const name of readdirSync(dir, { withFileTypes: true })) {
    const p = dir + '/' + name.name;
    if (name.isDirectory()) { const s = dirSize(p); bytes += s.bytes; files += s.files; }
    else { bytes += statSync(p).size; files++; }
  }
  return { bytes, files };
}

// 1. Prepared assets (v86 runtime + biber binary) must exist. `prepare.mjs`
// is the source of truth for their digests; re-run it if assets/ is empty.
const sources = JSON.parse(readFileSync(root + 'sources.json', 'utf8'));
const required = ['libv86.js', 'v86.wasm', 'seabios.bin', 'vgabios.bin', 'buildroot-bzimage68.bin', 'biber'];
for (const name of required) {
  if (!existsSync(root + 'assets/' + name)) {
    console.log('Missing assets/' + name + ', running prepare.mjs');
    run(process.execPath, [root + 'prepare.mjs']);
    break;
  }
}

// 2. Build the guest image (unless reusing an already-built one).
if (!process.argv.includes('--export-only')) {
  run('docker', ['build', '--platform', 'linux/386', '--progress', 'plain', '-t', image, '.']);
}
const info = JSON.parse(execFileSync('docker', ['image', 'inspect', image], { encoding: 'utf8' }))[0];
mkdirSync(root + 'assets', { recursive: true });
writeFileSync(root + 'assets/image-receipt.json', JSON.stringify({ id: info.Id, architecture: info.Architecture, created: info.Created, size: info.Size }, null, 2) + '\n');

// 3. Export the rootfs.
run('docker', ['create', '--name', container, '--platform', 'linux/386', image, '/bin/true']);
try {
  run('docker', ['export', container, '-o', root + 'assets/rootfs.tar']);
} finally {
  run('docker', ['rm', container]);
}
rmSync(root + 'assets/rootfs', { recursive: true, force: true });
mkdirSync(root + 'assets/rootfs', { recursive: true });
run('tar', ['--extract', '--file', root + 'assets/rootfs.tar', '--directory', root + 'assets/rootfs', '--no-same-owner', '--exclude=dev/*']);

// 4. Pack into a staging fs.json + objects (final location depends on the
// content hash of vm.json, computed below, so pack into a scratch dir first).
const stagingDir = root + 'assets/staging';
rmSync(stagingDir, { recursive: true, force: true });
mkdirSync(stagingDir, { recursive: true });
const packReceipt = pack(root + 'assets/rootfs', stagingDir);
writeFileSync(root + 'assets/filesystem-receipt.json', JSON.stringify(packReceipt, null, 2) + '\n');
console.log('Packed rootfs:', packReceipt);

// 5. Copy the pinned v86 runtime files into staging, verifying digests
// against sources.json / the prepare-receipt.
const prepareReceipt = JSON.parse(readFileSync(root + 'assets/prepare-receipt.json', 'utf8'));
const runtimeFiles = {
  'libv86.js': 'libv86.js',
  'v86.wasm': 'v86.wasm',
  'seabios.bin': 'seabios.bin',
  'vgabios.bin': 'vgabios.bin',
  'bzimage': 'buildroot-bzimage68.bin',
};
const fileEntries = {};
for (const [destName, srcName] of Object.entries(runtimeFiles)) {
  const srcPath = root + 'assets/' + srcName;
  const data = readFileSync(srcPath);
  const got = sha256hex(data);
  const expected = prepareReceipt[srcName]?.sha256;
  if (expected && got !== expected) throw new Error(`Runtime file ${srcName} digest mismatch before packaging`);
  copyFileSync(srcPath, stagingDir + '/' + destName);
  fileEntries[destName] = { url: destName, sha256: got, size: data.length };
}
{
  const fsData = readFileSync(stagingDir + '/fs.json');
  fileEntries['fs.json'] = { url: 'fs.json', sha256: sha256hex(fsData), size: fsData.length };
}

// 6. vm.json, exactly as docs/specs/wasmtex-interfaces.md section 6.
const licences = sources.licences;
const vmJson = {
  runtime: 'v86',
  version: sources.v86_runtime.recipe_pin,
  licence: sources.v86_runtime.licence,
  memory_mb: 256,
  files: fileEntries,
  objects: null, // filled with the final biber-vm/<vmRelease>/objects/ path below
  biber: sources.biber.version,
  perl: 'none (Biber TeX Live binary is self-contained; no Perl interpreter is shipped in the guest)',
  guest: 'debian-bookworm-i386',
  // The kernel is v86's Buildroot one and its shell prompt is `~% `; the
  // packed guest is the 9p tree it sees at /mnt. `setup` runs once at the
  // prompt and prints `ready` or `failed`; `exec` is how every job command
  // enters the guest. The browser worker reads these rather than knowing
  // the guest's layout itself.
  boot: {
    ready: 'LIBREPAPER_VM_READY',
    failed: 'LIBREPAPER_VM_FAILED',
    shell: 'sh',
    setup: "stty -echo; test -x /mnt/usr/local/bin/biber && mount --bind /dev /mnt/dev && mount -t proc proc /mnt/proc && printf '\\nLIBREPAPER_VM_READY\\n' || printf '\\nLIBREPAPER_VM_FAILED\\n'",
    exec: 'chroot /mnt /usr/bin/env PATH=/usr/local/bin:/usr/bin:/bin LC_ALL=C.UTF-8 HOME=/tmp /bin/sh -c',
    cmdline: 'tsc=reliable mitigations=off random.trust_cpu=on',
  },
  recipe: 'latex/tools/biber-vm/',
  sources: [
    { name: 'debian-bookworm-slim base image', ...sources.base_image },
    { name: 'biber', ...sources.biber.download, version: sources.biber.version },
    { name: 'v86 runtime', recipe_pin: sources.v86_runtime.recipe_pin, licence: sources.v86_runtime.licence },
  ],
  licences: {
    v86: licences.v86_runtime,
    debian_base: licences.debian_base_image,
    biber: licences.biber,
    perl: licences.perl,
  },
};

const canonical = JSON.stringify(vmJson);
const vmRelease = sha256hex(canonical).slice(0, 16);
vmJson.objects = `biber-vm/${vmRelease}/objects/`;

const outDir = mirrorRoot + 'biber-vm/' + vmRelease;
mkdirSync(outDir, { recursive: true });
mkdirSync(outDir + '/objects', { recursive: true });

// Copy fs.json, runtime files, and objects into the final immutable
// location. Idempotent: existing identical files are left untouched.
function copyIfMissingOrDifferent(src, dst) {
  if (existsSync(dst)) {
    const a = readFileSync(src);
    const b = readFileSync(dst);
    if (a.length === b.length && sha256hex(a) === sha256hex(b)) return false;
  }
  copyFileSync(src, dst);
  return true;
}

let copied = 0;
for (const destName of Object.keys(runtimeFiles)) {
  if (copyIfMissingOrDifferent(stagingDir + '/' + destName, outDir + '/' + destName)) copied++;
}
copyIfMissingOrDifferent(stagingDir + '/fs.json', outDir + '/fs.json');

for (const name of readdirSync(stagingDir + '/objects')) {
  const dst = outDir + '/objects/' + name;
  if (!existsSync(dst)) { copyFileSync(stagingDir + '/objects/' + name, dst); copied++; }
}

writeFileSync(outDir + '/vm.json', JSON.stringify(vmJson, null, 2) + '\n');

const totalSize = dirSize(outDir);
console.log(`\nBiber VM release ${vmRelease}`);
console.log(`  output: ${outDir}`);
console.log(`  total size: ${(totalSize.bytes / (1024 * 1024)).toFixed(1)} MiB across ${totalSize.files} files`);
console.log(`  objects: ${readdirSync(outDir + '/objects').length}`);
console.log(`  files newly copied this run: ${copied}`);

// 7. Register in the manifest. Prefer package A's `wasmtex.mjs --vm`, since
// that is the documented, single writer of manifest.json's shape (section 1
// of docs/specs/wasmtex-interfaces.md). Fall back to register.mjs, which
// edits manifest.json additively, only if that flag is not implemented yet.
const wasmtexTool = fileURLToPath(new URL('../wasmtex.mjs', import.meta.url));
const repoRoot = fileURLToPath(new URL('../../../', import.meta.url));
const registerScript = fileURLToPath(new URL('./register.mjs', import.meta.url));

function currentVmEntry() {
  const m = JSON.parse(readFileSync(mirrorRoot + 'manifest.json', 'utf8'));
  return m.releases?.[m.default_release]?.vm ?? null;
}

let registered = false;
if (existsSync(wasmtexTool)) {
  try {
    execFileSync(process.execPath, [wasmtexTool, '--vm', outDir], { stdio: 'inherit', cwd: repoRoot });
    // wasmtex.mjs may exit 0 while silently ignoring an unrecognized flag
    // (it did, the first time this ran against an early version of that
    // file), so verify the manifest was actually updated before trusting it.
    registered = currentVmEntry()?.id === vmRelease;
    if (registered) console.log('Registered VM release in manifest.json via wasmtex.mjs --vm');
    else console.log('wasmtex.mjs ran but did not set releases.<default_release>.vm (--vm not implemented yet); using register.mjs instead');
  } catch (error) {
    console.log(`wasmtex.mjs --vm failed (${error.message.split('\n')[0]}); using register.mjs instead`);
  }
} else {
  console.log('latex/tools/wasmtex.mjs does not exist yet; using register.mjs instead');
}
if (!registered) {
  execFileSync(process.execPath, [registerScript, outDir], { stdio: 'inherit' });
  registered = currentVmEntry()?.id === vmRelease;
}
if (!registered) console.warn('WARNING: the VM release was built but could not be registered in manifest.json');
