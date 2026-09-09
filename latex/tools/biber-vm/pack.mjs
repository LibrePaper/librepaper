// Packs an exported guest rootfs into v86's 9p filesystem.json + content-
// addressed objects, the way v86's own fs2json packer lays them out, taking
// explicit source/destination directories so it can be reused for any guest.
//
// Objects are stored as raw bytes:
// v86 reads each object's bytes directly as file content through its 9p
// filesystem, so the stored bytes must be the guest file's exact bytes.
// "Gzip-compressed delivery" (docs/specs/latex-compiler.md) happens at the HTTP layer
// (Content-Encoding: gzip, as the production static mirror does), which the browser's fetch
// transparently decompresses before v86 ever sees the bytes -- storing
// pre-gzipped bytes as the object content would corrupt every guest file.
import { createHash } from 'node:crypto';
import { existsSync, lstatSync, mkdirSync, readFileSync, readdirSync, readlinkSync, writeFileSync } from 'node:fs';

export function pack(rootfsDir, outDir) {
  const objectsDir = outDir + '/objects';
  mkdirSync(objectsDir, { recursive: true });
  const hashes = new Map();
  let bytes = 0, files = 0, objects = 0;

  function walk(relative = '') {
    return readdirSync(rootfsDir + '/' + relative).sort().flatMap((name) => {
      const rel = relative ? relative + '/' + name : name;
      if (/^(?:dev|proc|sys)\//.test(rel)) return [];
      const absolute = rootfsDir + '/' + rel;
      const st = lstatSync(absolute);
      // Fixed mtime (not st.mtimeMs): container export/creation stamps a few
      // top-level entries (notably .dockerenv) with the current build time,
      // which would otherwise make fs.json - and therefore <vmRelease>,
      // computed from vm.json's digest of fs.json - non-reproducible across
      // otherwise-identical builds. The guest boot process does not depend
      // on file mtimes.
      const node = [name, st.size, 0, st.mode, 0, 0];
      if (st.isSymbolicLink()) {
        node.push(readlinkSync(absolute));
      } else if (st.isDirectory()) {
        node.push(walk(rel));
      } else if (st.isFile()) {
        const data = readFileSync(absolute);
        const sha = createHash('sha256').update(data).digest('hex');
        const filename = sha.slice(0, 10) + '.bin';
        if (hashes.has(filename) && hashes.get(filename) !== sha) throw new Error('Truncated hash collision: ' + filename);
        if (!hashes.has(filename)) {
          hashes.set(filename, sha);
          if (!existsSync(objectsDir + '/' + filename)) {
            writeFileSync(objectsDir + '/' + filename, data);
          }
          objects++;
        }
        node.push(filename);
        files++;
        bytes += st.size;
      } else {
        return [];
      }
      return [node];
    });
  }

  const fsroot = walk();
  const manifest = { fsroot, version: 3, size: bytes };
  writeFileSync(outDir + '/fs.json', JSON.stringify(manifest));
  return { files, bytes, objects, manifestSha256: createHash('sha256').update(JSON.stringify(manifest)).digest('hex') };
}

if (import.meta.url === `file://${process.argv[1]}`) {
  const [, , src, dst] = process.argv;
  if (!src || !dst) { console.error('usage: pack.mjs <rootfsDir> <outDir>'); process.exit(1); }
  console.log(pack(src, dst));
}
