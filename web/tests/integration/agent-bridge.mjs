// The assistant's document tools, end to end: a real `librepaper mcp`
// bridge on stdio, resolving a runner connection record, talking plain RPC to
// a real deployment. This is the hop no unit test can see: served schemas,
// the bridge's injected operation identity, and the result mapping.
// Requires a built librepaper binary and LIBREPAPER_TEST_POSTGRES_URL.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdtemp, mkdir, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createInterface } from "node:readline";
import { deploymentBinary, startDeployment } from "../helpers/deployment.mjs";

const deployment = await startDeployment({ label: "agent_bridge" });
if (!deployment || deployment.unavailable) {
  console.log(`agent-bridge: ${deployment?.unavailable || "no librepaper binary; run cargo build"}; skipping`);
  process.exit(0);
}

let state;
let bridge;
try {
  const { slug } = await deployment.publish({
    title: "Bridge",
    source: "# Bridge\n\nThe first paragraph.\n",
    source_format: "markdown",
  });
  const sharing = await fetch(`${deployment.base}/api/documents/${slug}/share`, {
    method: "POST",
    headers: { "content-type": "application/json", "x-librepaper-client": "1", cookie: deployment.cookie },
    body: JSON.stringify({ link: { role: "commenter", until: "1d", label: "Agent" } }),
  }).then((response) => response.json());
  const link = new URL(sharing.links.commenter.url, deployment.base).href;

  // The record the companion writes before it starts a runner, under the
  // name the runner derives (`runner_connection_name`).
  state = await mkdtemp(join(tmpdir(), "librepaper-agent-bridge-"));
  const conversation = "c".repeat(32);
  const name = `runner-${createHash("sha256").update(`${link}\0${conversation}`).digest("hex").slice(0, 48)}`;
  await mkdir(join(state, "librepaper", "local"), { recursive: true });
  const now = Math.floor(Date.now() / 1000);
  await writeFile(join(state, "librepaper", "local", "connections.json"),
    JSON.stringify({ [name]: { link, origin: "http://127.0.0.1", created: now, used: now } }), { mode: 0o600 });

  bridge = spawn(deploymentBinary(), ["mcp", "--connection", name], {
    stdio: ["pipe", "pipe", "pipe"],
    env: { ...process.env, XDG_STATE_HOME: state },
  });
  let stderr = "";
  bridge.stderr.on("data", (bytes) => { stderr += bytes; });
  const pending = new Map();
  createInterface({ input: bridge.stdout }).on("line", (line) => {
    const message = JSON.parse(line);
    pending.get(message.id)?.(message);
    pending.delete(message.id);
  });
  let next = 0;
  const rpc = (method, params) => new Promise((resolve, reject) => {
    const id = ++next;
    const timer = setTimeout(() => reject(new Error(`${method} timed out\n${stderr}`)), 15000);
    pending.set(id, (message) => { clearTimeout(timer); resolve(message); });
    bridge.stdin.write(`${JSON.stringify({ jsonrpc: "2.0", id, method, params })}\n`);
  });
  const call = async (name, args) => (await rpc("tools/call", { name, arguments: args })).result;

  const initialized = await rpc("initialize", {
    protocolVersion: "2025-06-18", capabilities: {}, clientInfo: { name: "agent-bridge-test", version: "1" },
  });
  assert.ok(initialized.result, `initialize failed: ${JSON.stringify(initialized)}\n${stderr}`);
  assert.match(initialized.result.instructions || "", /document/i, "the server's instructions reach the agent");
  bridge.stdin.write(`${JSON.stringify({ jsonrpc: "2.0", method: "notifications/initialized" })}\n`);

  // The model never sees an operation key: the bridge owns identity.
  const { tools } = (await rpc("tools/list", {})).result;
  assert.deepEqual(tools.map((tool) => tool.name).sort(),
    ["document_apply", "document_comment", "document_propose", "document_read", "document_result"]);
  for (const tool of tools) {
    assert.equal(tool.inputSchema.properties.operation, undefined, `${tool.name} offers no operation key`);
    assert.equal(tool.inputSchema.properties.target_operation, undefined, `${tool.name} offers no target operation`);
  }

  // A mutation before any read is refused by the bridge itself.
  const early = await call("document_comment", { action: "create", body: "Too early" });
  assert.equal(early.isError, true);
  assert.equal(early.structuredContent.error.code, "read_required");

  const read = await call("document_read", { queries: [{ kind: "search", query: "first paragraph" }] });
  assert.notEqual(read.isError, true, JSON.stringify(read));
  const hit = read.structuredContent.results[0];
  const rangeId = hit.matches?.[0]?.range_id;
  assert.ok(rangeId, `the search names a range: ${JSON.stringify(hit)}`);

  // A page open on the document, the way the browser holds its socket, so the
  // agent's comment can be seen arriving without a reload.
  const page = new WebSocket(`${deployment.base.replace("http", "ws")}/ws/${slug}`, { headers: { cookie: deployment.cookie } });
  const frames = [];
  page.addEventListener("message", (event) => frames.push(JSON.parse(String(event.data))));
  await new Promise((done, fail) => {
    page.addEventListener("open", done, { once: true });
    page.addEventListener("error", () => fail(new Error("the page socket could not be opened")), { once: true });
  });
  const seen = async (what, match) => {
    for (let waited = 0; waited < 15000; waited += 50) {
      const found = frames.find(match);
      if (found) return found;
      await new Promise((done) => setTimeout(done, 50));
    }
    throw new Error(`the page never received ${what}; got ${JSON.stringify(frames.map((frame) => frame.type === "comments-changed" ? frame : frame.type))}`);
  };
  await seen("hello", (frame) => frame.type === "hello");
  // An editor's page joins the room when it opens the source, as the
  // browser's editor does right after hello.
  page.send(JSON.stringify({ type: "doc-open", protocol: "librepaper.room.v3", vector: "", request_id: "open" }));
  await seen("the doc-open reply", (frame) => frame.type === "doc-rows");

  // The comment carries no operation from the model and still lands.
  const comment = await call("document_comment", {
    action: "create", body: "Checked end to end.", view_id: read.structuredContent.view_id, range_id: rangeId,
  });
  assert.notEqual(comment.isError, true, JSON.stringify(comment));
  // The open page is told, so it refreshes its comments rather than waiting
  // for a reload.
  await seen("comments-changed", (frame) => frame.type === "comments-changed" && frame.state?.total === 1);

  // A suggestion's before and after come from its proposal, so an open page
  // must be sent the proposal itself, not only told the comments moved.
  const proposed = await call("document_propose", {
    view_id: read.structuredContent.view_id, publish: "suggestions", validation: "source",
    patches: [{ range_id: rangeId, replacement: "The revised paragraph" }],
  });
  assert.notEqual(proposed.isError, true, JSON.stringify(proposed));
  const announced = await seen("proposal-changed", (frame) => frame.type === "proposal-changed");
  assert.ok(announced.proposal?.id && announced.proposal.branch, `the page gets the whole proposal: ${JSON.stringify(announced).slice(0, 300)}`);
  page.close();
  const stored = await fetch(`${deployment.base}/api/documents/${slug}/comments`, {
    headers: { "x-librepaper-client": "1", cookie: deployment.cookie },
  }).then((response) => response.json());
  const bodies = (stored.comments || stored.items || stored).map?.((item) => item.body) || [];
  assert.ok(bodies.includes("Checked end to end."), `the comment is stored: ${JSON.stringify(stored).slice(0, 400)}`);
  console.log("agent-bridge: tools listed, read, and a comment stored through the real bridge");
} finally {
  bridge?.kill();
  await deployment.stop();
  if (state) await rm(state, { recursive: true, force: true });
}
