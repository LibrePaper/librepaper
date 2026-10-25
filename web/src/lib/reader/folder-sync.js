// Keeps the granted project folder in step with the project. Every change to
// the project calls `poke`. The run starts 800 ms after the last poke, at most
// one run is in flight, and a poke that arrives during a run schedules one
// more afterwards.
export const FOLDER_SYNC_PAUSE = 800;

// `run()` performs one sync and may throw. `ready()` says whether syncing
// applies now. `onError(error)` hears a failed run.
export function createFolderSync({ run, ready = () => true, onError = () => {}, pause = FOLDER_SYNC_PAUSE, timers = { set: setTimeout, clear: clearTimeout } }) {
  let timer = null;
  let running = false;
  let again = false;
  let disposed = false;

  async function start() {
    timer = null;
    if (disposed || !ready()) return;
    if (running) {
      again = true;
      return;
    }
    running = true;
    try {
      await run();
    } catch (error) {
      if (!disposed) onError(error);
    } finally {
      running = false;
    }
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

  function dispose() {
    disposed = true;
    if (timer !== null) timers.clear(timer);
    timer = null;
  }

  return { poke, dispose };
}
