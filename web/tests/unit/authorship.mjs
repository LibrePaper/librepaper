// Who wrote which part of the document, read back from real loro-crdt docs.
//
// Two peers write to one text and sync. The runs must come back merged, with
// UTF-16 bounds that count an emoji as two units, and the summary must group
// one person's peers together, put the unknown last, and draw nothing for
// text nobody can name.
import assert from "node:assert/strict";
import test from "node:test";
import { LoroDoc } from "loro-crdt";
import { authorshipRuns, markSpans, summarize } from "../../src/lib/authorship.js";
import { personSeed, presenceColour } from "../../src/lib/presence-colour.js";

// A document whose changes carry timestamps, as project-session.js makes them.
function peerDoc(peer) {
  const doc = new LoroDoc();
  doc.setPeerId(peer);
  doc.setRecordTimestamp(true);
  return doc;
}

// Peer 1 writes "ab😀", peer 2 adds "XY" after it, then peer 1 adds "Z" at the
// front. The emoji is a surrogate pair, so the text is seven UTF-16 units.
function twoAuthors() {
  const one = peerDoc("1");
  one.getText("t").insert(0, "ab😀");
  one.commit();
  const two = peerDoc("2");
  two.import(one.export({ mode: "snapshot" }));
  two.getText("t").insert(4, "XY");
  two.commit();
  one.import(two.export({ mode: "snapshot" }));
  one.getText("t").insert(0, "Z");
  one.commit();
  return one;
}

test("runs cover the text by peer, with UTF-16 bounds", () => {
  const doc = twoAuthors();
  const text = doc.getText("t");
  assert.equal(text.toString(), "Zab😀XY");
  const runs = authorshipRuns(doc, text);
  assert.deepEqual(
    runs.map(({ from, to, peer }) => ({ from, to, peer })),
    [
      { from: 0, to: 5, peer: "1" },
      { from: 5, to: 7, peer: "2" },
    ],
  );
  // Each run carries the latest change timestamp it holds, in seconds.
  for (const run of runs) assert.ok(run.at > 0);
});

test("adjacent characters of one peer merge into one run", () => {
  const doc = peerDoc("7");
  doc.getText("t").insert(0, "hello");
  doc.commit();
  const runs = authorshipRuns(doc, doc.getText("t"));
  assert.equal(runs.length, 1);
  assert.deepEqual([runs[0].from, runs[0].to, runs[0].peer], [0, 5, "7"]);
});

test("an empty text has no runs", () => {
  const doc = peerDoc("1");
  assert.deepEqual(authorshipRuns(doc, doc.getText("t")), []);
});

test("summarize groups two peers of one account into one row, unknown last", () => {
  const runs = [
    { from: 0, to: 3, peer: "1", at: 1000 },
    { from: 3, to: 5, peer: "2", at: 2000 },
    { from: 5, to: 9, peer: "3", at: 1500 },
    { from: 9, to: 10, peer: "9", at: 0 },
  ];
  const firstSeen = new Date("2026-01-01T00:00:00Z");
  const authors = new Map([
    ["1", { name: "Ada", accountId: "acc-1", agent: false, firstSeen }],
    ["3", { name: "Ada", accountId: "acc-1", agent: false, firstSeen: new Date("2026-02-01T00:00:00Z") }],
    ["2", { name: "Bob", accountId: null, agent: false, firstSeen: new Date("2026-03-01T00:00:00Z") }],
  ]);
  const { rows, since, missing } = summarize(runs, authors);

  assert.equal(rows.length, 3);
  const [ada, bob, unknown] = rows;
  assert.equal(ada.key, "account:acc-1");
  assert.equal(ada.name, "Ada");
  assert.equal(ada.chars, 7);
  assert.ok(Math.abs(ada.share - 0.7) < 1e-9);
  // The latest of Ada's two peers' changes, 1500 seconds, as a Date.
  assert.equal(ada.last.getTime(), 1500 * 1000);
  // Each row is coloured from the seed the person's live cursor uses: the
  // account for Ada, the name for Bob. A later row steps past a colour an
  // earlier row already wears.
  assert.equal(ada.colour, presenceColour("account:acc-1"));

  assert.equal(bob.key, "name:Bob");
  assert.equal(bob.chars, 2);
  assert.equal(bob.colour, presenceColour("name:Bob", [ada.colour]));
  assert.notEqual(bob.colour, ada.colour);

  assert.equal(unknown.key, "unknown");
  assert.equal(unknown.unknown, true);
  assert.equal(unknown.colour, "");
  assert.equal(unknown.last, null);

  assert.equal(since.getTime(), firstSeen.getTime());
  // Peer "9" is not in the map, so the caller should fetch the list again.
  assert.equal(missing, true);
});

test("an agent sharing its owner's account gets its own row, in either order", () => {
  const firstSeen = new Date("2026-01-01T00:00:00Z");
  const human = { name: "Ada", accountId: "acc-1", agent: false, firstSeen };
  const agent = { name: "Build bot", accountId: "acc-1", agent: true, firstSeen };
  const authors = new Map([
    ["1", human],
    ["2", agent],
  ]);
  const humanRun = { from: 0, to: 3, peer: "1", at: 1000 };
  const agentRun = { from: 3, to: 5, peer: "2", at: 2000 };

  for (const runs of [
    [agentRun, humanRun],
    [humanRun, agentRun],
  ]) {
    const { rows, missing } = summarize(runs, authors);
    assert.equal(rows.length, 2);
    assert.equal(missing, false);
    const [first, second] = rows;
    // The larger row comes first: Ada with 3 characters, then the agent with 2.
    assert.equal(first.key, "account:acc-1");
    assert.equal(first.name, "Ada");
    assert.equal(first.agent, false);
    assert.equal(first.chars, 3);
    assert.equal(second.key, "agent:acc-1:Build bot");
    assert.match(second.name, /Build bot/);
    assert.equal(second.agent, true);
    assert.equal(second.chars, 2);
    assert.equal(first.colour, presenceColour("account:acc-1"));
    assert.equal(second.colour, presenceColour("agent:acc-1:Build bot", [first.colour]));
    assert.notEqual(first.colour, second.colour);
  }
});

test("a person's row colour is the colour their live cursor takes from the same seed", () => {
  // The Authors tab and the caret both go through personSeed, so a signed-in
  // person named differently in two places is still one colour.
  const firstSeen = new Date("2026-01-01T00:00:00Z");
  const authors = new Map([
    ["1", { name: "Ada L.", accountId: "acc-1", agent: false, firstSeen }],
  ]);
  const { rows } = summarize([{ from: 0, to: 4, peer: "1", at: 1000 }], authors);
  assert.equal(rows[0].colour, presenceColour(personSeed({ accountId: "acc-1", name: "Ada Lovelace" })));
});

test("markSpans draws every run but the unknown ones", () => {
  const authors = new Map([["1", { name: "Ada", accountId: "acc-1", agent: false, firstSeen: new Date(0) }]]);
  const runs = [
    { from: 0, to: 2, peer: "", at: 0 },
    { from: 2, to: 4, peer: "1", at: 1000 },
    { from: 4, to: 5, peer: "", at: 0 },
  ];
  const { rows } = summarize(runs, authors);
  const spans = markSpans(runs, authors, rows);
  assert.equal(spans.length, 1);
  assert.equal(spans[0].from, 2);
  assert.equal(spans[0].to, 4);
  assert.equal(spans[0].key, "account:acc-1");
  assert.equal(spans[0].colour, rows[0].colour);
  assert.ok(spans[0].title.length > 0);
});

test("an emoji written by one peer is one run of that peer", () => {
  const one = peerDoc("1");
  one.getText("t").insert(0, "ab");
  one.commit();
  const two = peerDoc("2");
  two.import(one.export({ mode: "snapshot" }));
  two.getText("t").insert(1, "😀");
  two.commit();
  const runs = authorshipRuns(two, two.getText("t"));
  assert.deepEqual(
    runs.map(({ from, to, peer }) => ({ from, to, peer })),
    [
      { from: 0, to: 1, peer: "1" },
      { from: 1, to: 3, peer: "2" },
      { from: 3, to: 4, peer: "1" },
    ],
  );
});
