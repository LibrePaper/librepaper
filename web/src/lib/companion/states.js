// Where to get the companion. Every place that asks for it links here.
export const INSTALL_URL = "https://librepaper.org/local-app.html";

// How the companion's connection state is worded: the pill text, its tone and
// what to do next. The connection controls and the settings footer share it.
export const STATES = {
  connected: { says: "Running", tone: "good" },
  unreachable: { says: "Not running", tone: "error", hint: "Install the companion, or run librepaper in a terminal if it is installed." },
  denied: { says: "Blocked", tone: "error", hint: "Allow local network access for this site in the browser's site settings." },
  asking: { says: "Not reachable", tone: "warn", hint: "Install and start the companion. If it is running, allow local network access for this site when the browser asks." },
  reachable: { says: "Needs approval", tone: "warn", hint: "Connect, then approve the dialog the companion shows." },
  unauthorized: { says: "Needs approval", tone: "warn", hint: "Connect, then approve the dialog the companion shows." },
  incompatible: { says: "Update needed", tone: "warn", hint: "This companion is too old for this site. Install the latest version." },
  unknown: { says: "Not checked", tone: "neutral" },
};

// The wording for a status, with the version after "Running" when it is known.
export function stateOf(status) {
  const state = STATES[status?.state] || STATES.unknown;
  const says = status?.state === "connected" && status?.version ? `${state.says} · ${status.version}` : state.says;
  return { says, tone: state.tone, hint: state.hint || "" };
}
