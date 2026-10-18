// Tracked changes, end to end, against a real `librepaper admin serve`: two
// clients share one document, one writes a tracked change as a plain Loro
// update, the other sees it, decides it, and the decision reaches the first.
//
// A tracked change is part of the document. Client A writes the new text and
// the change record with `trackedSplice` and commits, the way the editor's
// tracking plugin does (the layout is in docs/dev/protocol/track-changes.md).
// Client B reads it with `listChanges`, and accepts or rejects it with
// `accept` and `reject`.
// Both are ordinary document edits, so the decision travels through the same
// `doc-update` path as typing and needs no message of its own.
//
// The contract pinned here: B sees A's change; a reject by B puts A's original
// text back and removes the entry on both sides; an accept keeps the text and
// removes the entry; and a client that joins afterwards finds the decided
// state, not the pending one.
//
// Requires a built librepaper binary and LIBREPAPER_TEST_POSTGRES_URL. Only
// absent optional server/database configuration is skipped; setup errors fail.
import assert from "node:assert/strict";
import { createProjectSession } from "../../src/lib/project-session.js";
import { accept, listChanges, reject, trackedSplice } from "../../src/lib/track-changes.js";
import { startDeployment } from "../helpers/deployment.mjs";

const deployment = await startDeployment({ label: "tracked_changes" });
if (!deployment || deployment.unavailable) {
  console.log(`tracked-changes: ${deployment?.unavailable || "no librepaper binary; run cargo build"}; skipping`);
  process.exit(0);
}

const wait = async (what, check, timeout = 15000) => {
  const until = Date.now() + timeout;
  for (;;) {
    const found = await check();
    if (found) return found;
    if (Date.now() > until) throw new Error(`timed out waiting for ${what}`);
    await new Promise((done) => setTimeout(done, 50));
  }
};

/// A client the way the browser holds one: a socket with the session cookie,
/// a project session fed by the room's frames, and nothing else.
async function join(slug, name) {
  const url = `${deployment.base.replace("http", "ws")}/ws/${slug}`;
  const socket = new WebSocket(url, { headers: { cookie: deployment.cookie } });
  await new Promise((done, fail) => {
    socket.addEventListener("open", done, { once: true });
    socket.addEventListener("error", () => fail(new Error("the socket could not be opened")), { once: true });
  });
  const session = createProjectSession({
    send: (message) => socket.send(JSON.stringify(message)),
    presenceId: () => name,
    name,
    fetchReference: async (reference) => {
      const response = await fetch(new URL(reference, deployment.base), { headers: { cookie: deployment.cookie, "x-librepaper-client": "1" } });
      if (!response.ok) throw new Error(`the document state could not be fetched: ${response.status}`);
      return new Uint8Array(await response.arrayBuffer());
    },
  });
  const failures = [];
  socket.addEventListener("message", (event) => {
    const frame = JSON.parse(String(event.data));
    const done = (promise) => promise?.catch?.((error) => failures.push(error));
    if (frame.type === "hello") socket.send(JSON.stringify(session.open()));
    else if (frame.type === "doc-state") done(session.start(frame));
    else if (frame.type === "doc-rows") done(session.rows(frame));
    else if (frame.type === "doc-update") session.apply(frame.update);
    else if (frame.type === "doc-gap") session.gap(frame.vector);
    else if (frame.type === "doc-ack") session.acknowledge(frame.upTo || 0);
    else if (frame.type === "doc-durable") session.durable(frame.vector);
  });
  await wait(`${name} to join`, () => session.joined || failures[0] && Promise.reject(failures[0]));
  return {
    session,
    doc: session.doc,
    text: () => session.text_(),
    changes: () => listChanges(session.doc),
    close: () => { session.leave(); socket.close(); },
  };
}

const sourceOf = async (slug) => {
  const response = await fetch(`${deployment.base}/api/documents/${slug}/source`, { headers: { cookie: deployment.cookie } });
  assert.equal(response.status, 200);
  return (await response.json()).source;
};

/// What the editor's tracking plugin writes for one replacement: the new text
/// and the change record, in one commit.
function trackedReplace(client, from, original, inserted) {
  const fileId = client.session.mainId();
  const at = client.session.textOf(fileId).toString().indexOf(original);
  assert.notEqual(at, -1, `${original} is in the source`);
  const ids = trackedSplice(client.doc, fileId, at, at + original.length, inserted, { author: from.author, who: from.who }, { run: from.run ?? null });
  assert.equal(ids.length, 1, "one replacement is one change");
  client.doc.commit();
  return ids[0];
}

const clients = [];
const open = async (slug, name) => {
  const client = await join(slug, name);
  clients.push(client);
  return client;
};

try {
  const original = "# Decisions\n\nAlpha is first.\n\nBeta is second.\n\nGamma is third.\n";
  const { slug } = await deployment.publish({ title: "Tracked", source: original, source_format: "markdown" });
  const a = await open(slug, "ada");
  const b = await open(slug, "grace");
  assert.equal(a.text(), original);
  assert.equal(b.text(), original);
  assert.deepEqual(a.changes(), [], "a fresh document has no pending change");

  // Case 1: B rejects A's change. The original words come back for both, and
  // the entry is gone on both sides and for a client that joins afterwards.
  {
    const id = trackedReplace(a, { author: "Ada", who: "acct-ada", run: "pass-1" }, "Beta", "Bravo");
    await wait("B to see the change", () => b.changes().length === 1);
    const [seen] = b.changes();
    assert.equal(seen.id, id);
    assert.equal(seen.deleted, "Beta");
    assert.equal(seen.inserted, "Bravo");
    assert.equal(seen.author, "Ada");
    assert.equal(seen.who, "acct-ada");
    assert.equal(seen.run, "pass-1");
    assert.ok(b.text().includes("Bravo is second."), "the tracked text is in the document for everyone");
    assert.equal(reject(b.doc, id), true, "B rejects the change");
    assert.ok(b.text().includes("Beta is second.") && !b.text().includes("Bravo"), "the original words are back at once for B");
    assert.deepEqual(b.changes(), []);
    await wait("A to see the rejection", () => a.text().includes("Beta is second.") && a.changes().length === 0);
    assert.ok(!a.text().includes("Bravo"), "the tracked words are gone for A");
    await wait("the rejection to be saved", async () => (await sourceOf(slug)).includes("Beta is second."));
    const fresh = await open(slug, "fresh-after-reject");
    assert.equal(fresh.text(), original, "a client that joins later finds the restored text");
    assert.deepEqual(fresh.changes(), [], "and no pending change");
    console.log("tracked-changes: a change rejected by another client is undone for everyone");
  }

  // Case 2: B accepts A's change. The text stays and the entry is dropped.
  {
    const id = trackedReplace(a, { author: "Ada", who: "acct-ada" }, "Gamma", "Golf");
    await wait("B to see the second change", () => b.changes().length === 1);
    assert.equal(b.changes()[0].run, null, "a change with no run has none");
    assert.equal(accept(b.doc, id), true, "B accepts the change");
    assert.deepEqual(b.changes(), []);
    await wait("A to see the acceptance", () => a.changes().length === 0);
    assert.ok(a.text().includes("Golf is third."), "an accepted change keeps its text");
    await wait("the acceptance to be saved", async () => (await sourceOf(slug)).includes("Golf is third."));
    const fresh = await open(slug, "fresh-after-accept");
    assert.ok(fresh.text().includes("Golf is third."));
    assert.deepEqual(fresh.changes(), []);
    console.log("tracked-changes: a change accepted by another client keeps its text and leaves the queue");
  }

  // Case 3: a change nobody has decided survives a reconnect.
  {
    const id = trackedReplace(a, { author: "Ada", who: "acct-ada" }, "Alpha", "Alef");
    await wait("the change to be saved", async () => (await sourceOf(slug)).includes("Alef is first."));
    const fresh = await open(slug, "fresh-pending");
    await wait("the pending change to arrive", () => fresh.changes().length === 1);
    assert.equal(fresh.changes()[0].id, id);
    assert.equal(fresh.changes()[0].deleted, "Alpha");
    assert.equal(fresh.changes()[0].inserted, "Alef");
    console.log("tracked-changes: an undecided change is still pending for a client that joins later");
  }
} catch (error) {
  console.error(deployment.log);
  throw error;
} finally {
  for (const client of clients) {
    try { client.close(); } catch {}
  }
  await deployment.stop();
}
