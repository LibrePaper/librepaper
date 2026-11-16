// Putting the project tree into one nested engine before a compile.
//
// `worker.js` stages every project engine before each compile. The old way
// flushed each engine's `/work` and wrote the whole tree back, which on a
// large project costs more than the compile's own bookkeeping. An engine
// that advertises `retain` (see `release.capabilities` in `worker.js`) can
// instead keep what it already holds: `retain` deletes everything not on the
// keep list and reports which kept files changed since the host last wrote
// them, so only those, and the files new to the engine, are written again.
//
// This module has no imports on purpose: it is the one piece of the worker
// the unit tests drive with fake engines, so it must load in plain Node.

/// Engine kinds whose controller answers `retain` and `flushcache` alike.
/// Kept in sync with the controllers' own support; `driver.js` checks the
/// same set before sending the command.
export const RETAIN_KINDS = new Set(["pdftex", "xetex", "luatex", "dvipdfm"]);

export function dirname(path) {
  const at = path.lastIndexOf("/");
  return at < 0 ? null : path.slice(0, at);
}

export async function ensureDirs(engine, path) {
  const dir = dirname(path);
  // A file at the root has no directory to make, and the engine's
  // `mkdir .` is an error it prints to the console for every file.
  if (dir && dir !== "." && dir !== "/") await engine.mkdir(dir);
}

export async function writeFiles(engine, files) {
  for (const [path, contents] of Object.entries(files ?? {})) {
    await ensureDirs(engine, path);
    await engine.writeFile(path, contents);
  }
}

/// Byte-for-byte equality of two byte arrays. Returns on the first
/// difference; a string on either side is never the same as bytes.
export function sameBytes(a, b) {
  if (!(a instanceof Uint8Array) || !(b instanceof Uint8Array)) return false;
  if (a.length !== b.length) return false;
  for (let i = 0; i < a.length; i++) {
    if (a[i] !== b[i]) return false;
  }
  return true;
}

function sameValue(previous, next) {
  if (typeof previous === "string" || typeof next === "string") return previous === next;
  return sameBytes(previous, next);
}

/// Stages `tree` into one engine. `engine.staged` is `null` or a Map of the
/// tree files this engine last received from the host; `stageEngine` owns it.
///
/// `generated` (aux/bbl/ind from an earlier pass) is never written here and
/// never recorded in `staged`: the caller writes it after the tree, and
/// `retain` keeps its names so a compile's own changes to them are reported
/// dirty instead of silently deleted.
export async function stageEngine(engine, tree, { canRetain = false, generated = null } = {}) {
  const supported = canRetain === true && RETAIN_KINDS.has(engine.kind);
  const texts = tree?.texts ?? {};
  const assets = tree?.assets ?? {};
  const entries = [...Object.entries(texts), ...Object.entries(assets)];

  if (supported && engine.staged instanceof Map) {
    const keep = [...new Set([...entries.map(([path]) => path), ...Object.keys(generated ?? {})])];
    let dirty = null;
    try {
      dirty = new Set(await engine.retain(keep));
    } catch {
      // Fall through to the flush path below: a failed retain may have
      // deleted some files, so the engine is rebuilt from scratch.
      dirty = null;
    }
    if (dirty) {
      const previous = engine.staged;
      // Cleared first: if a write below fails, the next stage must not trust
      // a record that no longer matches what the engine holds.
      engine.staged = null;
      for (const [path, value] of entries) {
        if (dirty.has(path) || !previous.has(path) || !sameValue(previous.get(path), value)) {
          await ensureDirs(engine, path);
          await engine.writeFile(path, value);
        }
      }
      engine.staged = new Map(entries);
      return;
    }
  }

  engine.staged = null;
  await engine.flushCache();
  await writeFiles(engine, texts);
  await writeFiles(engine, assets);
  engine.staged = supported ? new Map(entries) : null;
}
