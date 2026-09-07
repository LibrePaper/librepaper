import { createHash } from 'node:crypto';
import { existsSync, readFileSync, readdirSync, statSync } from 'node:fs';
import { join, relative } from 'node:path';

const DEFAULT_ROOT = new URL('../tinytex-v86/assets/rootfs/opt/tinytex/', import.meta.url).pathname;
const SKIP = /(^|\/)(doc|source|bin)(\/|$)/;
const FORMAT_EXTENSIONS = {
  tex: ['.tex', '.sty', '.cls', '.def', '.cfg', '.ltx', '.fd', '.ldf', '.bbx', '.cbx', '.dbx', '.lbx', '.clo', '.ini', '.txt', '.dat'],
  pdftex: ['.tex', '.sty', '.cls', '.def', '.cfg', '.fd', '.map', '.tfm', '.pk'],
  xetex: ['.tex', '.sty', '.cls', '.def', '.cfg', '.fd', '.map', '.tfm', '.otf', '.ttf'],
  luatex: ['.tex', '.sty', '.cls', '.def', '.cfg', '.fd', '.map', '.tfm', '.otf', '.ttf'],
  tfm: ['.tfm'], map: ['.map'], opentype: ['.otf'], truetype: ['.ttf'],
  config: ['.cnf', '.cfg', '.dat', '.map'],
  '3': ['.tfm'], '6': ['.bib'], '7': ['.bst'], '8': ['.cnf'],
  '11': ['.map'], '32': ['.pfb', '.pfa'], '33': ['.vf'],
  '36': ['.ttf', '.ttc'], '44': ['.enc'], '47': ['.otf'],
};

function filesUnder(root, directory = root) {
  return readdirSync(directory, { withFileTypes: true }).flatMap(entry => {
    const full = join(directory, entry.name), rel = relative(root, full).replaceAll('\\', '/');
    if (SKIP.test(rel)) return [];
    return entry.isDirectory() ? filesUnder(root, full) : [full];
  });
}

function digest(bytes) { return createHash('sha256').update(bytes).digest('hex'); }

/** Build a read-only resolver over the prepared TinyTeX tree. */
export function createTinytexResolver({ root = DEFAULT_ROOT } = {}) {
  const rootPath = root.endsWith('/') ? root.slice(0, -1) : root;
  if (!existsSync(rootPath)) throw new Error(`TinyTeX resource root does not exist: ${rootPath}`);
  const receiptPath = join(rootPath, '../../../filesystem-receipt.json');
  const receipt = existsSync(receiptPath) ? JSON.parse(readFileSync(receiptPath, 'utf8')) : null;
  const entries = ['texmf-config', 'texmf-var', 'texmf-local', 'texmf-dist']
    .flatMap(name => existsSync(join(rootPath, name)) ? filesUnder(join(rootPath, name)) : [])
    .map(sourcePath => {
    const rel = relative(rootPath, sourcePath).replaceAll('\\', '/');
    const bytes = readFileSync(sourcePath);
    const runtime = /^(texmf-config|texmf-var|texmf-local)\//.test(rel);
    return { sourcePath, rel, basename: rel.split('/').at(-1), bytes, sha256: digest(bytes), fileid: rel.split('/').at(-1), runtime, priority: runtime ? 0 : 1 };
  });
  const byName = new Map();
  for (const entry of entries) { const list = byName.get(entry.basename) || []; list.push(entry); byName.set(entry.basename, list); }
  for (const list of byName.values()) list.sort((a, b) => a.priority - b.priority || a.rel.localeCompare(b.rel));
  const treeIdentity = digest(Buffer.from(entries.map(e => `${e.rel}\0${e.sha256}`).sort().join('\n')));
  const resolve = (format, requested) => {
    if (typeof requested !== 'string' || !requested || requested.includes('/') || requested.includes('\\') || requested.includes('\0') || String(format) === '10') return null;
    const name = requested;
    const extensions = FORMAT_EXTENSIONS[format] || FORMAT_EXTENSIONS.tex;
    const names = [name];
    if (!name.includes('.')) for (const ext of extensions) names.push(name + ext);
    for (const candidate of names) {
      const found = byName.get(candidate)?.find(entry => !entry.basename.endsWith('.fmt'));
      if (found) return { bytes: new Uint8Array(found.bytes), sourcePath: found.sourcePath, sha256: found.sha256, fileid: found.fileid };
    }
    return null;
  };
  return { resolve, root: rootPath, count: entries.length, treeIdentity, receipt };
}

export { DEFAULT_ROOT };
