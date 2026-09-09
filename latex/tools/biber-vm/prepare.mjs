// Fetches (or reuses) the pinned v86 runtime files and the pinned Biber
// binary. Verifies every downloaded byte against sources.json/assets-lock
// digests before use. No network access happens at guest runtime; this
// script is a build-time tool only.
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('.', import.meta.url));
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

// v86 runtime files: pinned per file by URL and digest in sources.json.
const results = {};
for (const [name, file] of Object.entries(sources.v86_runtime.files)) {
  const local = root + 'assets/' + name;
  const got = fetchTo(local, file.url, file.sha256);
  if (got.bytes !== file.bytes) throw new Error(`Size mismatch for ${name}: expected ${file.bytes} bytes, got ${got.bytes}`);
  results[name] = { url: file.url, ...got };
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
