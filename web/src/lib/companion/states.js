// Where to get the companion. Every place that asks for it links here.
export const INSTALL_URL = "https://librepaper.org/local-app.html";

// How the companion's connection state is worded: the pill text, its tone, a
// short hint and the full instructions. The connection controls, the settings
// footer and the status message share it.
export const STATES = {
  connected: { says: "Running", tone: "good", instructions: "LibrePaper Companion is connected." },
  unreachable: {
    says: "Not running",
    tone: "error",
    hint: "Install the companion, or start it if it is installed.",
    instructions: "LibrePaper Companion is not running on this computer (nothing answered at {address}). The companion is a separate program you install once. If it is installed, start it, then try again.",
  },
  denied: {
    says: "Blocked",
    tone: "error",
    hint: "Allow local network access for this site in the browser's site settings.",
    instructions: "Your browser blocked this site from reaching LibrePaper Companion on this computer. Allow local network access for this site in the browser's site settings, then try again.",
  },
  asking: {
    says: "Not reachable",
    tone: "warn",
    hint: "Install or start the companion. If it is running, allow local network access for this site when the browser asks.",
    instructions: "LibrePaper could not reach the companion on this computer. If it is not installed or not running, install or start it. If it is running, allow local network access for this site when the browser asks, or in the browser's site settings.",
  },
  reachable: {
    says: "Needs approval",
    tone: "warn",
    hint: "Connect, then approve the dialog the companion shows.",
    instructions: "LibrePaper Companion responded but could not be verified yet. Connect again.",
  },
  unauthorized: {
    says: "Needs approval",
    tone: "warn",
    hint: "Connect, then approve the dialog the companion shows.",
    instructions: "LibrePaper Companion is running but has not allowed this site yet. Click Connect and approve the dialog the companion shows.",
  },
  incompatible: {
    says: "Update needed",
    tone: "warn",
    hint: "This companion is too old for this site. Install the latest version.",
    instructions: "This version of LibrePaper Companion is too old for this site. Install the latest version, then connect.",
  },
  unknown: { says: "Not checked", tone: "neutral" },
};

// The full instructions for a state. The address is filled into {address}.
export function instructionsFor(state, address) {
  const text = STATES[state]?.instructions;
  return text ? text.replaceAll("{address}", address) : "";
}

// The wording for a status, with the version after "Running" when it is known.
export function stateOf(status) {
  const state = STATES[status?.state] || STATES.unknown;
  const says = status?.state === "connected" && status?.version ? `${state.says} · ${status.version}` : state.says;
  return { says, tone: state.tone, hint: state.hint || "" };
}
