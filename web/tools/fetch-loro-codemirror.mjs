// Fetches the CodeMirror binding named in loro-codemirror.lock, and verifies it.
//
// The binding is a fork of loro-codemirror carrying fixes upstream has not
// released (docs/loro-codemirror.md). It used to be vendored into the
// repository, which meant the same source existed twice -- here and in the
// fork -- and the two drifted, with the fork behind for a while without
// anything noticing. So the fork is the only copy, and this is how it arrives:
// by commit, over HTTPS, checked against the digest recorded in the lock.
//
// A file whose bytes do not hash to what the lock says is not written. A build
// that silently compiled a different binding than the one it pinned would be
// worse than a build that failed, and this binding is the thing standing
// between an editor and the shared document.
//
// Already-correct files are left alone, so this is cheap to run on every build
// and downloads nothing when nothing moved.

import { createHash } from "node:crypto";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { existsSync } from "node:fs";
import { basename, join } from "node:path";

const root = new URL("../..", import.meta.url).pathname;
const lock = join(root, "loro-codemirror.lock");
const out = join(root, "web/vendor/loro-codemirror");

const digest = (bytes) => createHash("sha256").update(bytes).digest("hex");

const lines = (await readFile(lock, "utf8"))
  .split("\n")
  .map((line) => line.replace(/#.*$/, "").trim())
  .filter(Boolean);

const directive = (name) => {
  const found = lines.filter((line) => line.startsWith(`${name} `));
  if (found.length !== 1) throw new Error(`loro-codemirror.lock must name exactly one ${name}`);
  return found[0].slice(name.length).trim();
};

const repository = directive("repository");
const commit = directive("commit");
if (!/^[A-Za-z0-9._-]+\/[A-Za-z0-9._-]+$/.test(repository)) {
  throw new Error(`loro-codemirror.lock: "${repository}" is not an owner/repository`);
}
// A full sha rather than a branch or a tag. A branch moves under the pin, and
// a tag can be repointed; neither is a name for a particular set of bytes.
if (!/^[a-f0-9]{40}$/.test(commit)) {
  throw new Error(`loro-codemirror.lock: "${commit}" is not a full 40-character commit sha`);
}

const entries = lines
  .filter((line) => !line.startsWith("repository ") && !line.startsWith("commit "))
  .map((line) => {
    const [path, sha256, ...extra] = line.split(/\s+/);
    if (extra.length || !/^[a-f0-9]{64}$/.test(sha256 || "")) {
      throw new Error(`${path || "lock entry"}: expected "path sha256"`);
    }
    return { path, sha256 };
  });

// The editor imports these by name, so a lock that is missing one fails the
// build at compile time with a module-not-found rather than here, where it can
// say what is actually wrong.
const required = ["src/index.ts", "src/sync.ts", "src/undo.ts", "src/awareness.ts", "src/ephemeral.ts", "src/utils.ts", "LICENSE"];
const missing = required.filter((path) => !entries.some((entry) => entry.path === path));
if (missing.length) throw new Error(`loro-codemirror.lock is missing: ${missing.join(", ")}`);

await mkdir(out, { recursive: true });
let fetched = 0;

for (const { path, sha256 } of entries) {
  // Flattened on the way in: upstream keeps these under src/, and the editor
  // imports them from one directory.
  const target = join(out, basename(path));
  if (existsSync(target) && digest(await readFile(target)) === sha256) {
    console.log(`${basename(path).padEnd(16)} ok`);
    continue;
  }
  const url = `https://raw.githubusercontent.com/${repository}/${commit}/${path}`;
  const response = await fetch(url);
  if (!response.ok) throw new Error(`${path}: ${url} -> ${response.status} ${response.statusText}`);
  const bytes = Buffer.from(await response.arrayBuffer());
  const got = digest(bytes);
  if (got !== sha256) {
    throw new Error(
      `${path}: the bytes at ${url} are not what loro-codemirror.lock pins.\n` +
        `  expected ${sha256}\n  received ${got}\n` +
        "Nothing was written. Either the commit was rewritten, or the lock is stale.",
    );
  }
  await writeFile(target, bytes);
  fetched += 1;
  console.log(`${basename(path).padEnd(16)} ${(bytes.length / 1024).toFixed(1)} KiB  ${commit.slice(0, 8)}`);
}

if (fetched === 0) console.log(`every pinned file was already in place (${commit.slice(0, 8)})`);
