// An agent takes back its own tracked changes: the server cuts a proposal down
// to the spans that really differ, and document_withdraw rejects only the
// caller's own pending changes.
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

const deployment = await startDeployment({ label: "agent_withdraw" });
if (!deployment || deployment.unavailable) {
  console.log(`agent-withdraw: ${deployment?.unavailable || "no librepaper binary; run cargo build"}; skipping`);
  process.exit(0);
}

const bridges = [];
let home;

function startBridge(token) {
  const child = spawn(deploymentBinary(), ["mcp"], {
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
      protocolVersion: "2025-06-18", capabilities: {}, clientInfo: { name: "agent-withdraw-test", version: "1" },
    });
    assert.ok(initialized.result, `initialize failed: ${JSON.stringify(initialized)}\n${stderr}`);
    child.stdin.write(`${JSON.stringify({ jsonrpc: "2.0", method: "notifications/initialized" })}\n`);
    const { tools } = (await rpc("tools/list", {})).result;
    return tools.map((tool) => tool.name).sort();
  };
  return { rpc, call, start, exited };
}

try {
  home = await mkdtemp(join(tmpdir(), "librepaper-agent-withdraw-"));
  for (const dir of ["config", "data", "state", "cache"]) await mkdir(join(home, dir), { recursive: true });

  const original = "# Report\n\nAlpha paragraph stays.\n\nBeta paragraph has one word wrong.\n\nGamma paragraph stays.\n";
  const { slug } = await deployment.publish({ title: "Report", source: original, source_format: "markdown" });
  const ownerHeaders = { "content-type": "application/json", "x-librepaper-client": "1", cookie: deployment.cookie };
  const mint = async (name) => {
    const response = await fetch(`${deployment.base}/api/documents/${slug}/share`, {
      method: "POST", headers: ownerHeaders,
      body: JSON.stringify({ create: { kind: "token", role: "edit", name, until: "1d", budget: null } }),
    });
    const payload = await response.json();
    assert.equal(response.status, 200, `share refused: ${response.status} ${JSON.stringify(payload)}`);
    assert.match(payload.created?.token || "", /^lpt_/, `no token minted: ${JSON.stringify(payload)}`);
    return payload.created;
  };

  // The pending changes, seen the way a page sees them.
  const pendingChanges = async () => {
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
    const joined = await waitFor("the doc-open reply", (frame) => frame.type === "doc-rows");
    const mirror = new LoroDoc();
    for (const update of joined.updates || []) mirror.import(Uint8Array.from(Buffer.from(update, "base64")));
    page.close();
    return listChanges(mirror);
  };

  // A view of the file and a range_id covering [start, end) of it.
  const rangeOf = async (bridge, from, to) => {
    const manifest = await bridge.call("document_read", { queries: [{ kind: "manifest" }] });
    assert.notEqual(manifest.isError, true, `manifest failed: ${JSON.stringify(manifest)}`);
    const viewId = manifest.structuredContent.view_id;
    const file = manifest.structuredContent.results[0].files.find((entry) => /\.(md|markdown)$/.test(entry.path)) ?? manifest.structuredContent.results[0].files[0];
    assert.ok(file?.path, `the manifest names a file: ${JSON.stringify(manifest.structuredContent).slice(0, 400)}`);
    const start = from ?? 0;
    const end = to ?? file.bytes ?? Buffer.byteLength(original);
    const read = await bridge.call("document_read", {
      queries: [{ kind: "source", path: file.path, start, end }], snapshot: { view_id: viewId },
    });
    assert.notEqual(read.isError, true, `source read failed: ${JSON.stringify(read)}`);
    const rangeId = read.structuredContent.results[0].blocks?.[0]?.range_id;
    assert.ok(rangeId, `the source read names a range: ${JSON.stringify(read.structuredContent.results[0]).slice(0, 400)}`);
    return { viewId, rangeId };
  };

  const effectIds = (result, kind) => {
    const effects = result.structuredContent?.effects
      ?? JSON.parse(JSON.stringify(result).match(/"effects":(\[[^\]]*\])/)?.[1] ?? "[]");
    return effects.filter((effect) => effect.kind === kind).map((effect) => effect.id);
  };
  const withdraw = (bridge, ids) => bridge.call("document_withdraw", { change_ids: ids });
  const projectJson = async () => JSON.stringify(
    await fetch(`${deployment.base}/api/documents/${slug}/project`, { headers: ownerHeaders }).then((response) => response.json()),
  );

  // Agent one rewrites the whole file but changes one word.
  const one = startBridge((await mint("Agent one")).token);
  const tools = await one.start();
  assert.ok(tools.includes("document_withdraw"), `an edit token is served document_withdraw: ${tools}`);
  const whole = await rangeOf(one);
  const proposed = await one.call("document_propose", {
    view_id: whole.viewId, publish: "suggestions", validation: "source",
    patches: [{ range_id: whole.rangeId, replacement: original.replace("wrong", "right") }],
  });
  assert.notEqual(proposed.isError, true, `the proposal is accepted: ${JSON.stringify(proposed)}`);
  const changeIds = effectIds(proposed, "change");
  assert.ok(changeIds.length >= 1, `the proposal reports its changes: ${JSON.stringify(proposed).slice(0, 400)}`);

  const afterPropose = await pendingChanges();
  assert.equal(afterPropose.length, 1, `exactly one pending change: ${JSON.stringify(afterPropose)}`);
  assert.equal(afterPropose[0].inserted, "right", "only the changed word is inserted, not the document");
  if ("deleted" in afterPropose[0]) assert.equal(afterPropose[0].deleted, "wrong");

  // Withdrawing restores the document exactly.
  const withdrawn = await withdraw(one, changeIds);
  assert.notEqual(withdrawn.isError, true, `the withdrawal is accepted: ${JSON.stringify(withdrawn)}`);
  assert.deepEqual(effectIds(withdrawn, "withdrawn").sort(), [...changeIds].sort(), "every change is reported withdrawn");
  assert.equal((await pendingChanges()).length, 0, "no change is pending after the withdrawal");
  const restored = await projectJson();
  assert.ok(restored.includes(JSON.stringify(original).slice(1, -1)), "the file text is the original again");
  assert.ok(!restored.includes("one word right"), "the edit is gone");

  // A second withdrawal of the same ids is refused.
  const again = await withdraw(one, changeIds);
  assert.equal(again.isError, true, `a decided change cannot be withdrawn again: ${JSON.stringify(again)}`);
  assert.match(JSON.stringify(again), /not pending/, "the refusal says the change is not pending");

  // Another author's change is not yours to withdraw.
  const two = startBridge((await mint("Agent two")).token);
  await two.start();
  const gammaStart = Buffer.byteLength(original.slice(0, original.indexOf("Gamma")));
  const gamma = await rangeOf(one, gammaStart, gammaStart + Buffer.byteLength("Gamma"));
  const small = await one.call("document_propose", {
    view_id: gamma.viewId, publish: "suggestions", validation: "source",
    patches: [{ range_id: gamma.rangeId, replacement: "Delta" }],
  });
  assert.notEqual(small.isError, true, `the small proposal is accepted: ${JSON.stringify(small)}`);
  const smallIds = effectIds(small, "change");
  assert.ok(smallIds.length >= 1, `the small proposal reports its change: ${JSON.stringify(small).slice(0, 400)}`);
  const stolen = await withdraw(two, smallIds);
  assert.equal(stolen.isError, true, `agent two cannot withdraw agent one's change: ${JSON.stringify(stolen)}`);
  assert.match(JSON.stringify(stolen), /was not made by you/, "the refusal names ownership");
  assert.equal((await pendingChanges()).length, 1, "the change is still pending");

  console.log("agent-withdraw: minimal proposal, withdrawal restores the text, repeat and cross-author withdrawals refused");
} finally {
  for (const child of bridges) child.kill();
  await deployment.stop();
  if (home) await rm(home, { recursive: true, force: true });
}
