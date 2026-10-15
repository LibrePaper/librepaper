// Where to get the desktop app. Every place that asks for it links here.
export const INSTALL_URL = "https://librepaper.org/local-app.html";

// How the desktop app's connection state is worded: the pill text, its tone, a
// short hint and the full instructions. The notice, the settings footer and
// the status message share it.
export const STATES = {
  browser: {
    says: "Browser",
    tone: "neutral",
    hint: "Computing, agents, folders and backups need the LibrePaper desktop app.",
    instructions: "Computing, agents, folders and backups need the LibrePaper desktop app.",
  },
  checking: { says: "Checking", tone: "neutral", instructions: "Checking the LibrePaper desktop app." },
  ready: { says: "Running", tone: "good", instructions: "The LibrePaper desktop app is ready." },
  unavailable: {
    says: "Not running",
    tone: "error",
    hint: "The desktop app could not start its local service. Retry, or restart the app.",
    instructions: "The LibrePaper desktop app could not reach its local service. Retry, or restart the desktop app.",
  },
  incompatible: {
    says: "Update needed",
    tone: "warn",
    hint: "This desktop app is too old for this site. Install the latest version.",
    instructions: "This version of the LibrePaper desktop app is too old for this site. Install the latest version.",
  },
};

// The full instructions for a state.
export function instructionsFor(state) {
  return STATES[state]?.instructions || "";
}

// The wording for a status, with the version after "Running" when it is known.
export function stateOf(status) {
  const state = STATES[status?.state] || STATES.browser;
  const says = status?.state === "ready" && status?.version ? `${state.says} · ${status.version}` : state.says;
  return { says, tone: state.tone, hint: state.hint || "" };
}
