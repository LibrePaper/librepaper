// Biber WASM from the selected LaTeX release. Every executable/data byte is
// verified before use. A fresh worker isolates each run and can be terminated.
import { fetchVerified } from './resources.js';

let assets = null;
const active = new Set();
const TIMEOUT_MS = 120000;

function abortError() { return new DOMException('Biber canceled', 'AbortError'); }
function checkSignal(signal) { if (signal?.aborted) throw abortError(); }

async function load(base, release, signal, onProgress) {
  const key = `${base}:${release.digest}`;
  if (!assets || assets.key !== key) {
    const promise = (async () => {
      const spec = release.engines?.biber;
      if (!spec) throw new Error('This release has no Biber WASM backend');
      // The manifest is the inventory for this release.  In particular,
      // biber.build.json is part of the published Biber receipt and must be
      // present and verified along with the executable bytes, even though the
      // worker only consumes the worker/glue/Wasm/data subset.
      const names = [...new Set(spec.files || [spec.worker, 'biber.js', 'biber.wasm', 'biber.data', 'biber.build.json'])];
      for (const required of [spec.worker, 'biber.js', 'biber.wasm', 'biber.data', 'biber.build.json']) {
        if (!names.includes(required)) throw new Error(`Incomplete Biber asset inventory: ${required}`);
      }
      const bytes = {};
      let done = 0;
      await Promise.all(names.map(async name => {
        const file = release.files?.[name];
        if (!file || !/^[a-f0-9]{64}$/.test(file.sha256)) throw new Error(`Unhashed Biber asset: ${name}`);
        const response = await fetchVerified(release, new URL(file.url, base).href, file);
        const data = await response.arrayBuffer();
        if (data.byteLength !== file.size) throw new Error(`Biber asset size mismatch: ${name}`);
        bytes[name] = data;
        onProgress?.({ done: ++done, total: names.length, scope: 'biber' });
      }));
      return {
        worker: new TextDecoder().decode(bytes[spec.worker]),
        glue: new TextDecoder().decode(bytes['biber.js']),
        wasm: await WebAssembly.compile(bytes['biber.wasm']),
        data: bytes['biber.data'],
      };
    })();
    assets = { key, promise };
    promise.catch(() => { if (assets?.promise === promise) assets = null; });
  }
  checkSignal(signal);
  // Downloads can complete into the verified cache even when this job is canceled.
  // A canceled caller must stop waiting immediately and must never start a worker.
  if (!signal) return assets.promise;
  return new Promise((resolve, reject) => {
    const aborted = () => reject(abortError());
    signal.addEventListener('abort', aborted, { once: true });
    assets.promise.then(resolve, reject).finally(() => signal.removeEventListener('abort', aborted));
  });
}

export async function runBiber(request, { base, release, signal, onProgress } = {}) {
  const runtime = await load(base, release, signal, onProgress);
  checkSignal(signal);
  const url = URL.createObjectURL(new Blob([runtime.worker], { type: 'text/javascript' }));
  let worker;
  try { worker = new Worker(url); } finally { URL.revokeObjectURL(url); }
  return new Promise((resolve, reject) => {
    let finished = false;
    const finish = (error, result) => {
      if (finished) return;
      finished = true;
      clearTimeout(timer);
      signal?.removeEventListener('abort', aborted);
      active.delete(stop);
      worker.terminate();
      error ? reject(error) : resolve(result);
    };
    const aborted = () => finish(abortError());
    const stop = () => finish(abortError());
    const timer = setTimeout(() => finish(new Error('Biber timed out')), TIMEOUT_MS);
    active.add(stop);
    signal?.addEventListener('abort', aborted, { once: true });
    worker.onerror = event => finish(new Error(event.message || 'Biber worker failed'));
    worker.onmessage = ({ data }) => {
      if (data.infrastructure) { finish(new Error(data.error)); return; }
      finish(null, { ...data, identity: request.identity,
        tool: { backend: 'browser', version: release.bibliography?.biber?.version } });
    };
    try { worker.postMessage({ ...runtime, stem: request.stem, main: request.main, bcf: request.bcf, files: request.files }); }
    catch (error) { finish(error); }
  });
}

export function cancel() { for (const stop of [...active]) stop(); }
