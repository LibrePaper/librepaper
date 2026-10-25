// Keeps a local project's folder in step with the project. Every change to
// the project calls `poke`. The run starts 800 ms after the last poke, at most
// one run is in flight, and a poke that arrives during a run schedules one
// more afterwards. `flush()` does not wait for the pause: it settles the
// folder at once, for a caller that is about to read it.
export const FOLDER_SYNC_PAUSE = 800;

// `run()` performs one sync and may throw. `ready()` says whether syncing
// applies now. `onError(error)` hears a failed run.
/** @param {{ run: () => Promise<void>, ready?: () => boolean, onError?: (error: any) => void, pause?: number, timers?: { set: (callback: () => void, ms: number) => unknown, clear: (handle: unknown) => void } }} options */
export function createFolderSync({ run, ready = () => true, onError = () => {}, pause = FOLDER_SYNC_PAUSE, timers = { set: (callback, ms) => setTimeout(callback, ms), clear: (handle) => clearTimeout(/** @type {ReturnType<typeof setTimeout>} */ (handle)) } }) {
  let timer = null;
  let running = false;
  let again = false;
  let disposed = false;
  // Whether the last run succeeded, and the run now in flight.
  let ok = true;
  let inflight = Promise.resolve();

  async function start() {
    timer = null;
    if (disposed || !ready()) return;
    if (running) {
      again = true;
      return;
    }
    running = true;
    const finished = (async () => {
      try {
        await run();
        ok = true;
      } catch (error) {
        ok = false;
        if (!disposed) onError(error);
      } finally {
        running = false;
      }
    })();
    inflight = finished;
    await finished;
    if (again && !disposed) {
      again = false;
      poke();
    }
  }

  function poke() {
    if (disposed) return;
    if (running) {
      again = true;
      return;
    }
    if (timer !== null) timers.clear(timer);
    timer = timers.set(start, pause);
  }

  /// Settles the folder now. A run that is waiting for the pause starts at
  /// once, a run in flight is waited for, and when nothing ran and the last
  /// run had failed, one is tried. Resolves with whether the last run
  /// succeeded, so the folder then holds what the editor holds.
  async function flush() {
    let ran = false;
    while (!disposed) {
      if (timer !== null) {
        ran = true;
        timers.clear(timer);
        await start();
      } else if (running) {
        ran = true;
        await inflight;
      } else if (!ok && !ran) {
        ran = true;
        await start();
      } else {
        break;
      }
    }
    return ok;
  }

  function dispose() {
    disposed = true;
    if (timer !== null) timers.clear(timer);
    timer = null;
  }

  return { poke, flush, dispose };
}
