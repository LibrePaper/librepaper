// The local app's status, as one piece of reactive state for the whole page.
//
// `client.js` is a plain module on purpose: Node tests import it directly, so
// it cannot hold runes. Every component that wants to *watch* the status
// therefore declared its own `$state` and copied each probe result into it --
// Reader, BuildSettings and LocalAppSettings all carried the same two lines,
// which is three chances for the copy to drift and three subscriptions where
// one will do.
//
// This module owns that state instead (the rule every module in lib/reader
// follows). A component reads `companion.status` and declares its interest
// with `$effect(() => companion.watch())`.
import * as defaultClient from "./client.js";

/// The client is injected so the watch bookkeeping can be driven without a
/// real local app; the page uses the singleton below.
export function createCompanionStatus(client = defaultClient) {
  // Replaced wholesale by the client on every probe, never edited in place.
  let current = $state.raw(client.status());

  // The client only auto-reconnects while somebody is listening, so the one
  // subscription is held for exactly as long as one component is watching.
  let release = null;
  let watchers = 0;

  return {
    get status() { return current; },

    /// Register a live view of the status. Call it from an `$effect`; the
    /// returned function drops the view, and the last one out stops the
    /// client's reconnect timer.
    watch() {
      watchers += 1;
      if (!release) release = client.subscribe((next) => { current = next; });
      let held = true;
      return () => {
        // An effect may clean up more than once. Only the first release of
        // this view counts, or the subscription ends while others still hold.
        if (!held) return;
        held = false;
        watchers -= 1;
        if (watchers > 0) return;
        release?.();
        release = null;
      };
    },
  };
}

export const companion = createCompanionStatus();
