import assert from "node:assert/strict";
import test from "node:test";
import { createProjectSession } from "../../src/lib/project-session.js";
import { PRESENCE_COLOURS, presenceColour } from "../../src/lib/presence-colour.js";

/// Three browsers in one file, each relaying its presence to the other two.
/// An entry may name an account as its third item, passed as `accountId`.
function room(names, colourFor) {
  const peers = [];
  for (const [name, tab, accountId] of names) {
    const session = createProjectSession({
      name,
      accountId,
      presenceId: () => tab,
      presenceColour: colourFor,
      send: (message) => {
        if (message.type !== "doc-presence") return;
        for (const other of peers) {
          if (other.tab !== tab) other.session.applyPresence(message.update);
        }
      },
      onState: () => {},
    });
    peers.push({ tab, name, session });
  }
  return peers;
}

const settle = () => new Promise((resume) => setTimeout(resume, 120));

test("everybody in a file sees everybody else", async () => {
  // The store is one key-value map shared by every peer. While all of them
  // announced themselves under `user`, each new arrival replaced the last:
  // this used to return nobody, from all three.
  const peers = room([["Ada", "tab-a"], ["Grace", "tab-b"], ["Alan", "tab-c"]], presenceColour);
  try {
    for (const peer of peers) await peer.session.start({ protocol: "librepaper.room.v3", vector: "", durableVector: "", updates: [] });
    await settle();
    for (const peer of peers) {
      const seen = peer.session.participants();
      assert.deepEqual(
        seen.map(({ name }) => name).sort(),
        peers.filter((other) => other !== peer).map(({ name }) => name).sort(),
        `${peer.name} should see the other two`,
      );
      assert.equal(peer.session.localPresence().name, peer.name, "and should still be themselves");
    }
  } finally {
    for (const peer of peers) peer.session.leave();
  }
});

test("two people who choose the same colour do not stay that way", async () => {
  // Everybody picks the same colour when they are alone, which is what two
  // names landing in the same bucket looks like from inside the palette. The
  // one whose key sorts higher gives way once it can see the other.
  const stubborn = (_who, taken = []) =>
    PRESENCE_COLOURS.find((colour) => !taken.includes(colour)) || PRESENCE_COLOURS[0];
  const peers = room([["Ada", "tab-a"], ["Grace", "tab-b"], ["Alan", "tab-c"]], stubborn);
  try {
    for (const peer of peers) {
      assert.equal(peer.session.localPresence().color, PRESENCE_COLOURS[0], "same colour to start");
    }
    for (const peer of peers) await peer.session.start({ protocol: "librepaper.room.v3", vector: "", durableVector: "", updates: [] });
    await settle();
    const mine = peers.map((peer) => peer.session.localPresence().color);
    assert.equal(new Set(mine).size, peers.length, "three people, three colours");
    // The first to sort keeps what everybody already saw them in.
    assert.equal(mine[0], PRESENCE_COLOURS[0]);
    for (const peer of peers) {
      const seen = peer.session.participants().map(({ colour }) => colour);
      assert.equal(new Set(seen).size, seen.length, `${peer.name} should see no two alike`);
    }
  } finally {
    for (const peer of peers) peer.session.leave();
  }
});

test("one signed-in account open in two tabs is one avatar to everyone else", async () => {
  // Ada has the file open in two tabs under one account. Grace sees her once;
  // Ada's first tab does not list her own second tab.
  const peers = room([["Ada", "tab-a", "u1"], ["Ada", "tab-b", "u1"], ["Grace", "tab-c", "u2"]], presenceColour);
  try {
    for (const peer of peers) await peer.session.start({ protocol: "librepaper.room.v3", vector: "", durableVector: "", updates: [] });
    await settle();
    const grace = peers[2].session.participants().map(({ name }) => name);
    assert.deepEqual(grace, ["Ada"], "Grace sees one Ada");
    const ada = peers[0].session.participants().map(({ name }) => name);
    assert.deepEqual(ada, ["Grace"], "Ada's other tab is not listed");
  } finally {
    for (const peer of peers) peer.session.leave();
  }
});

test("a signed-in person is coloured by their account, as the Authors tab colours them", () => {
  // Two spellings of one account's name are one colour: the caret follows the
  // account, the same seed the Authors tab draws the person's row from.
  const session = createProjectSession({
    name: "Ada",
    accountId: "acc-1",
    presenceId: () => "tab-a",
    presenceColour,
    send: () => {},
    onState: () => {},
  });
  try {
    assert.equal(session.localPresence().color, presenceColour("account:acc-1"));
    session.rename("Ada Lovelace", "acc-1");
    assert.equal(session.localPresence().color, presenceColour("account:acc-1"));
    // Signing out of the account falls back to the name, as an anonymous reader's.
    session.rename("Ada Lovelace", "");
    assert.equal(session.localPresence().color, presenceColour("name:Ada Lovelace"));
  } finally {
    session.leave();
  }
});

test("an anonymous reader with no name still wears their tab's colour", () => {
  const session = createProjectSession({
    presenceId: () => "tab-z",
    presenceColour,
    send: () => {},
    onState: () => {},
  });
  try {
    assert.equal(session.localPresence().color, presenceColour("tab-z"));
  } finally {
    session.leave();
  }
});

test("signing in part way through recolours by the name, not the tab", async () => {
  const peers = room([["", "tab-a"], ["Grace", "tab-b"]], presenceColour);
  try {
    for (const peer of peers) await peer.session.start({ protocol: "librepaper.room.v3", vector: "", durableVector: "", updates: [] });
    await settle();
    const before = peers[0].session.localPresence().color;
    assert.equal(before, presenceColour("tab-a"), "a reader with no name wears their tab's colour");
    peers[0].session.rename("Ada");
    await settle();
    const after = peers[0].session.localPresence();
    assert.equal(after.name, "Ada");
    assert.notEqual(after.color, peers[1].session.localPresence().color);
    assert.deepEqual(peers[1].session.participants().map(({ name }) => name), ["Ada"]);
  } finally {
    for (const peer of peers) peer.session.leave();
  }
});
