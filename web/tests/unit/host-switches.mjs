// What the server's host allows: agents and local builds, each on unless the
// last /api/config answer turns it off, and a server that says nothing allows both.
import assert from "node:assert/strict";

const store = new Map();
globalThis.localStorage = {
  getItem: (key) => (store.has(key) ? store.get(key) : null),
  setItem: (key, value) => store.set(key, String(value)),
  removeItem: (key) => store.delete(key),
};

// The module reads the kept answer when it loads, so it is imported only once
// the storage above is in place.
const { setHostSwitches, agentsAllowed, localBuildsAllowed } = await import("../../src/lib/host-switches.js");

// With nothing kept, both are allowed.
assert.equal(agentsAllowed(), true, "agents are allowed with nothing kept");
assert.equal(localBuildsAllowed(), true, "local builds are allowed with nothing kept");

// Turning agents off leaves local builds alone.
setHostSwitches({ agents: false });
assert.equal(agentsAllowed(), false, "agents are off");
assert.equal(localBuildsAllowed(), true, "and local builds stay on");

// Turning local builds off leaves agents alone.
setHostSwitches({ local_builds: false });
assert.equal(agentsAllowed(), true, "agents are on again");
assert.equal(localBuildsAllowed(), false, "local builds are off");

// An answer that says nothing, or no answer at all, allows both.
for (const answer of [null, {}]) {
  setHostSwitches({ agents: false, local_builds: false });
  setHostSwitches(answer);
  assert.equal(agentsAllowed(), true, `agents are allowed for ${JSON.stringify(answer)}`);
  assert.equal(localBuildsAllowed(), true, `local builds are allowed for ${JSON.stringify(answer)}`);
}

console.log("host-switches: agents and local builds each follow the last answer, and a silent server allows both");
