// A remote agent's path, end to end: an owner-issued agent token handed to a
// real `librepaper mcp --document` on stdio, talking to a real deployment.
// The token is the bridge's only credential, so each role shows exactly what
// that token can do, and the same token is refused on every other route.
// Requires a built librepaper binary and LIBREPAPER_TEST_POSTGRES_URL.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { mkdtemp, mkdir, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createInterface } from "node:readline";
import { LoroDoc } from "loro-crdt";
import { listChanges } from "../../src/lib/track-changes.js";
import { deploymentBinary, startDeployment } from "../helpers/deployment.mjs";

const deployment = await startDeployment({ label: "agent_token" });
if (!deployment || deployment.unavailable) {
  console.log(`agent-token: ${deployment?.unavailable || "no librepaper binary; run cargo build"}; skipping`);
  process.exit(0);
}

const bridges = [];
let home;

// A bridge with a fresh home and no login of any kind, so the token is the
// only credential it can possibly use.
function startBridge(token, slug) {
  const child = spawn(deploymentBinary(), ["mcp", "--document", `${deployment.base}/docs/${slug}`], {
    stdio: ["pipe", "pipe", "pipe"],
    env: {
      PATH: process.env.PATH,
      HOME: home,
      XDG_CONFIG_HOME: join(home, "config"),
      XDG_DATA_HOME: join(home, "data"),
      XDG_STATE_HOME: join(home, "state"),
      XDG_CACHE_HOME: join(home, "cache"),
      LIBREPAPER_AGENT_TOKEN: token,
    },
  });
  bridges.push(child);
  let stderr = "";
  child.stderr.on("data", (bytes) => { stderr += bytes; });
  const pending = new Map();
  const exited = new Promise((done) => child.once("close", (code) => done(code)));
  createInterface({ input: child.stdout }).on("line", (line) => {
    let message;
    try { message = JSON.parse(line); } catch { return; }
    pending.get(message.id)?.(message);
    pending.delete(message.id);
  });
  let next = 0;
  const rpc = (method, params) => new Promise((resolve, reject) => {
    const id = ++next;
    const timer = setTimeout(() => reject(new Error(`${method} timed out\n${stderr}`)), 15000);
    pending.set(id, (message) => { clearTimeout(timer); resolve(message); });
    exited.then((code) => { clearTimeout(timer); reject(new Error(`the bridge exited with ${code} before ${method} answered\n${stderr}`)); });
    child.stdin.write(`${JSON.stringify({ jsonrpc: "2.0", id, method, params })}\n`);
  });
  const call = async (name, args) => (await rpc("tools/call", { name, arguments: args })).result;
  const start = async () => {
    const initialized = await rpc("initialize", {
      protocolVersion: "2025-06-18", capabilities: {}, clientInfo: { name: "agent-token-test", version: "1" },
    });
    assert.ok(initialized.result, `initialize failed: ${JSON.stringify(initialized)}\n${stderr}`);
    child.stdin.write(`${JSON.stringify({ jsonrpc: "2.0", method: "notifications/initialized" })}\n`);
    const { tools } = (await rpc("tools/list", {})).result;
    return tools.map((tool) => tool.name).sort();
  };
  return { rpc, call, start, exited, get stderr() { return stderr; } };
}

try {
  home = await mkdtemp(join(tmpdir(), "librepaper-agent-token-"));
  for (const dir of ["config", "data", "state", "cache"]) await mkdir(join(home, dir), { recursive: true });

  const { slug } = await deployment.publish({
    title: "Token",
    source: "# Token\n\nThe first paragraph.\n",
    source_format: "markdown",
  });
  const ownerHeaders = { "content-type": "application/json", "x-librepaper-client": "1", cookie: deployment.cookie };
  const share = async (body) => {
    const response = await fetch(`${deployment.base}/api/documents/${slug}/share`, {
      method: "POST", headers: ownerHeaders, body: JSON.stringify(body),
    });
    const payload = await response.json();
    assert.equal(response.status, 200, `share ${JSON.stringify(body)} refused: ${response.status} ${JSON.stringify(payload)}`);
    return payload;
  };
  const mint = async (role, name, budget = null) => {
    const payload = await share({ create: { kind: "token", role, name, until: "1d", budget } });
    assert.match(payload.created?.token || "", /^lpt_/, `no token minted: ${JSON.stringify(payload)}`);
    return payload.created;
  };
  const commentsOf = async () => {
    const stored = await fetch(`${deployment.base}/api/documents/${slug}/comments`, { headers: ownerHeaders }).then((response) => response.json());
    return stored.comments || stored.items || stored;
  };
  const readFirst = async (bridge) => {
    const read = await bridge.call("document_read", { queries: [{ kind: "search", query: "first paragraph" }] });
    assert.notEqual(read.isError, true, `document_read failed: ${JSON.stringify(read)}`);
    const rangeId = read.structuredContent.results[0].matches?.[0]?.range_id;
    assert.ok(rangeId, `the search names a range: ${JSON.stringify(read.structuredContent.results[0])}`);
    return { viewId: read.structuredContent.view_id, rangeId };
  };

  // Comment token with a budget of one.
  const commentToken = await mint("comment", "Remote reviewer", 1);
  const commenter = startBridge(commentToken.token, slug);
  const commentTools = await commenter.start();
  for (const name of ["document_read", "document_comment", "document_result"]) {
    assert.ok(commentTools.includes(name), `a comment token is served ${name}: ${commentTools}`);
  }
  for (const name of ["document_propose", "document_apply"]) {
    assert.ok(!commentTools.includes(name), `a comment token is not served ${name}: ${commentTools}`);
  }
  const first = await readFirst(commenter);
  const created = await commenter.call("document_comment", {
    action: "create", body: "A remote remark.", view_id: first.viewId, range_id: first.rangeId,
  });
  assert.notEqual(created.isError, true, `the first comment is allowed: ${JSON.stringify(created)}`);
  const second = await commenter.call("document_comment", {
    action: "create", body: "A second remark.", view_id: first.viewId, range_id: first.rangeId,
  });
  assert.equal(second.isError, true, `the second comment is refused: ${JSON.stringify(second)}`);
  assert.match(JSON.stringify(second.structuredContent ?? second), /budget/i, "the refusal names the comment budget");
  const items = await commentsOf();
  const remark = items.find((item) => item.body === "A remote remark.");
  assert.ok(remark, `the comment is stored: ${JSON.stringify(items).slice(0, 400)}`);
  assert.equal(remark.creator, "Remote reviewer", "the comment displays under the token's name");
  assert.ok(!items.some((item) => item.body === "A second remark."), "the refused comment is not stored");

  // The token opens nothing but the document's tools.
  const bearer = { authorization: `Bearer ${commentToken.token}`, "x-librepaper-client": "1" };
  const create = await fetch(`${deployment.base}/api/documents`, {
    method: "POST", headers: { ...bearer, "content-type": "application/json" },
    body: JSON.stringify({ title: "Nope", source: "x", source_format: "markdown" }),
  });
  assert.equal(create.status, 401, "a token cannot create a project");
  const list = await fetch(`${deployment.base}/api/list`, { headers: bearer });
  assert.equal(list.status, 401, "a token cannot list projects");

  // Suggest token.
  const suggestToken = await mint("suggest", "Remote suggester");
  const suggester = startBridge(suggestToken.token, slug);
  const suggestTools = await suggester.start();
  assert.ok(suggestTools.includes("document_propose"), `a suggest token is served document_propose: ${suggestTools}`);
  assert.ok(!suggestTools.includes("document_apply"), `a suggest token is not served document_apply: ${suggestTools}`);
  const seen = await readFirst(suggester);
  const proposed = await suggester.call("document_propose", {
    view_id: seen.viewId, publish: "suggestions", validation: "source",
    patches: [{ range_id: seen.rangeId, replacement: "The suggested paragraph" }],
  });
  assert.notEqual(proposed.isError, true, `the suggestion is accepted: ${JSON.stringify(proposed)}`);

  // The change is pending in the document: join the room the way a page does.
  const page = new WebSocket(`${deployment.base.replace("http", "ws")}/ws/${slug}`, { headers: { cookie: deployment.cookie } });
  const frames = [];
  page.addEventListener("message", (event) => frames.push(JSON.parse(String(event.data))));
  await new Promise((done, fail) => {
    page.addEventListener("open", done, { once: true });
    page.addEventListener("error", () => fail(new Error("the page socket could not be opened")), { once: true });
  });
  const waitFor = async (what, match) => {
    for (let waited = 0; waited < 15000; waited += 50) {
      const found = frames.find(match);
      if (found) return found;
      await new Promise((done) => setTimeout(done, 50));
    }
    throw new Error(`the page never received ${what}`);
  };
  await waitFor("hello", (frame) => frame.type === "hello");
  page.send(JSON.stringify({ type: "doc-open", protocol: "librepaper.room.v3", vector: "", request_id: "open" }));
  const join_ = await waitFor("the doc-open reply", (frame) => frame.type === "doc-rows");
  const mirror = new LoroDoc();
  const base64 = (text) => Uint8Array.from(Buffer.from(text, "base64"));
  for (const update of join_.updates || []) mirror.import(base64(update));
  const changes = listChanges(mirror);
  page.close();
  assert.equal(changes.length, 1, `one pending change: ${JSON.stringify(changes)}`);
  assert.equal(changes[0].inserted, "The suggested paragraph");

  // Edit token: stage privately, then apply.
  const editToken = await mint("edit", "Remote editor");
  const editor = startBridge(editToken.token, slug);
  const editTools = await editor.start();
  assert.ok(editTools.includes("document_apply"), `an edit token is served document_apply: ${editTools}`);
  const target = await readFirst(editor);
  const staged = await editor.call("document_propose", {
    view_id: target.viewId, publish: "private", validation: "source",
    patches: [{ range_id: target.rangeId, replacement: "The edited paragraph" }],
  });
  assert.notEqual(staged.isError, true, `the private candidate is staged: ${JSON.stringify(staged)}`);
  const candidateId = staged.structuredContent?.candidate_id
    ?? JSON.stringify(staged).match(/"candidate_id":"([^"]+)"/)?.[1];
  assert.ok(candidateId, `the candidate has an id: ${JSON.stringify(staged).slice(0, 400)}`);
  const applied = await editor.call("document_apply", { view_id: target.viewId, candidate_id: candidateId });
  assert.notEqual(applied.isError, true, `the candidate is applied: ${JSON.stringify(applied)}`);
  const project = await fetch(`${deployment.base}/api/documents/${slug}/project`, { headers: ownerHeaders }).then((response) => response.json());
  assert.ok(JSON.stringify(project).includes("The edited paragraph"), "the owner sees the applied edit");

  // Revoking the comment token stops a fresh bridge.
  await share({ revoke: { id: commentToken.id } });
  const revoked = startBridge(commentToken.token, slug);
  let refused = false;
  try {
    await revoked.start();
    const attempt = await revoked.call("document_read", { queries: [{ kind: "outline" }] });
    refused = attempt?.isError === true;
  } catch {
    refused = true;
  }
  assert.ok(refused, "a revoked token neither starts a bridge nor reads");

  console.log("agent-token: comment, suggest and edit tokens, budget, 401 elsewhere and revocation verified");
} finally {
  for (const child of bridges) child.kill();
  await deployment.stop();
  if (home) await rm(home, { recursive: true, force: true });
}
