// One subscription to the local app's status, shared by every component that
// shows it.
//
// The three components that watch this used to keep their own copy of the
// status and their own subscription. What is checked here is what replaced
// them: that the status a component reads is the last one the client
// published, that a second watcher does not open a second subscription, and
// that the subscription ends only when the last watcher lets go -- the client
// stops probing for a reconnect when nothing is listening, so releasing too
// eagerly would leave a running local app undiscovered.
import assert from "node:assert/strict";
import { loadRunes } from "../helpers/runes.mjs";

const { createCompanionStatus } = await loadRunes(
  new URL("../../src/lib/companion/status.svelte.js", import.meta.url),
);

function fakeClient() {
  const listeners = new Set();
  let status = { state: "unknown" };
  return {
    subscriptions: 0,
    status: () => status,
    subscribe(listener) {
      this.subscriptions += 1;
      listeners.add(listener);
      listener(status);
      return () => listeners.delete(listener);
    },
    get listening() { return listeners.size; },
    publish(next) {
      status = next;
      for (const listener of listeners) listener(status);
    },
  };
}

const client = fakeClient();
const companion = createCompanionStatus(client);

assert.equal(companion.status.state, "unknown", "the status reads before anyone watches");
assert.equal(client.listening, 0, "and watching is what opens the subscription");

const first = companion.watch();
const second = companion.watch();
assert.equal(client.subscriptions, 1, "two watchers share one subscription");

client.publish({ state: "connected", capabilities: { quarto: {} } });
assert.equal(companion.status.state, "connected");
assert.equal(companion.status.capabilities.quarto !== undefined, true);

first();
assert.equal(client.listening, 1, "one watcher leaving keeps the subscription");
first();
assert.equal(client.listening, 1, "and a repeated release of the same view changes nothing");

second();
assert.equal(client.listening, 0, "the last watcher out stops the client listening");

companion.watch();
assert.equal(client.subscriptions, 2, "watching again opens a fresh subscription");
assert.equal(companion.status.state, "connected", "which replays what the client last knew");
