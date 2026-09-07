// Standalone Biber bridge for browser-side hybrid TeX runtimes.
//
// This module deliberately invokes only Biber. It creates its own CheerpX VM
// and never calls an engine or latexmk.

const DEFAULT_TIMEOUT = 10 * 60 * 1000;
export const CHEERPX_RUNTIME_URL = "https://cxrtnc.leaningtech.com/1.2.8/cx.esm.js";
export const DEFAULT_EXT2_URL = "/assets/rootfs.ext2";
let sequence = 0;
let vmPromise;
let vmIdentity;
let poisoned = false;
let busy = Promise.resolve();

const now = () => performance.now();
const asBytes = value => {
  if (value instanceof Uint8Array) return value;
  if (value instanceof ArrayBuffer) return new Uint8Array(value);
  if (ArrayBuffer.isView(value)) return new Uint8Array(value.buffer, value.byteOffset, value.byteLength);
  throw new TypeError("Biber bridge inputs must be Uint8Array-compatible bytes");
};

function relativePath(path) {
  if (typeof path !== "string" || !path || path.startsWith("/") || path.includes("\\") ||
      path.split("/").some(part => !part || part === "." || part === "..") || /[\u0000-\u001f\u007f]/.test(path)) {
    throw new TypeError(`Invalid relative guest path: ${String(path)}`);
  }
  return path;
}

function inputEntry(value, fallback) {
  if (value && typeof value === "object" && !(value instanceof Uint8Array) &&
      !(value instanceof ArrayBuffer) && !ArrayBuffer.isView(value) && "bytes" in value) {
    return { path: relativePath(value.path || fallback), bytes: asBytes(value.bytes) };
  }
  return { path: relativePath(fallback), bytes: asBytes(value) };
}

function inputEntries(value, fallback) {
  if (value == null) return [];
  if (value instanceof Uint8Array || value instanceof ArrayBuffer || ArrayBuffer.isView(value)) {
    return [inputEntry(value, fallback)];
  }
  if (value.bytes != null) return [inputEntry(value, fallback)];
  if (typeof value !== "object") throw new TypeError("Biber bridge file collection must be an object");
  return Object.entries(value).map(([path, bytes]) => inputEntry({ path, bytes }, path));
}

export async function lazywarmVM({ timeoutMs = DEFAULT_TIMEOUT, runtimeUrl = CHEERPX_RUNTIME_URL,
  ext2Url = DEFAULT_EXT2_URL, cacheName = "biber-bridge" } = {}) {
  if (poisoned) throw new Error("CheerpX timed out; reload before another request");
  const identity = JSON.stringify([runtimeUrl, ext2Url, cacheName]);
  if (vmPromise && vmIdentity !== identity) throw new Error("Cannot change the active Biber environment");
  if (vmPromise) return vmPromise;
  vmIdentity = identity;
  vmPromise = timedCommand(() => createVM({ runtimeUrl, ext2Url, cacheName }), "", timeoutMs);
  try { return await vmPromise; } catch (error) { vmPromise = null; throw error; }
}

async function createVM({ timeoutMs, runtimeUrl, ext2Url, cacheName }) {
  const started = now();
  const C = await import(runtimeUrl);
  const disk = await C.HttpBytesDevice.create(ext2Url);
  const cache = await C.IDBDevice.create(`${cacheName}-disk`);
  const overlay = await C.OverlayDevice.create(disk, cache);
  const work = await C.IDBDevice.create(`${cacheName}-work`);
  const input = await C.DataDevice.create();
  const machine = await C.Linux.create({ mounts: [
    { type: "ext2", path: "/", dev: overlay },
    { type: "dir", path: "/export", dev: work },
    { type: "dir", path: "/input", dev: input },
    { type: "devs", path: "/dev" },
  ] });
  let serial = "";
  const decoder = new TextDecoder();
  machine.setCustomConsole(bytes => {
    serial = (serial + decoder.decode(bytes, { stream: true })).slice(-100000);
  }, 120, 40);
  const run = (command, uid = 1000) => machine.run("/bin/sh", ["-c", command], {
    env: ["PATH=/opt/tinytex/bin/i386-linux:/usr/bin:/bin", "LC_ALL=C.UTF-8", "HOME=/tmp"],
    cwd: "/work", uid, gid: uid === 0 ? 0 : 100,
  });
  const write = async (path, bytes) => {
    const id = `input-${sequence++}`;
    await input.writeFile(`/${id}`, asBytes(bytes));
    const result = await run(`cat ${shellQuote(`/input/${id}`)} > ${shellQuote(path)}`);
    if (result.status) throw new Error(`Input staging failed (exit ${result.status})`);
  };
  const read = async path => {
    const id = `export-${sequence++}`;
    // The IDB export mount starts owned by guest root. Only this copy uses
    // root; Biber and source staging continue to use the ordinary guest user.
    const result = await run(`cat ${shellQuote(path)} > ${shellQuote(`/export/${id}`)}`, 0);
    if (result.status) return null;
    const blob = await work.readFileAsBlob(`/${id}`);
    return blob ? new Uint8Array(await blob.arrayBuffer()) : null;
  };
  return { machine, run, write, read, get serial() { return serial; }, milliseconds: now() - started,
    runtime: { hostedRuntime: runtimeUrl, ext2Url } };
}

const shellQuote = path => "'" + path.replaceAll("'", "'\\''") + "'";

/**
 * Run Biber in an isolated CheerpX directory.
 *
 * bcf: Uint8Array or {path, bytes}; bib/config: Uint8Array or path->bytes map.
 * Returns raw bbl/blg bytes (or null when Biber did not create that file).
 */
export function runBiber(options = {}) {
  const next = busy.then(() => runBiberUnlocked(options));
  busy = next.catch(() => {});
  return next;
}

async function runBiberUnlocked({ bcf, bib, config, timeoutMs = DEFAULT_TIMEOUT,
  runtimeUrl = CHEERPX_RUNTIME_URL, ext2Url = DEFAULT_EXT2_URL } = {}) {
  const source = inputEntry(bcf, "document.bcf");
  if (!/^[A-Za-z0-9_][A-Za-z0-9_.-]*\.bcf$/.test(source.path)) {
    throw new TypeError("This prototype requires a simple BCF basename");
  }
  const stem = source.path.slice(0, -4);
  const files = [source, ...inputEntries(bib, "bibliography.bib"), ...inputEntries(config, "biber.conf")];
  const duplicate = new Set();
  for (const file of files) {
    if (duplicate.has(file.path)) throw new TypeError(`Duplicate Biber input path: ${file.path}`);
    duplicate.add(file.path);
  }
  const initStart = now();
  const init = await lazywarmVM({ timeoutMs, runtimeUrl, ext2Url });
  const nonce = globalThis.crypto?.randomUUID?.() || `${Date.now()}-${sequence++}`;
  const dir = `/work/.biber-bridge-${nonce}`;
  const timings = { initializationMilliseconds: now() - initStart };
  const stageStart = now();
  const dirs = [...new Set(files.flatMap(file => {
    const parts = file.path.split("/");
    return parts.slice(0, -1).map((_, i) => parts.slice(0, i + 1).join("/"));
  }))];
  const madeResult = await timedCommand(init.run, `mkdir -p ${[dir, ...dirs.map(path => `${dir}/${path}`)].map(shellQuote).join(" ")}`, timeoutMs);
  if (madeResult.status !== 0) throw new Error(`Cannot create Biber workspace (exit ${madeResult.status})`);
  for (const file of files) await init.write(`${dir}/${file.path}`, file.bytes);
  timings.stagingMilliseconds = now() - stageStart;

  const processStart = now();
  const conf = config ? [...inputEntries(config, "biber.conf")][0]?.path : null;
  const options = conf ? ` --configfile=${shellQuote(conf)}` : "";
  const processResult = await timedCommand(init.run,
    `cd ${shellQuote(dir)} && rm -f ${shellQuote(`${stem}.bbl`)} ${shellQuote(`${stem}.blg`)} && biber${options} ${shellQuote(source.path)}`, timeoutMs);
  if (!Number.isInteger(processResult.status)) throw new Error("Biber command returned no guest exit code");
  timings.biberMilliseconds = now() - processStart;
  const exportStart = now();
  const bbl = await init.read(`${dir}/${stem}.bbl`);
  const blg = await init.read(`${dir}/${stem}.blg`);
  timings.exportMilliseconds = now() - exportStart;
  return { bbl, blg, exitCode: processResult.status, success: processResult.status === 0 && bbl instanceof Uint8Array,
    timings, workspace: dir, runtime: init.runtime, serial: init.serial };
}

async function timedCommand(run, command, timeoutMs) {
  let timer;
  try {
    return await Promise.race([
      run(command),
      new Promise((_, reject) => { timer = setTimeout(() => { poisoned = true; reject(new Error("Biber command timed out; reload required")); }, timeoutMs); }),
    ]);
  } finally { clearTimeout(timer); }
}
