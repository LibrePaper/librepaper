import assert from "node:assert/strict";
import { loadRunes } from "../helpers/runes.mjs";

const { createBackupStatus } = await loadRunes(
  new URL("../../src/lib/companion/backups.svelte.js", import.meta.url),
);

let requests = [];
const bridge = {
  backups(accountId) {
    return new Promise((resolve, reject) => requests.push({ accountId, resolve, reject }));
  },
};
const state = createBackupStatus(bridge);

state.setScope("", false);
assert.equal(requests.length, 0, "signed-out pages do not ask the companion for backup status");
state.setScope("account-a", false);
assert.equal(requests.length, 0, "an unpaired companion is not probed for backup status");

state.setScope("account-a", true);
assert.equal(requests[0].accountId, "account-a");
state.setScope("account-b", true);
assert.equal(requests[1].accountId, "account-b");
requests[0].resolve({ enabled: true, destination_set: true, destination: "folder-a" });
await Promise.resolve();
assert.equal(state.status.accountId, "account-b", "a late response cannot restore the previous account scope");
assert.equal(state.status.data, null);
requests[1].resolve({ enabled: false, destination_set: true, destination: "folder-b" });
await Promise.resolve();
assert.equal(state.status.data.destination, "folder-b");

state.setScope("account-b", false);
assert.equal(state.status.data, null, "disconnecting clears account backup state");
state.reset();

// A running backup schedules one quicker follow-up read, then stops once it is done.
requests = [];
const bridge2 = {
  backups(accountId) {
    return new Promise((resolve, reject) => requests.push({ accountId, resolve, reject }));
  },
};
const fast = createBackupStatus(bridge2, { runningPollMs: 1 });
fast.setScope("account-a", true);
assert.equal(requests.length, 1);
requests[0].resolve({ running: true });
await new Promise((r) => setTimeout(r, 20));
assert.equal(requests.length, 2, "a running backup triggers one quicker follow-up read");
requests[1].resolve({ running: false });
await new Promise((r) => setTimeout(r, 20));
assert.equal(requests.length, 2, "a finished backup does not schedule another quick read");
fast.reset();
