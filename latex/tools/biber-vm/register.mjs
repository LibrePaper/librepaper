// Fallback registration: edits latex/mirror/manifest.json additively to set
// releases.<default_release>.vm, per docs/specs/wasmtex-interfaces.md section
// 6 ("The build writes releases.<id>.vm = {...} into the manifest through
// latex/tools/wasmtex.mjs --vm <dir>"). Used only when that flag does not
// exist yet in latex/tools/wasmtex.mjs (package A's file, not edited here).
// Never touches any other manifest key.
import { createHash } from 'node:crypto';
import { readFileSync, statSync, writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

const manifestPath = fileURLToPath(new URL('../../mirror/manifest.json', import.meta.url));

const outDir = process.argv[2];
if (!outDir) { console.error('usage: register.mjs <biber-vm-release-dir>'); process.exit(1); }

const vmJsonPath = outDir.replace(/\/$/, '') + '/vm.json';
const vmJson = JSON.parse(readFileSync(vmJsonPath, 'utf8'));
const vmRelease = outDir.replace(/\/$/, '').split('/').pop();

const manifest = JSON.parse(readFileSync(manifestPath, 'utf8'));
if (!manifest.releases || !manifest.default_release || !manifest.releases[manifest.default_release]) {
  throw new Error('manifest.json has no releases.<default_release> to attach vm to');
}

const vmBytes = readFileSync(vmJsonPath);
const entry = {
  id: vmRelease,
  url: `biber-vm/${vmRelease}/vm.json`,
  sha256: createHash('sha256').update(vmBytes).digest('hex'),
  size: statSync(vmJsonPath).size,
  biber: vmJson.biber,
};

manifest.releases[manifest.default_release].vm = entry;

writeFileSync(manifestPath, JSON.stringify(manifest, null, 2) + '\n');
console.log(`register.mjs: wrote releases.${manifest.default_release}.vm =`, entry);
