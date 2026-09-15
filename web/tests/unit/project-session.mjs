import assert from "node:assert/strict";
import { createProjectSession } from "../../src/lib/project-session.js";
import { assertSameProject, projectIdentity, sameProject } from "../../src/lib/project-identity.js";

const states = [];
let persistenceEvents;
let closed = false;
const sent = [];
const session = createProjectSession({
  send: (message) => sent.push(message),
  onState: (state) => states.push(state),
  presenceId: () => "test-presence",
  persistence: {
    open(_doc, events) {
      persistenceEvents = events;
      return { close: () => { closed = true; } };
    },
  },
});

try {
  assert.equal(states.at(-1).local, false);
  persistenceEvents.hydrated();
  assert.equal(states.at(-1).local, true);
  persistenceEvents.writing(2);
  assert.equal(states.at(-1).localPending, 2);
  persistenceEvents.persisted();
  assert.equal(states.at(-1).localPending, 0);

  session.addText("paper.md", "offline");
  assert.equal(states.at(-1).pending, 1);
  assert.equal(sent.at(-1).type, "doc-update");
  session.acknowledge(sent.at(-1).seq);
  assert.equal(states.at(-1).pending, 0);

  persistenceEvents.failed(new Error("quota full"));
  assert.equal(states.at(-1).localError, "quota full");
  assert.equal(states.at(-1).local, false);
} finally {
  session.leave();
}
assert.equal(closed, true);

// A swap is the main file becoming a different file. Editing the main file is
// not one: everything hanging off `onSwap` rebuilds itself, so a swap per
// keystroke used to throw the editor -- and the caret with it -- back to the
// top of the file on every commit.
const swapping = createProjectSession({ send: () => {}, onState: () => {} });
try {
  const main = swapping.addText("paper.md", "hello");
  swapping.setMain(main);
  swapping.doc.commit();
  await new Promise((resume) => setTimeout(resume, 20));
  let swaps = 0;
  swapping.onSwap(() => { swaps += 1; });
  swapping.textOf(main).insert(5, " world");
  swapping.doc.commit();
  await new Promise((resume) => setTimeout(resume, 20));
  assert.equal(swaps, 0);
  assert.equal(swapping.text.toString(), "hello world");

  const other = swapping.addText("other.md", "x");
  swapping.setMain(other);
  swapping.doc.commit();
  await new Promise((resume) => setTimeout(resume, 20));
  assert.equal(swaps, 1);
  assert.equal(swapping.text.toString(), "x");
} finally {
  swapping.leave();
}

const first = projectIdentity({ server: "https://paper.example/path", slug: "paper", createdAt: "one" });
const same = projectIdentity({ server: "https://paper.example", slug: "paper", createdAt: "one" });
const recreated = projectIdentity({ server: "https://paper.example", slug: "paper", createdAt: "two" });
assert.equal(sameProject(first, same), true);
assert.throws(() => assertSameProject(first, recreated), { code: "project-identity-changed" });

console.log("project-session: persistence, identity, swap and acknowledgement boundaries passed");
