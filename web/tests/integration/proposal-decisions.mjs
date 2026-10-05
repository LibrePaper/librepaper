// Deciding a suggestion, end to end, against a real `librepaper admin serve`:
// the WebSocket the browser uses, the durable rows behind it, and a fresh join
// afterwards.
//
// A suggestion is a comment of kind "suggestion" backed by a proposal. It is
// created the way the browser creates one (`annotations.comment` in
// web/src/lib/reader/annotations.svelte.js): a `comment` message with
// motivation "editing" and the proposed text. It is decided the way
// `decideSuggestion` in Reader.svelte decides it: a whole-suggestion
// `proposal-decide` message with `all: true`, the tip as `proposal-list` handed
// it out, and a UUID request_id.
//
// The contract pinned here: once a decision is acknowledged with
// `proposal-decided`, the proposal row and its suggestion comment are both
// gone, for an accept and for a reject, a resumed ID replays its final
// receipt, and a decision with a request_id that is not a UUID is refused and
// removes nothing.
//
// Requires a built librepaper binary and LIBREPAPER_TEST_POSTGRES_URL. Only
// absent optional server/database configuration is skipped; setup errors fail.
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { randomUUID } from "node:crypto";
import { LoroDoc, decodeFrontiers, encodeFrontiers } from "loro-crdt";
import { psqlCommand } from "../helpers/postgres-test.mjs";
import { startDeployment } from "../helpers/deployment.mjs";

const deployment = await startDeployment({ label: "proposal_decisions" });
if (!deployment || deployment.unavailable) {
  console.log(`proposal-decisions: ${deployment?.unavailable || "no librepaper binary; run cargo build"}; skipping`);
  process.exit(0);
}

function sql(text) {
  const { program, prefix } = psqlCommand();
  return execFileSync(program, [
    ...prefix, deployment.postgres.psqlUrl, "-v", "ON_ERROR_STOP=1", "-tA", "-c", text,
  ], { encoding: "utf8" }).trim();
}

const count = (table, column, id) => Number(sql(`SELECT count(*) FROM ${table} WHERE ${column} = '${id}'`));

/// A socket the way the browser holds one: the session cookie on the
/// handshake, every frame kept, and a way to wait for the one that answers.
async function connect(slug) {
  const url = `${deployment.base.replace("http", "ws")}/ws/${slug}`;
  const socket = new WebSocket(url, { headers: { cookie: deployment.cookie } });
  const frames = [];
  const watchers = new Set();
  socket.addEventListener("message", (event) => {
    const frame = JSON.parse(String(event.data));
    frames.push(frame);
    for (const watcher of [...watchers]) watcher(frame);
  });
  await new Promise((done, fail) => {
    socket.addEventListener("open", done, { once: true });
    socket.addEventListener("error", () => fail(new Error("the socket could not be opened")), { once: true });
  });
  /// The first frame, already received or still to come, that `match` accepts.
  const next = (what, match, timeout = 15000) => new Promise((done, fail) => {
    const found = frames.find(match);
    if (found) return done(found);
    const timer = setTimeout(() => { watchers.delete(watcher); fail(new Error(`timed out waiting for ${what}`)); }, timeout);
    const watcher = (frame) => {
      if (!match(frame)) return;
      clearTimeout(timer);
      watchers.delete(watcher);
      done(frame);
    };
    watchers.add(watcher);
  });
  const hello = await next("hello", (frame) => frame.type === "hello");
  return {
    hello,
    frames,
    next,
    send: (message) => socket.send(JSON.stringify(message)),
    close: () => socket.close(),
  };
}

const sourceOf = async (slug) => {
  const response = await fetch(`${deployment.base}/api/documents/${slug}/source`, { headers: { cookie: deployment.cookie } });
  assert.equal(response.status, 200);
  return (await response.json()).source;
};

/// What the editor sends for a suggestion: the passage with its surroundings,
/// worked out from the source as it is now, and the words to put in its place.
async function suggest(socket, slug, exact, proposed) {
  const source = await sourceOf(slug);
  const at = source.indexOf(exact);
  assert.notEqual(at, -1, `${exact} is in the source`);
  const body = `suggestion ${randomUUID()}`;
  socket.send({
    type: "comment",
    exact,
    prefix: source.slice(Math.max(0, at - 24), at),
    suffix: source.slice(at + exact.length, at + exact.length + 24),
    render_digest: "",
    motivation: "editing",
    body,
    proposed,
    temp_id: randomUUID(),
  });
  const made = await socket.next(`the suggestion ${exact}`, (frame) =>
    (frame.type === "comment" && frame.comment?.body === body)
    || (frame.type === "error" && /suggest/i.test(frame.message || "")));
  assert.equal(made.type, "comment", `the suggestion was refused: ${made.message}`);
  const comment = made.comment;
  assert.ok(comment.id, "the suggestion has an id");
  assert.ok(comment.proposal, "the suggestion is backed by a proposal");
  assert.equal(comment.proposed, proposed, "the created suggestion returns its replacement text");
  assert.equal(count("annotations", "id", comment.id), 1);
  assert.equal(count("document_proposals", "id", comment.proposal), 1);
  return comment;
}

/// The proposal snapshot exactly as the server sends it, which is what the
/// browser echoes back (`proposalTip` in Reader.svelte).
async function stateOf(socket, proposal) {
  const request_id = randomUUID();
  socket.send({ type: "proposal-list", request_id });
  const list = await socket.next("proposal-list", (frame) => frame.type === "proposal-list" && frame.request_id === request_id);
  const open = list.proposals.find((entry) => entry.id === proposal);
  assert.ok(open, "the proposal is listed as open");
  assert.ok(open.tip, "the listed proposal carries its tip");
  return open;
}

async function tipOf(socket, proposal) {
  return (await stateOf(socket, proposal)).tip;
}

/// Decides the whole suggestion with exactly the shape the browser sends.
async function decide(socket, comment, accepted, { request_id = randomUUID() } = {}) {
  const tip = await tipOf(socket, comment.proposal);
  socket.send({ type: "proposal-decide", proposal_id: comment.proposal, all: true, accepted, tip, request_id });
  return socket.next("the answer to the decision", (frame) =>
    (frame.type === "proposal-decided" || frame.type === "error") && frame.request_id === request_id);
}

/// A previously acknowledged id must replay a final result after reload,
/// including the original frontiers and decisions, without reopening a row.
async function replayOutcome(socket, proposal, prior, decisions) {
  socket.send({
    type: "proposal-open",
    request_id: proposal,
    base: prior.base,
    resume: true,
  });
  const answer = await socket.next("the resumed proposal outcome", (frame) =>
    frame.request_id === proposal && frame.type === "proposal-decided");
  assert.equal(answer.replay, true);
  assert.equal(answer.resolved_base, prior.base);
  assert.equal(answer.resolved_tip, prior.tip);
  assert.deepEqual(answer.decisions, decisions);
  assert.equal(count("document_proposals", "id", proposal), 0, "resume does not recreate the row");
  const request_id = randomUUID();
  socket.send({ type: "proposal-list", request_id });
  const list = await socket.next("the list after resume", (frame) =>
    frame.type === "proposal-list" && frame.request_id === request_id);
  assert.equal(list.proposals.some((entry) => entry.id === proposal), false);
}

/// Replaces the single-splice suggestion branch with a realistic legacy
/// branch containing two distant changes. Old clients could record one
/// decision per hunk, while current suggestion decisions must answer both.
async function installLegacyTwoHunkBranch(socket, proposal, prior) {
  const stateFrame = socket.next("the document state", (frame) =>
    frame.type === "doc-state" || frame.type === "doc-rows");
  socket.send({ type: "doc-open", protocol: "librepaper.room.v3", vector: "", request_id: randomUUID() });
  const first = await stateFrame;
  const state = first.type === "doc-state" ? first : null;
  const rows = first.type === "doc-rows"
    ? first
    : await socket.next("the document rows", (frame) => frame.type === "doc-rows");
  const doc = new LoroDoc();
  if (state?.base) {
    doc.import(Buffer.from(state.base, "base64"));
  } else if (state?.ref) {
    const snapshot = await fetch(new URL(state.ref, deployment.base), {
      headers: { cookie: deployment.cookie },
    });
    assert.equal(snapshot.status, 200, "the document snapshot reference is readable");
    doc.import(new Uint8Array(await snapshot.arrayBuffer()));
  }
  for (const update of [...(state?.updates || []), ...(rows.updates || [])]) {
    doc.import(Buffer.from(update, "base64"));
  }
  const base = decodeFrontiers(Buffer.from(prior.base, "base64"));
  const atBase = doc.forkAt(base);
  const branch = atBase.fork();
  branch.setPeerId(BigInt(Date.now()) + 1_000_000n);
  const paths = branch.getMap("paths");
  const fileId = [...paths.entries()].find(([, path]) => path === "main.md")?.[0];
  assert.ok(fileId, "the document state includes its main text file");
  const text = branch.getMap("files").get(fileId);
  const original = text.toString();
  const exact = "Alpha 0123456789abcdefghij Omega";
  const start = original.indexOf(exact);
  assert.notEqual(start, -1, "the proposal base contains the selected words");
  const finalWord = original.indexOf("Omega", start);
  text.delete(finalWord, "Omega".length);
  text.insert(finalWord, "Omicron");
  text.delete(start, "Alpha".length);
  text.insert(start, "Aleph");
  branch.commit();
  const tip = Buffer.from(encodeFrontiers(branch.frontiers())).toString("hex");
  const bytes = Buffer.from(branch.export({ mode: "update", from: atBase.oplogVersion() })).toString("hex");
  sql(`UPDATE document_proposals
    SET tip_frontiers=decode('${tip}','hex'), branch_bytes=decode('${bytes}','hex'), version=version+1
    WHERE id='${proposal}'`);
  branch.destroy?.();
  atBase.destroy?.();
  doc.destroy?.();
}

function assertGone(comment) {
  assert.equal(count("document_proposals", "id", comment.proposal), 0, "the proposal row is deleted");
  assert.equal(count("annotations", "proposal_id", comment.proposal), 0, "no annotation still points at the proposal");
  assert.equal(count("annotations", "id", comment.id), 0, "the suggestion comment is deleted");
}

/// A fresh join, the way a reloaded page is: neither the proposal nor the
/// suggestion comment comes back.
async function assertAbsentOnJoin(slug, ...comments) {
  const fresh = await connect(slug);
  try {
    const request_id = randomUUID();
    fresh.send({ type: "proposal-list", request_id });
    const list = await fresh.next("proposal-list", (frame) => frame.type === "proposal-list" && frame.request_id === request_id);
    for (const comment of comments) {
      assert.equal(list.proposals.some((entry) => entry.id === comment.proposal), false, "a fresh join does not list the proposal");
      assert.equal((fresh.hello.comments || []).some((entry) => entry.id === comment.id), false, "a fresh join does not list the suggestion");
    }
  } finally {
    fresh.close();
  }
}

let socket;
try {
  const { slug } = await deployment.publish({
    title: "Decisions",
    source: "# Decisions\n\nAlpha is first.\n\nBeta is second.\n\nGamma is third.\n\nDelta is fourth.\n\nEpsilon is fifth.\n\nZeta Alpha 0123456789abcdefghij Omega is long.\n",
    source_format: "markdown",
  });
  socket = await connect(slug);

  // Case 1: a rejected suggestion is deleted, and the source is untouched.
  {
    const comment = await suggest(socket, slug, "Alpha", "Aleph");
    const prior = await stateOf(socket, comment.proposal);
    const answer = await decide(socket, comment, false);
    assert.equal(answer.type, "proposal-decided", answer.message);
    assert.equal(answer.accepted, false);
    assertGone(comment);
    assert.ok((await sourceOf(slug)).includes("Alpha is first."), "a rejection leaves the source alone");
    await replayOutcome(socket, comment.proposal, prior, [{ hunk: 0, accepted: false }]);
    await assertAbsentOnJoin(slug, comment);
    console.log("proposal-decisions: a rejected suggestion is deleted");
  }

  // Case 2: an accepted suggestion is deleted too, and its text is in the source.
  {
    const comment = await suggest(socket, slug, "Beta", "Bravo");
    const prior = await stateOf(socket, comment.proposal);
    const answer = await decide(socket, comment, true);
    assert.equal(answer.type, "proposal-decided", answer.message);
    assert.equal(answer.accepted, true);
    assertGone(comment);
    const source = await sourceOf(slug);
    assert.ok(source.includes("Bravo is second."), "the accepted text is in the source");
    assert.ok(!source.includes("Beta is second."), "the replaced text is gone from the source");
    // A discard that arrives after a final decision may find no live row.
    // It must answer from the durable result instead of claiming a discard
    // with missing frontiers.
    const discardRequest = randomUUID();
    socket.send({ type: "proposal-discard", proposal_id: comment.proposal, request_id: discardRequest });
    const racedDiscard = await socket.next("the outcome for a late discard", (frame) =>
      ["proposal-decided", "proposal-discarded", "error"].includes(frame.type)
      && frame.request_id === discardRequest);
    assert.equal(racedDiscard.type, "proposal-decided", "the final decision wins over a late discard");
    assert.equal(racedDiscard.replay, true);
    assert.equal(racedDiscard.resolved_base, prior.base);
    assert.equal(racedDiscard.resolved_tip, prior.tip);
    assert.deepEqual(racedDiscard.decisions, [{ hunk: 0, accepted: true }]);
    await replayOutcome(socket, comment.proposal, prior, [{ hunk: 0, accepted: true }]);
    await assertAbsentOnJoin(slug, comment);
    console.log("proposal-decisions: an accepted suggestion is deleted and applied");
  }

  // Case 3: what "Reject all" does, one decision after the other.
  {
    const first = await suggest(socket, slug, "Gamma", "Golf");
    const second = await suggest(socket, slug, "Delta", "Dover");
    for (const comment of [first, second]) {
      const answer = await decide(socket, comment, false);
      assert.equal(answer.type, "proposal-decided", answer.message);
    }
    assertGone(first);
    assertGone(second);
    const source = await sourceOf(slug);
    assert.ok(source.includes("Gamma is third.") && source.includes("Delta is fourth."), "rejecting all leaves the source alone");
    await assertAbsentOnJoin(slug, first, second);
    console.log("proposal-decisions: rejecting two suggestions in a row deletes both");
  }

  // Case 4: a request_id that is not a UUID is refused and deletes nothing.
  {
    const comment = await suggest(socket, slug, "Epsilon", "Echo");
    const before = await sourceOf(slug);
    const answer = await decide(socket, comment, false, { request_id: "not-a-uuid" });
    assert.equal(answer.type, "error", "a decision without a UUID request_id is refused");
    assert.equal(count("document_proposals", "id", comment.proposal), 1, "the proposal survives the refusal");
    assert.equal(count("annotations", "id", comment.id), 1, "the suggestion survives the refusal");
    assert.equal(await sourceOf(slug), before, "a refused decision changes no source");
    // Still decidable afterwards, so the refusal left it open rather than broken.
    const settled = await decide(socket, comment, false);
    assert.equal(settled.type, "proposal-decided", settled.message);
    assertGone(comment);
    console.log("proposal-decisions: a decision without a UUID request_id is refused");
  }

  // Case 5: a lost-response retry resends the same request id after the
  // proposal is already gone. It is answered from the label the first
  // decision wrote, not refused, and changes nothing.
  {
    const comment = await suggest(socket, slug, "Delta", "Dover");
    const request_id = randomUUID();
    const tip = await tipOf(socket, comment.proposal);
    const decision = { type: "proposal-decide", proposal_id: comment.proposal, all: true, accepted: true, tip, request_id };
    for (const attempt of ["first", "retry"]) {
      socket.send(decision);
      const answer = await socket.next(`the ${attempt} answer`, (frame) =>
        (frame.type === "proposal-decided" || frame.type === "error") && frame.request_id === request_id);
      assert.equal(answer.type, "proposal-decided", `the ${attempt}: ${answer.message}`);
      assert.equal(answer.resolved, true);
    }
    assertGone(comment);
    const source = await sourceOf(slug);
    assert.equal(source.split("Dover is fourth.").length, 2, "the retry did not apply the change twice");
    console.log("proposal-decisions: a retried decision is answered, not refused");
  }

  // Case 6: a legacy partially reviewed, multi-hunk suggestion is still one
  // decision. all:true replaces mixed old rows and applies every hunk.
  {
    const exact = "Alpha 0123456789abcdefghij Omega";
    const comment = await suggest(socket, slug, exact, "Aleph 0123456789abcdefghij Omicron");
    const original = await stateOf(socket, comment.proposal);
    await installLegacyTwoHunkBranch(socket, comment.proposal, original);
    const prior = await stateOf(socket, comment.proposal);
    const tipBytes = Buffer.from(prior.tip, "base64").toString("hex");
    sql(`INSERT INTO document_proposal_hunks
      (proposal_id,hunk_index,accepted,decided_by,decided_against,note)
      VALUES ('${comment.proposal}',0,false,'legacy-reviewer',decode('${tipBytes}','hex'),NULL),
             ('${comment.proposal}',1,true,'legacy-reviewer',decode('${tipBytes}','hex'),NULL)`);
    const answer = await decide(socket, comment, true);
    assert.equal(answer.type, "proposal-decided", answer.message);
    assert.deepEqual(answer.decisions, [
      { hunk: 0, accepted: true },
      { hunk: 1, accepted: true },
    ], "the whole answer replaces the old mixed per-hunk choices");
    assertGone(comment);
    const source = await sourceOf(slug);
    assert.ok(source.includes("Zeta Aleph 0123456789abcdefghij Omicron is long."));
    await replayOutcome(socket, comment.proposal, prior, [
      { hunk: 0, accepted: true },
      { hunk: 1, accepted: true },
    ]);
    console.log("proposal-decisions: a legacy mixed multi-hunk suggestion resolves as one choice");
  }

  // Case 7: a syntactically valid but unknown id is not reported as a
  // successful discard with null frontiers.
  {
    const request_id = randomUUID();
    socket.send({ type: "proposal-discard", proposal_id: randomUUID(), request_id });
    const answer = await socket.next("unknown proposal discard status", (frame) =>
      frame.request_id === request_id && (frame.type === "error" || frame.type === "proposal-discarded"));
    assert.equal(answer.type, "error");
    assert.equal(answer.status_unknown, true);
    console.log("proposal-decisions: an unknown discard is reported as status unknown");
  }
} catch (error) {
  console.error(deployment.log);
  throw error;
} finally {
  try { socket?.close(); } catch {}
  await deployment.stop();
}
