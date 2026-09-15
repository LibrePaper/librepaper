// Serializes preview renders, coalesces bursts into the newest request, and
// owns the ordering used to reject results captured before navigation or a
// strict source change. The renderer itself remains injected so this state
// machine can be tested without mounting Reader.svelte.
export function createRenderCoordinator({ navigation, source, main, render }) {
  let issued = 0;
  let committed = 0;
  let running = false;
  let queued = false;

  async function execute(ticket) {
    running = true;
    try {
      await render(ticket);
    } finally {
      running = false;
      if (queued) {
        queued = false;
        void execute(issued);
      }
    }
  }

  function request() {
    issued += 1;
    if (running) {
      queued = true;
      return Promise.resolve();
    }
    return execute(issued);
  }

  return {
    request,
    invalidate() { issued += 1; },
    superseded({ ticket, capturedNavigation, capturedSource, strictSource, capturedMain }) {
      return ticket <= committed ||
        capturedNavigation !== navigation() ||
        (strictSource && capturedSource !== source()) ||
        capturedMain !== main();
    },
    commit(ticket) {
      if (ticket <= committed) return false;
      committed = ticket;
      return true;
    },
    isCommitted: (ticket) => ticket === committed,
    isNewer: (ticket) => ticket > committed,
    atLeastCommitted: (ticket) => ticket >= committed,
    get committed() { return committed; },
    get running() { return running; },
    get queued() { return queued; },
  };
}
