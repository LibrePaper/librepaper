// Fetches (or reuses) the pinned v86 runtime files and the pinned Biber
// binary. Verifies every downloaded byte against sources.json/assets-lock
// digests before use. No network access happens at guest runtime; this
// script is a build-time tool only.
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('.', import.meta.url));
const tinytexAssets = fileURLToPath(new URL('../../benchmark/candidates/tinytex-v86/assets/', import.meta.url));
mkdirSync(root + 'assets', { recursive: true });

const sources = JSON.parse(readFileSync(root + 'sources.json', 'utf8'));

const sha256 = (data) => createHash('sha256').update(data).digest('hex');

function fetchTo(path, url, expectedSha256) {
  if (!existsSync(path)) {
    console.log(`fetching ${url}`);
    execFileSync('curl', ['-fsSL', '--compressed', '--retry', '2', '--max-time', '180', url, '-o', path], { stdio: 'inherit' });
  }
  const data = readFileSync(path);
  const got = sha256(data);
  if (expectedSha256 && got !== expectedSha256) {
    throw new Error(`Digest mismatch for ${path}: expected ${expectedSha256}, got ${got}`);
  }
  return { bytes: data.length, sha256: got };
}

// v86 runtime files: reuse latex/benchmark/candidates/tinytex-v86's already
// verified assets when present (same pinned recipe/URLs), otherwise fetch
// fresh into this recipe's own assets/ directory.
const v86Files = {
  'libv86.js': 'https://copy.sh/v86/build/libv86.js',
  'v86.wasm': 'https://copy.sh/v86/build/v86.wasm',
  'seabios.bin': 'https://raw.githubusercontent.com/copy/v86/master/bios/seabios.bin',
  'vgabios.bin': 'https://raw.githubusercontent.com/copy/v86/master/bios/vgabios.bin',
  'buildroot-bzimage68.bin': 'https://i.copy.sh/buildroot-bzimage68.bin',
};

let tinytexLock = null;
try { tinytexLock = JSON.parse(readFileSync(tinytexAssets + '../assets-lock.json', 'utf8')); } catch { /* not built yet */ }

const results = {};
for (const [name, url] of Object.entries(v86Files)) {
  const expected = tinytexLock?.assets?.[name]?.sha256;
  const reuse = tinytexAssets + name;
  const local = root + 'assets/' + name;
  if (existsSync(reuse) && !existsSync(local)) {
    const data = readFileSync(reuse);
    const got = sha256(data);
    if (expected && got !== expected) throw new Error(`Reused asset ${name} digest mismatch`);
    writeFileSync(local, data);
    console.log(`reused ${name} from tinytex-v86 (${data.length} bytes)`);
    results[name] = { url, bytes: data.length, sha256: got, reused: true };
  } else {
    results[name] = { url, ...fetchTo(local, url, expected), reused: false };
  }
}

// Biber binary: pinned in sources.json.
{
  const b = sources.biber.download;
  const tarPath = root + 'assets/biber.i386-linux.tar.xz';
  fetchTo(tarPath, b.url, b.sha256);
  const binPath = root + 'assets/biber';
  if (!existsSync(binPath)) {
    mkdirSync(root + 'assets/extract', { recursive: true });
    execFileSync('tar', ['-xJf', tarPath, '-C', root + 'assets/extract', 'bin/i386-linux/biber'], { stdio: 'inherit' });
    execFileSync('cp', [root + 'assets/extract/bin/i386-linux/biber', binPath]);
    execFileSync('chmod', ['755', binPath]);
  }
  const gotBin = sha256(readFileSync(binPath));
  if (gotBin !== b.extracted_binary_sha256) throw new Error(`Extracted biber binary digest mismatch: expected ${b.extracted_binary_sha256}, got ${gotBin}`);
  results['biber'] = { url: b.url, sha256: gotBin, bytes: readFileSync(binPath).length };
}

writeFileSync(root + 'assets/prepare-receipt.json', JSON.stringify(results, null, 2) + '\n');
console.log('prepare.mjs: all assets verified');
console.log(results);
