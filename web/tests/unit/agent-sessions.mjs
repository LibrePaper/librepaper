import { test } from "node:test";
import assert from "node:assert/strict";
import {
  MAX_AGENT_SESSIONS, addSession, initialSessions, listKey, loadSessions,
  newSessionKey, removeSession, saveSessions, selectSession,
} from "../../src/lib/agent-sessions.js";
import { markSignedOut } from "../../src/lib/signed-out.js";

const storage = {
  values: new Map(),
  getItem(key) { return this.values.get(key) ?? null; },
  setItem(key, value) { this.values.set(key, value); },
};

const keys = (state) => state.sessions.map((s) => s.key);

test("a missing or damaged list loads as the initial session", () => {
  const empty = { getItem: () => null };
  assert.deepEqual(loadSessions(empty, "https://a.example", "paper"), initialSessions());
  const bad = [
    "not json",
    JSON.stringify({ sessions: [], active: "" }),
    JSON.stringify({ sessions: [{ key: "a", number: 0 }], active: "a" }),
    JSON.stringify({ sessions: [{ key: "a", number: 1 }, { key: "a", number: 2 }], active: "a" }),
    JSON.stringify({ sessions: [{ key: 3, number: 1 }], active: "3" }),
    JSON.stringify({ sessions: ["a"], active: "a" }),
  ];
  for (const raw of bad) {
    const one = { getItem: () => raw };
    assert.deepEqual(loadSessions(one, "https://a.example", "paper"), initialSessions(), raw);
  }
});

test("an unknown active key falls back to the first session", () => {
  const raw = JSON.stringify({ sessions: [{ key: "a", number: 1 }, { key: "b", number: 2 }], active: "zzz" });
  assert.deepEqual(loadSessions({ getItem: () => raw }, "o", "s"), { sessions: [{ key: "a", number: 1 }, { key: "b", number: 2 }], active: "a" });
});

test("a saved list round trips through storage", () => {
  const state = { sessions: [{ key: "a", number: 1 }, { key: "b", number: 4 }], active: "b" };
  saveSessions(storage, "https://a.example", "paper", state);
  assert.equal(storage.values.get(listKey("https://a.example", "paper")), JSON.stringify(state));
  assert.deepEqual(loadSessions(storage, "https://a.example", "paper"), state);
  // Another project's list is separate.
  assert.deepEqual(loadSessions(storage, "https://a.example", "other"), initialSessions());
});

test("the list key sits under the shared agent session prefix", () => {
  assert.equal(listKey("https://a.example", "paper"), "librepaper.agent.session.list:https://a.example:paper");
});

test("loading drops entries beyond the server limit", () => {
  const many = Array.from({ length: MAX_AGENT_SESSIONS + 3 }, (_, i) => ({ key: `k${i}`, number: i + 1 }));
  const loaded = loadSessions({ getItem: () => JSON.stringify({ sessions: many, active: "k0" }) }, "o", "s");
  assert.equal(loaded.sessions.length, MAX_AGENT_SESSIONS);
  assert.equal(loaded.sessions.at(-1).key, `k${MAX_AGENT_SESSIONS - 1}`);
});

test("add appends with the next number and activates it", () => {
  const start = { sessions: [{ key: "a", number: 1 }, { key: "b", number: 5 }], active: "a" };
  const next = addSession(start, "c");
  assert.deepEqual(next, { sessions: [{ key: "a", number: 1 }, { key: "b", number: 5 }, { key: "c", number: 6 }], active: "c" });
  assert.deepEqual(start, { sessions: [{ key: "a", number: 1 }, { key: "b", number: 5 }], active: "a" });
});

test("add generates a key when none is given", () => {
  const next = addSession(initialSessions());
  assert.equal(next.sessions.length, 2);
  assert.equal(next.active, next.sessions[1].key);
  assert.notEqual(next.active, "");
});

test("add refuses at the server limit", () => {
  let state = initialSessions();
  while (state.sessions.length < MAX_AGENT_SESSIONS) state = addSession(state);
  assert.equal(state.sessions.length, MAX_AGENT_SESSIONS);
  assert.equal(addSession(state, "overflow"), state);
});

test("removing an active middle entry activates the one that slid into its place", () => {
  const state = { sessions: [{ key: "a", number: 1 }, { key: "b", number: 2 }, { key: "c", number: 3 }], active: "b" };
  assert.deepEqual(removeSession(state, "b"), { sessions: [{ key: "a", number: 1 }, { key: "c", number: 3 }], active: "c" });
});

test("removing an active last entry activates the previous one", () => {
  const state = { sessions: [{ key: "a", number: 1 }, { key: "b", number: 2 }], active: "b" };
  assert.deepEqual(removeSession(state, "b"), { sessions: [{ key: "a", number: 1 }], active: "a" });
});

test("removing an inactive entry keeps the active one", () => {
  const state = { sessions: [{ key: "a", number: 1 }, { key: "b", number: 2 }, { key: "c", number: 3 }], active: "c" };
  assert.deepEqual(removeSession(state, "a"), { sessions: [{ key: "b", number: 2 }, { key: "c", number: 3 }], active: "c" });
});

test("removing the only entry leaves a fresh session", () => {
  const state = initialSessions();
  const next = removeSession(state, "");
  assert.equal(next.sessions.length, 1);
  assert.equal(next.sessions[0].number, 1);
  assert.notEqual(next.sessions[0].key, "");
  assert.equal(next.active, next.sessions[0].key);
});

test("removing an unknown key changes nothing", () => {
  const state = initialSessions();
  assert.equal(removeSession(state, "missing"), state);
});

test("select changes the active key only when it exists", () => {
  const state = { sessions: [{ key: "a", number: 1 }, { key: "b", number: 2 }], active: "a" };
  assert.deepEqual(selectSession(state, "b"), { sessions: state.sessions, active: "b" });
  assert.equal(selectSession(state, "unknown"), state);
  assert.equal(state.active, "a");
});

test("new session keys are short and distinct", () => {
  const first = newSessionKey();
  const second = newSessionKey();
  assert.match(first, /^[0-9a-z]{12}$/);
  assert.notEqual(first, second);
  assert.deepEqual(keys(addSession(initialSessions(), "x")), ["", "x"]);
});

test("saving is skipped after sign-out", () => {
  let writes = 0;
  const counting = { getItem: () => null, setItem() { writes += 1; } };
  saveSessions(counting, "o", "s", initialSessions());
  assert.equal(writes, 1);
  markSignedOut();
  saveSessions(counting, "o", "s", initialSessions());
  assert.equal(writes, 1);
});
