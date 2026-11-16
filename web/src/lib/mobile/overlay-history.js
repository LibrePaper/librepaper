// On a phone, a sheet or a panel is a layer over the document, and the Back
// button should close the layer rather than leave the page. Each open level
// is a history entry of its own, pushed when it opens and popped when it
// closes. A popstate the app caused itself is ignored, so the app does not
// close a layer twice.

/// Returns { sync(depth), dispose() }. `depth` is how many overlay levels are open now. `onpop(depth)` is called when the user's Back button closed one.
export function createOverlayHistory({ onpop, win = window, settle = 50 }) {
  let wanted = 0;
  let pushed = 0;
  let own = 0;
  let timer = null;

  // Opening a sheet and closing another in one gesture moves the depth up and
  // down within a moment. Waiting for it to settle means only the final depth
  // touches history, so the two steps leave no entries behind.
  function sync(depth) {
    wanted = depth;
    clearTimeout(timer);
    timer = setTimeout(apply, settle);
  }

  function apply() {
    timer = null;
    if (wanted > pushed) {
      while (pushed < wanted) {
        pushed += 1;
        // No URL here: the address and its query string stay where they are.
        win.history.pushState({ lpOverlay: pushed }, "");
      }
    } else if (wanted < pushed) {
      const before = pushed;
      // The go() below fires one popstate that is ours, not the user's.
      own += 1;
      pushed = wanted;
      win.history.go(wanted - before);
    }
  }

  function onPopState() {
    if (own > 0) {
      own -= 1;
      return;
    }
    // With nothing pushed, this popstate is a real navigation and is left alone.
    if (pushed === 0) return;
    pushed -= 1;
    wanted = pushed;
    onpop(pushed);
  }

  win.addEventListener("popstate", onPopState);

  function dispose() {
    clearTimeout(timer);
    timer = null;
    win.removeEventListener("popstate", onPopState);
  }

  return { sync, dispose };
}
