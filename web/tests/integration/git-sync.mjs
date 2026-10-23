// Git sync, end to end: the page's sync library (web/src/lib/gitsync) driving a
// real `librepaper admin serve` on one side and a real detached local service on
// the other, against real bare repositories. A second ordinary clone, driven
// with plain `git`, plays the other side. The harness lives in
// ../helpers/git-sync-harness.mjs; this file holds the scenarios. Each scenario
// publishes its own document and seeds its own repository.
// Requires a built librepaper binary, git 2.38 or newer and
// LIBREPAPER_TEST_POSTGRES_URL.
import assert from "node:assert/strict";
import { rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { test } from "node:test";
import { applyResolution, compare, connect, outgoing, pull, push } from "../../src/lib/gitsync/sync.js";
import { TEST, git, limit, linked, privateRepository, replaceIn, sorted, textAt, wait, withFixture } from "../helpers/git-sync-harness.mjs";

// ---------------------------------------------------------------- 1. first connection

const INTRO_REMOTE = "# Intro\n\nFrom the repository.\n";
const INTRO_LOCAL = "# Intro\n\nFrom LibrePaper.\n";
const BIB = "@book{a, title={A}}\n";
const NOTES = "Notes kept in LibrePaper.\n";

const firstConnection = () => ({
  remote: { "paper/intro.qmd": INTRO_REMOTE, "paper/refs.bib": BIB, "README.md": "# Repository\n" },
  project: { "intro.qmd": INTRO_LOCAL, "notes.md": NOTES },
  folder: "paper",
});

test("1a. first connection keeping the repository's copies", TEST, () => withFixture(firstConnection(), async (fx) => {
  const head = fx.repo.headOf();
  const looked = await limit("compare", compare(fx.ctx, fx.draft));
  assert.deepEqual(looked.refusals, []);
  assert.deepEqual(looked.incoming, ["refs.bib"], JSON.stringify(looked));
  assert.deepEqual(looked.outgoing, ["notes.md"], JSON.stringify(looked));
  assert.deepEqual(looked.differing, ["intro.qmd"], JSON.stringify(looked));
  assert.equal(await fx.link(), null, "comparing records nothing");

  const result = await limit("connect", connect(fx.ctx, fx.draft, "remote"));
  assert.equal(result.status, "pulled", JSON.stringify(result));
  await wait("the repository's files", () => textAt(fx.client, "intro.qmd") === INTRO_REMOTE && textAt(fx.client, "refs.bib") === BIB);
  assert.equal(textAt(fx.client, "notes.md"), NOTES, "the file only LibrePaper has stays");
  const link = await fx.link();
  assert.equal(link.checkpoint, head);
  assert.deepEqual(sorted(link.synced_paths), ["intro.qmd", "refs.bib"]);
  assert.equal(link.folder, "paper");
  assert.equal(fx.repo.headOf(), head, "connecting in this direction pushes nothing");
}));

test("1b. first connection keeping LibrePaper's copies", TEST, () => withFixture(firstConnection(), async (fx) => {
  const head = fx.repo.headOf();
  const result = await limit("connect", connect(fx.ctx, fx.draft, "local"));
  assert.equal(result.status, "pushed", JSON.stringify(result));
  await wait("the repository's bibliography", () => textAt(fx.client, "refs.bib") === BIB);
  assert.equal(textAt(fx.client, "intro.qmd"), INTRO_LOCAL, "LibrePaper's text is kept");

  const pushed = fx.repo.headOf();
  assert.notEqual(pushed, head);
  assert.deepEqual(fx.repo.parents(pushed), [head], "the push sits on the original head");
  assert.equal(fx.repo.read("paper/intro.qmd"), INTRO_LOCAL);
  assert.equal(fx.repo.read("paper/notes.md"), NOTES);
  assert.equal(fx.repo.read("paper/refs.bib"), BIB);
  assert.equal(fx.repo.read("README.md"), "# Repository\n", "files outside the folder are untouched");
  assert.equal((await fx.link()).checkpoint, pushed);
}));

test("1c. first connection to an empty branch pushes the project as the first commit", TEST, () => withFixture({
  project: { "intro.qmd": INTRO_LOCAL, "notes.md": NOTES },
  folder: "paper",
}, async (fx) => {
  assert.equal(fx.repo.headOf(), null, "the branch has no commits");
  const result = await limit("connect", connect(fx.ctx, fx.draft, "local"));
  assert.equal(result.status, "pushed", JSON.stringify(result));
  const first = fx.repo.headOf();
  assert.ok(first, "the branch now has a commit");
  assert.deepEqual(fx.repo.parents(first), [], "it is the first commit");
  assert.equal(fx.repo.read("paper/intro.qmd"), INTRO_LOCAL);
  assert.equal(fx.repo.read("paper/notes.md"), NOTES);
  const link = await fx.link();
  assert.equal(link.checkpoint, first);
  assert.deepEqual(sorted(link.synced_paths), ["intro.qmd", "notes.md"]);
}));

// ---------------------------------------------------------------- 2 and 3. merging

const TWO_PARAGRAPHS = "# Intro\n\nFirst paragraph.\n\nSecond paragraph.\n";

test("2. independent edits merge, and reach the repository only on a push", TEST, () => withFixture({
  remote: { "intro.qmd": TWO_PARAGRAPHS },
  project: { "intro.qmd": TWO_PARAGRAPHS },
}, async (fx) => {
  await linked(fx);
  await replaceIn(fx.client, "intro.qmd", "First paragraph.", "First paragraph, edited here.");
  const remoteEdit = "# Intro\n\nFirst paragraph.\n\nSecond paragraph, edited there.\n";
  fx.repo.commit({ "intro.qmd": remoteEdit });

  const pulled = await limit("pull", pull(fx.ctx));
  assert.equal(pulled.status, "pulled", JSON.stringify(pulled));
  const both = "# Intro\n\nFirst paragraph, edited here.\n\nSecond paragraph, edited there.\n";
  await wait("both edits", () => textAt(fx.client, "intro.qmd") === both);
  assert.equal(fx.repo.read("intro.qmd"), remoteEdit, "the repository has only its own edit until a push");
  assert.equal((await fx.link()).checkpoint, fx.repo.headOf());

  const pushed = await limit("push", push(fx.ctx, "Both edits"));
  assert.equal(pushed.status, "pushed", JSON.stringify(pushed));
  assert.equal(fx.repo.read("intro.qmd"), both);
  assert.equal(textAt(fx.client, "intro.qmd"), both);
}));

test("3. overlapping edits stop for a resolution and the resolved text is pushed", TEST, () => withFixture({
  remote: { "intro.qmd": "# Intro\n\nThe shared line.\n" },
  project: { "intro.qmd": "# Intro\n\nThe shared line.\n" },
}, async (fx) => {
  await linked(fx);
  const mine = "# Intro\n\nThe shared line, local.\n";
  await replaceIn(fx.client, "intro.qmd", "The shared line.", "The shared line, local.");
  fx.repo.commit({ "intro.qmd": "# Intro\n\nThe shared line, remote.\n" });

  const conflicted = await limit("pull", pull(fx.ctx));
  assert.equal(conflicted.status, "conflict", JSON.stringify(conflicted));
  assert.deepEqual(conflicted.conflicts.map((item) => item.path), ["intro.qmd"]);
  assert.equal(textAt(fx.client, "intro.qmd"), mine, "a conflicting pull changes nothing");

  const resolved = "# Intro\n\nThe shared line, both.\n";
  const applied = await limit("apply", applyResolution(fx.ctx, conflicted.session, { "intro.qmd": { text: resolved } }));
  assert.equal(applied.status, "pulled", JSON.stringify(applied));
  await wait("the resolved text", () => textAt(fx.client, "intro.qmd") === resolved);
  assert.equal((await fx.link()).checkpoint, fx.repo.headOf());

  const pushed = await limit("push", push(fx.ctx, "Resolved"));
  assert.equal(pushed.status, "pushed", JSON.stringify(pushed));
  assert.equal(fx.repo.read("intro.qmd"), resolved);
}));

// ---------------------------------------------------------------- 4. typing during a pull

test("4. typing in a touched file during a pull is refused once and kept", TEST, () => withFixture({
  remote: { "intro.qmd": "# Intro\n\nOne.\n\nTwo.\n" },
  project: { "intro.qmd": "# Intro\n\nOne.\n\nTwo.\n" },
}, async (fx) => {
  await linked(fx);
  fx.repo.commit({ "intro.qmd": "# Intro\n\nOne.\n\nTwo, remote.\n" });

  // The word is typed after the pull planned its patches and before the
  // request is sent, then the request goes through unchanged.
  const real = fx.ctx.api.sourceChange;
  const seen = [];
  const ctx = {
    ...fx.ctx,
    api: {
      ...fx.ctx.api,
      async sourceChange(body) {
        if (!seen.length) await replaceIn(fx.client, "intro.qmd", "One.", "One typed.");
        try {
          const answer = await real(body);
          seen.push({ status: 200 });
          return answer;
        } catch (error) {
          seen.push({ status: error.status, reason: error.body?.reason, files: error.body?.files });
          throw error;
        }
      },
    },
  };
  const pulled = await limit("pull", pull(ctx));
  assert.equal(pulled.status, "pulled", JSON.stringify(pulled));
  assert.ok(seen.length >= 2, `the pull retried: ${JSON.stringify(seen)}`);
  assert.equal(seen[0].status, 409, "the first request is refused");
  assert.equal(seen[0].reason, "changed", JSON.stringify(seen[0]));
  assert.equal(seen.at(-1).status, 200);
  const expected = "# Intro\n\nOne typed.\n\nTwo, remote.\n";
  await wait("the typed word and the pulled edit", () => textAt(fx.client, "intro.qmd") === expected);
}));

// ---------------------------------------------------------------- 5. rules change

test("5. a path the rules newly ignore stays where it is and is not pushed", TEST, () => withFixture({
  remote: { "intro.qmd": "# Intro\n\nHello.\n", "data/a.csv": "x,y\n1,2\n" },
  project: { "intro.qmd": "# Intro\n\nHello.\n", "data/a.csv": "x,y\n1,2\n" },
}, async (fx) => {
  const first = await linked(fx);
  assert.ok(first.synced_paths.includes("data/a.csv"), "the csv is synced before the rules change");

  fx.repo.commit({ ".librepaperignore": "data/\n" });
  const pulled = await limit("pull", pull(fx.ctx));
  assert.ok(["pulled", "nothing"].includes(pulled.status), JSON.stringify(pulled));
  assert.equal(textAt(fx.client, "data/a.csv"), "x,y\n1,2\n", "the project keeps the file");
  assert.equal(fx.repo.read("data/a.csv"), "x,y\n1,2\n", "the repository keeps the file");
  const link = await fx.link();
  assert.ok(!link.synced_paths.includes("data/a.csv"), "it leaves the synced paths");
  assert.ok(link.synced_paths.includes("intro.qmd"));

  await replaceIn(fx.client, "data/a.csv", "1,2", "1,2\n3,4");
  await replaceIn(fx.client, "intro.qmd", "Hello.", "Hello again.");
  const pushed = await limit("push", push(fx.ctx, "Edit the intro"));
  assert.equal(pushed.status, "pushed", JSON.stringify(pushed));
  assert.deepEqual(pushed.files.map((file) => file.path), ["intro.qmd"]);
  assert.equal(fx.repo.read("intro.qmd"), "# Intro\n\nHello again.\n");
  assert.equal(fx.repo.read("data/a.csv"), "x,y\n1,2\n", "the repository's copy is untouched");
  assert.equal(textAt(fx.client, "data/a.csv"), "x,y\n1,2\n3,4\n", "LibrePaper's edit stays in the project");
}));

// ---------------------------------------------------------------- 6. CRLF

test("6. a CRLF file keeps its endings until LibrePaper edits it", TEST, () => {
  const crlf = Buffer.from("@book{a,\r\n  title={A}\r\n}\r\n");
  return withFixture({
    remote: { "intro.qmd": "# Intro\n\nHello.\n", "refs.bib": crlf },
    project: { "intro.qmd": "# Intro\n\nHello.\n" },
  }, async (fx) => {
    const result = await limit("connect", connect(fx.ctx, fx.draft, "remote"));
    assert.equal(result.status, "pulled", JSON.stringify(result));
    await wait("the bibliography", () => textAt(fx.client, "refs.bib") === "@book{a,\n  title={A}\n}\n");

    await replaceIn(fx.client, "intro.qmd", "Hello.", "Hello there.");
    const first = await limit("push", push(fx.ctx, "Edit the intro"));
    assert.equal(first.status, "pushed", JSON.stringify(first));
    assert.deepEqual(first.files.map((file) => file.path), ["intro.qmd"]);
    assert.equal(fx.repo.read("intro.qmd"), "# Intro\n\nHello there.\n");
    assert.ok(fx.repo.show("refs.bib").equals(crlf), "the untouched file keeps its CRLF bytes");

    await replaceIn(fx.client, "refs.bib", "title={A}", "title={B}");
    const second = await limit("push", push(fx.ctx, "Edit the bibliography"));
    assert.equal(second.status, "pushed", JSON.stringify(second));
    const written = fx.repo.show("refs.bib");
    assert.ok(!written.includes(13), "an edited file is written with LF");
    assert.equal(written.toString("utf8"), "@book{a,\n  title={B}\n}\n");
  });
});

// ---------------------------------------------------------------- 7. rewritten history

test("7. rewritten history without the checkpoint asks to connect again", TEST, () => withFixture({
  remote: { "intro.qmd": "# Intro\n\nHello.\n" },
  project: { "intro.qmd": "# Intro\n\nHello.\n" },
}, async (fx) => {
  const link = await linked(fx);

  // A force push of an unrelated root commit, then the old objects are gone.
  const { clone, bare } = fx.repo;
  git(clone, "checkout", "--quiet", "--orphan", "rewritten");
  git(clone, "rm", "-rf", "--quiet", ".");
  writeFileSync(join(clone, "intro.qmd"), "# Intro\n\nA different history.\n");
  git(clone, "add", "--all");
  git(clone, "commit", "--quiet", "-m", "Rewritten");
  git(clone, "push", "--quiet", "--force", "origin", "rewritten:main");
  git(bare, "reflog", "expire", "--expire=now", "--all");
  git(bare, "gc", "--quiet", "--prune=now");
  assert.notEqual(fx.repo.headOf(), link.checkpoint);

  // This desktop loses its copy of the checkpoint.
  rmSync(privateRepository(link.connection_id), { recursive: true, force: true });
  await fx.git.connect({ connection_id: link.connection_id, remote: fx.draft.remote, branch: "main" });

  const refused = await limit("pull", pull(fx.ctx));
  assert.equal(refused.status, "refused", JSON.stringify(refused));
  assert.equal(refused.rebaseline, true, JSON.stringify(refused));
  assert.equal(textAt(fx.client, "intro.qmd"), "# Intro\n\nHello.\n", "nothing was guessed");
  assert.equal((await fx.link()).checkpoint, link.checkpoint);
}));

// ---------------------------------------------------------------- 8. interrupted push

test("8. a push whose record was never written is adopted by the next pull", TEST, () => withFixture({
  remote: { "intro.qmd": "# Intro\n\nHello.\n" },
  project: { "intro.qmd": "# Intro\n\nHello.\n" },
}, async (fx) => {
  const link = await linked(fx);
  const mine = "# Intro\n\nPushed by a desktop that died.\n";
  await replaceIn(fx.client, "intro.qmd", "Hello.", "Pushed by a desktop that died.");

  // The Git half of a push, by hand, and no record: the desktop died between
  // moving the branch and writing the link.
  const planned = await limit("outgoing", outgoing(fx.ctx));
  assert.equal(planned.status, "outgoing", JSON.stringify(planned));
  const plan = planned.outgoing;
  const tree = await fx.git.treeEdit(plan.remoteTree, plan.edits);
  const commit = await fx.git.commitTree(tree, [plan.remote], "Dead desktop", { name: "Tester", email: "tester@example.invalid" });
  await fx.git.push(commit, plan.remote, link.connection_id);
  assert.equal(fx.repo.headOf(), commit);
  assert.equal((await fx.link()).checkpoint, link.checkpoint, "the server never heard of the push");

  const second = await fx.another();
  assert.equal(textAt(second.client, "intro.qmd"), mine, "the second page sees the live text");
  const adopted = await limit("pull", pull(second.ctx));
  assert.ok(["pulled", "nothing"].includes(adopted.status), JSON.stringify(adopted));
  assert.equal((adopted.files || []).length, 0, "nothing in the project changes");
  assert.equal((await fx.link()).checkpoint, commit);
  assert.equal(textAt(fx.client, "intro.qmd"), mine);
  assert.equal(textAt(second.client, "intro.qmd"), mine);
}));

// ---------------------------------------------------------------- 9. push moved

test("9. a branch that moves after the review answers behind, then a pull and push land both", TEST, () => withFixture({
  remote: { "intro.qmd": TWO_PARAGRAPHS },
  project: { "intro.qmd": TWO_PARAGRAPHS },
}, async (fx) => {
  await linked(fx);
  await replaceIn(fx.client, "intro.qmd", "First paragraph.", "First paragraph, edited here.");
  const reviewed = await limit("outgoing", outgoing(fx.ctx));
  assert.equal(reviewed.status, "outgoing", JSON.stringify(reviewed));

  const theirs = fx.repo.commit({ "intro.qmd": "# Intro\n\nFirst paragraph.\n\nSecond paragraph, edited there.\n" });
  const behind = await limit("push", push(fx.ctx, "Reviewed", { confirmed: reviewed.outgoing }));
  assert.equal(behind.status, "behind", JSON.stringify(behind));
  assert.equal(fx.repo.headOf(), theirs, "nothing was pushed");

  const both = "# Intro\n\nFirst paragraph, edited here.\n\nSecond paragraph, edited there.\n";
  const pulled = await limit("pull", pull(fx.ctx));
  assert.equal(pulled.status, "pulled", JSON.stringify(pulled));
  await wait("the pulled edit", () => textAt(fx.client, "intro.qmd") === both);
  const landed = await limit("push", push(fx.ctx, "Both"));
  assert.equal(landed.status, "pushed", JSON.stringify(landed));
  assert.equal(fx.repo.read("intro.qmd"), both);
  assert.deepEqual(fx.repo.parents(fx.repo.headOf()), [theirs]);
}));

// ---------------------------------------------------------------- 10. comment anchors

// Anchoring a comment on a passage needs the editor's anchor records in the
// document, which only the browser's comment machinery writes. The HTTP route
// takes a place the page computed, so there is no honest way to make one here.
test("10. a comment anchor on text the pull leaves alone is kept", { skip: "creating an anchored comment needs the browser's anchor machinery" }, () => {});
