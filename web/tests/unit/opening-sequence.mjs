// The opening sequence, driven without a page: the role, the room, the
// session frames and what Ctrl-S says. Every effect the page would have is a
// recording callback, so the order of the sequence can be asserted from inside
// the callbacks themselves.
import assert from "node:assert/strict";
import { loadRunes } from "../helpers/runes.mjs";

const { createOpening } = await loadRunes(new URL("../../src/lib/reader/opening.svelte.js", import.meta.url));

const tick = () => new Promise(setImmediate);

// A session that records what the room told it, and a collaboration that hands
// it out and keeps the options it was made with.
function fakeSession() {
  const calls = [];
  const session = {
    calls,
    start: async (event) => { calls.push(["start", event]); if (session.failWith) throw session.failWith; },
    rows: async (event) => { calls.push(["rows", event]); if (session.failWith) throw session.failWith; },
    gap: (vector) => calls.push(["gap", vector]),
    apply: (update) => calls.push(["apply", update]),
    applyPresence: (update) => calls.push(["presence", update]),
    acknowledge: (upTo) => calls.push(["ack", upTo]),
    durable: (vector) => calls.push(["durable", vector]),
  };
  return session;
}

function harness(overrides = {}) {
  const log = [];
  const made = [];
  const sessions = [];
  const lifecycle = createOpening({
    slug: "paper",
    key: "k",
    loadCollab: async () => { log.push("loadCollab"); return { fake: true }; },
    createCollaboration: (options) => {
      const collaboration = {
        options,
        closed: 0,
        close() { this.closed++; },
        start(document_) { log.push(["collaboration.start", document_.role]); const session = fakeSession(); sessions.push(session); return session; },
      };
      made.push(collaboration);
      return collaboration;
    },
    roleKnown: (document_) => log.push(["roleKnown", lifecycle.state.mayEdit, lifecycle.state.openedLocally, document_.role]),
    formatKnown: (format) => log.push(["formatKnown", lifecycle.state.mayEdit, format]),
    canRender: () => true,
    noRenderer: (format) => log.push(["noRenderer", format]),
    refreshProjection: async () => { log.push("refreshProjection"); },
    editorReady: (format) => log.push(["editorReady", lifecycle.state.mayEdit, lifecycle.state.settled, format]),
    ...overrides,
  });
  return { lifecycle, log, made, sessions };
}

// An editor's role is settled before anything that depends on it runs.
{
  const { lifecycle, log, made } = harness();
  await lifecycle.prepare({ role: "editor", source_format: "typst", local_copy: true });
  assert.deepEqual(log, [
    ["roleKnown", true, true, "editor"],
    ["formatKnown", true, "typst"],
    "loadCollab",
    ["collaboration.start", "editor"],
    ["editorReady", true, true, "typst"],
  ]);
  assert.equal(made[0].options.sourceSync, true);
  assert.equal(made[0].options.getCanEdit(), true);
  assert.equal(lifecycle.state.settled, true);
  assert.ok(lifecycle.session, "the session is kept");
  assert.equal(lifecycle.collaboration, made[0]);
}

// A reader never joins CRDT sync: no module, no format, and the projection is
// fetched once the room is open.
{
  const { lifecycle, log, made } = harness();
  await lifecycle.prepare({ role: "commenter", source_format: "latex" });
  assert.equal(lifecycle.state.mayEdit, false);
  assert.deepEqual(log, [
    ["roleKnown", false, false, "commenter"],
    ["collaboration.start", "commenter"],
    "refreshProjection",
  ]);
  assert.equal(made[0].options.sourceSync, false);
  assert.equal(made[0].options.collab, null);
}

// A format nothing can draw says so, settles, and never starts.
{
  const { lifecycle, log, made } = harness({ canRender: () => false });
  await lifecycle.prepare({ role: "owner", source_format: "typst" });
  assert.deepEqual(log, [["roleKnown", true, false, "owner"], ["formatKnown", true, "typst"], ["noRenderer", "typst"]]);
  assert.equal(lifecycle.state.settled, true);
  assert.equal(made.length, 0);
}

// An empty project has no format and so needs no renderer.
{
  const { lifecycle, log } = harness({ canRender: () => false });
  await lifecycle.prepare({ role: "editor" });
  assert.deepEqual(log.at(-1), ["editorReady", true, true, ""]);
  assert.ok(!log.some((entry) => entry[0] === "noRenderer"));
}

// Quarto is recognized from the execution engine when no source format is set.
{
  const { lifecycle, log } = harness();
  await lifecycle.prepare({ role: "editor", source_format: "", execution_engine: "quarto" });
  assert.deepEqual(log.find((entry) => entry[0] === "formatKnown"), ["formatKnown", true, "quarto"]);
  const other = harness();
  await other.lifecycle.prepare({ role: "editor", source_format: "", execution_engine: "other" });
  assert.deepEqual(other.log.find((entry) => entry[0] === "formatKnown"), ["formatKnown", true, ""]);
}

// A page torn down while the CRDT loaded does not create a collaboration.
{
  let gone = false;
  const { lifecycle, made } = harness({ disposed: () => gone, loadCollab: async () => { gone = true; return {}; } });
  await lifecycle.start({ role: "editor" });
  assert.equal(made.length, 0);
  assert.equal(lifecycle.session, null);
}

// Starting again closes the room before it.
{
  const { lifecycle, made } = harness();
  await lifecycle.start({ role: "editor" });
  await lifecycle.start({ role: "editor" });
  assert.equal(made[0].closed, 1);
  lifecycle.close();
  assert.equal(made[1].closed, 1);
}

// A standalone session is seeded with the session that was just made.
{
  const seeded = [];
  const { lifecycle } = harness({ seed: async (session) => { seeded.push(session); } });
  await lifecycle.start({ role: "editor" });
  assert.deepEqual(seeded, [lifecycle.session]);
}

// Connection, peers, persistence and session events follow into state first.
{
  const heard = [];
  const { lifecycle, made } = harness({
    onConnected: (up) => heard.push(["connected", up, lifecycle.state.connected]),
    onSession: (active) => heard.push(["session", active === lifecycle.session]),
    localStorageFailed: (error) => heard.push(["failed", error]),
  });
  await lifecycle.prepare({ role: "editor", source_format: "latex" });
  const { options } = made[0];
  options.onConnected(true);
  options.onConnected(false);
  assert.deepEqual(heard.slice(0, 2), [["connected", true, true], ["connected", false, false]]);
  options.onPeers(3);
  options.onPeers(2);
  assert.equal(lifecycle.state.peers, 3, "the count only grows through the callback");
  const next = { pending: 1, local: true, localPending: 0, localError: "quota", joined: true };
  options.onState(next);
  options.onState({ ...next, pending: 2 });
  options.onState({ ...next, localError: "other" });
  assert.deepEqual(heard.filter((entry) => entry[0] === "failed"), [["failed", "quota"], ["failed", "other"]]);
  assert.equal(lifecycle.state.persistence.localError, "other");
  const replacement = fakeSession();
  options.onSession(replacement);
  assert.equal(lifecycle.session, replacement);
  assert.deepEqual(heard.at(-1), ["session", true]);
}

// A disposed page ignores peers and persistence.
{
  let gone = false;
  const { lifecycle, made } = harness({ disposed: () => gone, localStorageFailed: () => assert.fail("disposed") });
  await lifecycle.start({ role: "editor" });
  gone = true;
  made[0].options.onPeers(9);
  made[0].options.onState({ pending: 0, local: false, localPending: 0, localError: "x", joined: false });
  assert.equal(lifecycle.state.peers, 1);
  assert.equal(lifecycle.state.persistence.localError, "");
}

// Session frames.
{
  const painted = [];
  const failed = [];
  const { lifecycle, sessions } = harness({ paint: () => painted.push(true), openFailed: (error) => failed.push(error.message) });
  assert.equal(lifecycle.receive({ type: "doc-update", update: "x" }), true, "no session is not a reason to refuse the frame");
  await lifecycle.start({ role: "editor" });
  const session = sessions[0];
  assert.equal(lifecycle.receive({ type: "doc-state", count: 4 }), true);
  assert.equal(lifecycle.state.peers, 4);
  await tick();
  assert.equal(painted.length, 1);
  assert.equal(lifecycle.receive({ type: "doc-rows" }), true);
  await tick();
  assert.equal(painted.length, 2);
  for (const [event, expected] of [
    [{ type: "doc-gap", vector: "v" }, ["gap", "v"]],
    [{ type: "doc-update", update: "u" }, ["apply", "u"]],
    [{ type: "doc-presence", update: "p" }, ["presence", "p"]],
    [{ type: "doc-ack", upTo: 7 }, ["ack", 7]],
    [{ type: "doc-ack" }, ["ack", 0]],
    [{ type: "doc-durable", vector: "d" }, ["durable", "d"]],
  ]) {
    assert.equal(lifecycle.receive(event), true, event.type);
    assert.deepEqual(session.calls.at(-1), expected);
  }
  assert.equal(lifecycle.receive({ type: "doc-peers", count: 2 }), true);
  assert.equal(lifecycle.state.peers, 2);
  assert.equal(lifecycle.receive({ type: "doc-peers" }), true);
  assert.equal(lifecycle.state.peers, 1);
  for (const type of ["source-changed", "published", "error", "hello", "chat"]) {
    assert.equal(lifecycle.receive({ type }), false, type);
  }

  // A join that finishes after the session was replaced paints nothing.
  const slow = harness({ paint: () => painted.push("late") });
  await slow.lifecycle.start({ role: "editor" });
  let release;
  slow.sessions[0].start = () => new Promise((resolve) => { release = resolve; });
  slow.lifecycle.receive({ type: "doc-state" });
  slow.made[0].options.onSession(fakeSession());
  release();
  await tick();
  assert.ok(!painted.includes("late"));

  // A rejected start is reported, not thrown.
  session.failWith = new Error("join refused");
  lifecycle.receive({ type: "doc-state" });
  lifecycle.receive({ type: "doc-rows" });
  await tick();
  assert.deepEqual(failed, ["join refused", "join refused"]);
}

// Ctrl-S names where the work is, and reserves "saved" for the server. This
// browser's own storage is evictable and is one device, so an offline Ctrl-S
// says what will happen to the typing rather than calling it saved.
{
  const { lifecycle } = harness();
  lifecycle.state.connected = true;
  lifecycle.state.persistence = { pending: 1, local: true, localPending: 0, localError: "", joined: true };
  assert.equal(lifecycle.report(), "", "work without durable coverage is not reported as saved");
  lifecycle.state.persistence = { pending: 0, local: true, localPending: 0, localError: "", joined: false };
  assert.equal(lifecycle.report(), "", "a connected socket is not a completed join");
  lifecycle.state.persistence.joined = true;
  assert.equal(lifecycle.report(), "Saved on the server.");
  lifecycle.state.connected = false;
  const offline = lifecycle.report();
  assert.match(offline, /^Offline. Everything typed so far has already reached the server./);
  lifecycle.state.persistence.pending = 2;
  const pending = lifecycle.report();
  assert.match(pending, /in this browser only/, "work the server has not seen is not called saved");
  assert.doesNotMatch(offline + pending, /saved/i, "the word is the server's");
  lifecycle.state.persistence.local = false;
  assert.equal(lifecycle.report(), "", "nothing is stored anywhere to talk about");
  // A local write that failed or is still in flight says so through its own failure.
  lifecycle.state.persistence = { pending: 0, local: true, localPending: 0, localError: "quota exceeded", joined: true };
  assert.equal(lifecycle.report(), "");
  lifecycle.state.persistence = { pending: 0, local: true, localPending: 1, localError: "", joined: true };
  assert.equal(lifecycle.report(), "");
}

// Closing the page is held only while work has reached neither the server nor
// this browser, and only for someone who can have such work.
{
  const { lifecycle } = harness();
  assert.equal(lifecycle.atRisk, false);
  lifecycle.state.persistence = { pending: 1, local: false, localPending: 0, localError: "", joined: false };
  assert.equal(lifecycle.atRisk, false, "a reader has no work to lose");
  lifecycle.state.mayEdit = true;
  assert.equal(lifecycle.atRisk, true);
  lifecycle.state.persistence.local = true;
  assert.equal(lifecycle.atRisk, false, "this browser has it");
  lifecycle.state.persistence.localError = "quota";
  assert.equal(lifecycle.atRisk, true, "a failed local write leaves it nowhere");
}

// The document has answered.
{
  const { lifecycle } = harness();
  assert.equal(lifecycle.state.settled, false);
  lifecycle.settle();
  assert.equal(lifecycle.state.settled, true);
}

console.log("opening-sequence: role order, room events, session frames, reports and risk passed");
