import { createHash } from 'node:crypto';
import { existsSync, lstatSync, mkdirSync, readFileSync, readdirSync, readlinkSync, writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
const root = fileURLToPath(new URL('.', import.meta.url));
const directory = root + 'assets/rootfs';
const objects = root + 'assets/objects';
mkdirSync(objects, { recursive: true });
const hashes = new Map();
const paths = {};
let bytes = 0, files = 0;
function walk(relative = '') {
  return readdirSync(directory + '/' + relative).sort().flatMap(name => {
    const rel = relative ? relative + '/' + name : name;
    if (/^(?:dev|proc|sys)\//.test(rel)) return [];
    const absolute = directory + '/' + rel;
    const st = lstatSync(absolute);
    const node = [name, st.size, Math.floor(st.mtimeMs / 1000), st.mode, 0, 0];
    if (st.isSymbolicLink()) node.push(readlinkSync(absolute));
    else if (st.isDirectory()) node.push(walk(rel));
    else if (st.isFile()) {
      const data = readFileSync(absolute);
      const sha = createHash('sha256').update(data).digest('hex');
      const filename = sha.slice(0, 10) + '.bin';
      if (hashes.has(filename) && hashes.get(filename) !== sha) throw new Error('Truncated hash collision');
      hashes.set(filename, sha);
      if (!existsSync(objects + '/' + filename)) writeFileSync(objects + '/' + filename, data);
      paths[filename] ??= [];
      paths[filename].push(rel);
      node.push(filename);
      files++; bytes += st.size;
    } else return [];
    return [node];
  });
}
const manifest = { fsroot: walk(), version: 3, size: bytes };
writeFileSync(root + 'assets/fs.json', JSON.stringify(manifest));
writeFileSync(root + 'assets/object-paths.json', JSON.stringify(paths));
writeFileSync(root + 'assets/filesystem-receipt.json', JSON.stringify({ files, bytes, objects: hashes.size, manifestSha256: createHash('sha256').update(JSON.stringify(manifest)).digest('hex') }, null, 2) + '\n');
console.log({ files, bytes, objects: hashes.size });
