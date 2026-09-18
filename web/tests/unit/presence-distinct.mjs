import assert from "node:assert/strict";
import test from "node:test";
import { createProjectSession } from "../../src/lib/project-session.js";
import { PRESENCE_COLOURS, presenceColour } from "../../src/lib/presence-colour.js";

/// Three browsers in one file, each relaying its presence to the other two.
function room(names, colourFor) {
  const peers = [];
  for (const [name, tab] of names) {
    const session = createProjectSession({
      name,
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
    for (const peer of peers) await peer.session.start({});
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
    for (const peer of peers) await peer.session.start({});
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

test("signing in part way through recolours by the name, not the tab", async () => {
  const peers = room([["", "tab-a"], ["Grace", "tab-b"]], presenceColour);
  try {
    for (const peer of peers) await peer.session.start({});
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
