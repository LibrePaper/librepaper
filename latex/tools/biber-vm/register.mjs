// Compute the deployment flag for a built Biber VM.
//
// The mirror itself -- engines and the TeX Live package set -- is built and
// pushed from the wasm-latex repository (`make mirror`, `make push`; layout
// and manifest in wasm-latex/docs/mirror.md). The engine mirror is immutable
// and carries no VM metadata. Publish the built directory separately, then
// pass its public vm.json URL here. This command only reads the descriptor.
//
//   node latex/tools/biber-vm/register.mjs <release-dir> <published-vm.json-url>
//     Prints --biber-vm with the local descriptor's digest; it never edits a mirror.
import { createHash } from 'node:crypto';
import { existsSync, readFileSync } from 'node:fs';
import { join } from 'node:path';

const sha256 = (bytes) => createHash('sha256').update(bytes).digest('hex');

export function describeVm(dir, url) {
  const descriptorPath = join(dir, 'vm.json');
  if (!existsSync(descriptorPath)) throw new Error(`register: no vm.json in ${dir}`);
  let published;
  try { published = new URL(url); } catch { throw new Error(`register: invalid published vm.json URL: ${url}`); }
  if (!['http:', 'https:'].includes(published.protocol) || published.hash) {
    throw new Error('register: use an HTTP(S) vm.json URL without a fragment');
  }
  const bytes = readFileSync(descriptorPath);
  const descriptor = JSON.parse(bytes.toString('utf8'));
  const digest = sha256(bytes);
  console.log(`register: biber ${descriptor.biber ?? 'unknown'} (manifest remains unchanged)`);
  console.log(`--biber-vm ${url}#${digest}`);
  return { url, sha256: digest, size: bytes.length, biber: descriptor.biber };
}

/* --------------------------------------------------------------------- run */

async function main() {
  const dir = process.argv[2];
  const url = process.argv[3];
  if (!dir || !url) {
    console.error('usage: node latex/tools/biber-vm/register.mjs <release-dir> <published-vm.json-url>');
    process.exit(1);
  }
  describeVm(dir, url);
}

if (process.argv[1] && process.argv[1].endsWith('register.mjs')) {
  main().catch((error) => {
    console.error(error.message);
    process.exit(1);
  });
}
