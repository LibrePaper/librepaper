// The agent chat sessions open for one project in this browser: their order,
// their numbers, and which one is showing. Each session keeps its transcript
// under its own key (see agent-client.svelte.js); this list only says which
// keys exist. Every transform returns a new object and leaves its input alone.

import { AGENT_SESSION_PREFIX } from "./storage.js";
import { localDataForgotten } from "./signed-out.js";

// The server keeps at most this many live chat channels per project.
export const MAX_AGENT_SESSIONS = 16;

export function listKey(origin, slug) {
  return `${AGENT_SESSION_PREFIX}list:${origin}:${slug}`;
}

export function initialSessions() {
  return { sessions: [{ key: "", number: 1 }], active: "" };
}

// A short id for a new session. The fallback exists for browsers that predate
// randomUUID; it only has to be unlikely to collide within one project.
export function newSessionKey() {
  if (globalThis.crypto?.randomUUID) return globalThis.crypto.randomUUID().replace(/-/g, "").slice(0, 12);
  const alphabet = "abcdefghijklmnopqrstuvwxyz0123456789";
  return Array.from({ length: 12 }, () => alphabet[Math.floor(Math.random() * alphabet.length)]).join("");
}

function validSessions(value) {
  if (!Array.isArray(value) || !value.length) return null;
  const keys = new Set();
  const sessions = [];
  for (const entry of value.slice(0, MAX_AGENT_SESSIONS)) {
    if (!entry || typeof entry !== "object") return null;
    if (typeof entry.key !== "string" || keys.has(entry.key)) return null;
    if (!Number.isInteger(entry.number) || entry.number < 1) return null;
    keys.add(entry.key);
    sessions.push({ key: entry.key, number: entry.number });
  }
  return sessions;
}

// Anything unreadable falls back to a fresh list, so a damaged entry never
// keeps the project from opening.
export function loadSessions(storage, origin, slug) {
  try {
    const value = JSON.parse(storage?.getItem(listKey(origin, slug)) || "null");
    const sessions = validSessions(value?.sessions);
    if (!sessions) return initialSessions();
    const active = sessions.some((s) => s.key === value.active) ? value.active : sessions[0].key;
    return { sessions, active };
  } catch {
    return initialSessions();
  }
}

export function saveSessions(storage, origin, slug, state) {
  // After a sign-out the list must not be written back to disk.
  if (localDataForgotten()) return;
  try {
    storage?.setItem(listKey(origin, slug), JSON.stringify(state));
  } catch {
    /* private browsing: the list still works for this page */
  }
}

export function addSession(state, key = newSessionKey()) {
  if (state.sessions.length >= MAX_AGENT_SESSIONS) return state;
  const number = Math.max(0, ...state.sessions.map((s) => s.number)) + 1;
  return { sessions: [...state.sessions, { key, number }], active: key };
}

export function removeSession(state, key) {
  const index = state.sessions.findIndex((s) => s.key === key);
  if (index < 0) return state;
  const sessions = state.sessions.filter((s) => s.key !== key);
  if (!sessions.length) {
    const fresh = newSessionKey();
    return { sessions: [{ key: fresh, number: 1 }], active: fresh };
  }
  if (state.active !== key) return { sessions, active: state.active };
  // The entry that slid into the removed slot takes over, so the reader's eye
  // stays where it was; at the end of the list the previous entry does.
  const next = sessions[Math.min(index, sessions.length - 1)];
  return { sessions, active: next.key };
}

export function selectSession(state, key) {
  return state.sessions.some((s) => s.key === key) ? { ...state, active: key } : state;
}
