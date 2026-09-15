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

const first = projectIdentity({ server: "https://paper.example/path", slug: "paper", createdAt: "one" });
const same = projectIdentity({ server: "https://paper.example", slug: "paper", createdAt: "one" });
const recreated = projectIdentity({ server: "https://paper.example", slug: "paper", createdAt: "two" });
assert.equal(sameProject(first, same), true);
assert.throws(() => assertSameProject(first, recreated), { code: "project-identity-changed" });

console.log("project-session: persistence, identity and acknowledgement boundaries passed");
