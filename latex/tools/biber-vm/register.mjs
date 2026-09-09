// Registers a Biber VM release into the LaTeX mirror manifest.
//
// The mirror itself -- engines and the TeX Live package set -- is built and
// pushed from the wasm-latex repository (`make mirror`, `make push`; layout
// and manifest in wasm-latex/docs/mirror.md). This file's only job is the
// one piece LibrePaper still deploys on its own: the Biber VM, built by
// `latex/tools/biber-vm/build.mjs`, per docs/specs/latex-interfaces.md
// section 6 ("The build writes releases.<id>.vm = {...} into the manifest").
//
//   node latex/tools/biber-vm/register.mjs <biber-vm-release-dir>
//     Registers the VM's descriptor under the mirror's default release. The
//     mirror must already have that release (imported by wasm-latex's
//     tooling) before this can run.
import { createHash } from 'node:crypto';
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { basename, dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const OUT = fileURLToPath(new URL('../../mirror/', import.meta.url));
const sha256 = (bytes) => createHash('sha256').update(bytes).digest('hex');

/* ------------------------------------------------------------- manifest */

function readManifest() {
  const path = join(OUT, 'manifest.json');
  if (!existsSync(path)) return { format: 1, version: 1, releases: {} };
  return JSON.parse(readFileSync(path, 'utf8'));
}

function writeManifest(manifest) {
  mkdirSync(OUT, { recursive: true });
  writeFileSync(join(OUT, 'manifest.json'), JSON.stringify(manifest, null, 2) + '\n');
}

/// sha256 of a JSON object's canonical form: keys sorted at every level, so
/// the digest depends on content and not on insertion order or formatting.
function canonicalDigest(value) {
  const canon = (v) => {
    if (Array.isArray(v)) return v.map(canon);
    if (v && typeof v === 'object') {
      const out = {};
      for (const key of Object.keys(v).sort()) out[key] = canon(v[key]);
      return out;
    }
    return v;
  };
  return sha256(Buffer.from(JSON.stringify(canon(value))));
}

/// Registers a Biber VM release built by `latex/tools/biber-vm/build.mjs`
/// under the mirror's default release: the descriptor's digest is what the
/// browser verifies before it boots anything.
export function registerVm(dir) {
  const descriptorPath = join(dir, 'vm.json');
  if (!existsSync(descriptorPath)) throw new Error(`register: no vm.json in ${dir}`);
  const bytes = readFileSync(descriptorPath);
  const descriptor = JSON.parse(bytes.toString('utf8'));
  const manifest = readManifest();
  const release = manifest.releases?.[manifest.default_release];
  if (!release) throw new Error('register: no default release in the mirror; build it with wasm-latex\'s make mirror/push first');
  const id = basename(dir);
  release.vm = { id, url: `biber-vm/${id}/vm.json`, sha256: sha256(bytes), size: bytes.length, biber: descriptor.biber };
  release.digest = canonicalDigest({ ...release, digest: undefined });
  writeManifest(manifest);
  console.log(`register: vm ${id} (biber ${descriptor.biber}) registered on ${manifest.default_release}`);
}

/* --------------------------------------------------------------------- run */

async function main() {
  const dir = process.argv[2];
  if (!dir) {
    console.error('usage: node latex/tools/biber-vm/register.mjs <biber-vm-release-dir>');
    process.exit(1);
  }
  registerVm(dir);
}

if (process.argv[1] && process.argv[1].endsWith('register.mjs')) {
  main().catch((error) => {
    console.error(error.message);
    process.exit(1);
  });
}
