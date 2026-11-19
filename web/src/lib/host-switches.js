// What this server's host allows, from the last `/api/config` answer. A host
// turns either off under `[access]` in librepaper.toml: `agents` for every
// agent, `local_builds` for building documents and running their code on this
// computer through the desktop app. The server refuses agents itself; this is
// what lets the page stop offering what would be refused. A server that says
// nothing, as one from before the switches does, allows both.
import { CONFIG, read } from "./storage.js";

const from = (config) => ({ agents: config?.agents !== false, localBuilds: config?.local_builds !== false });

// The answer kept from the last visit stands in until this visit's arrives.
let switches = from(read(CONFIG, null));

/** Takes a `/api/config` answer. Called by `api.js` whenever it has one. */
export function setHostSwitches(config) {
  switches = from(config);
  return switches;
}

export const agentsAllowed = () => switches.agents;
export const localBuildsAllowed = () => switches.localBuilds;
