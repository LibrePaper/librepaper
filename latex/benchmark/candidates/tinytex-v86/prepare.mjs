import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
const root = fileURLToPath(new URL('.', import.meta.url));
mkdirSync(root + 'assets', { recursive: true });
const recipe = 'd01200eb642187eb3b00b02084500844906cbeef';
const sources = {
  'libv86.js': 'https://copy.sh/v86/build/libv86.js',
  'v86.wasm': 'https://copy.sh/v86/build/v86.wasm',
  'seabios.bin': 'https://raw.githubusercontent.com/copy/v86/master/bios/seabios.bin',
  'vgabios.bin': 'https://raw.githubusercontent.com/copy/v86/master/bios/vgabios.bin',
  'buildroot-bzimage68.bin': 'https://i.copy.sh/buildroot-bzimage68.bin',
  'install-tl.tar.gz': 'https://texlive.info/historic/systems/texlive/2025/tlnet-final/install-tl-unx.tar.gz',
  'pkgs-custom.txt': `https://raw.githubusercontent.com/rstudio/tinytex/${recipe}/tools/pkgs-custom.txt`,
};
const lockPath = root + 'assets-lock.json';
const previous = existsSync(lockPath) ? JSON.parse(readFileSync(lockPath)) : {};
const assets = {};
for (const [name, url] of Object.entries(sources)) {
  const path = root + 'assets/' + name;
  if (!existsSync(path)) execFileSync('curl', ['-fsSL', '--compressed', '--retry', '2', '--max-time', '120', url, '-o', path], { stdio: 'inherit' });
  const data = readFileSync(path);
  const sha256 = createHash('sha256').update(data).digest('hex');
  if (previous.assets?.[name] && previous.assets[name].sha256 !== sha256) throw new Error(`Asset digest changed: ${name}`);
  assets[name] = { url, bytes: data.length, sha256 };
  console.log(`${name}: ${data.length} bytes`);
}
writeFileSync(lockPath, JSON.stringify({ recipe, assets }, null, 2) + '\n');
