import assert from "node:assert/strict";
import test from "node:test";
import { compileReply, createCompileResponder, errorReply, parseCompileRequest } from "../../src/lib/agent-compile.js";

const id = `compile_${"a".repeat(32)}`;
const frame = (extra = {}) => ({ type: "compile_request", id, path: "paper.tex", output: null, run: false, expires_at: 2000, ...extra });

test("a valid compile request is parsed", () => {
  assert.deepEqual(parseCompileRequest(frame({ output: "pdf" }), 1000), { id, path: "paper.tex", output: "pdf", run: false, expires_at: 2000 });
});

test("a bad id, an unknown output or an expired request is refused", () => {
  assert.equal(parseCompileRequest(frame({ id: "compile_xyz" }), 1000), null);
  assert.equal(parseCompileRequest(frame({ output: "docx" }), 1000), null);
  assert.equal(parseCompileRequest(frame({ path: "" }), 1000), null);
  assert.equal(parseCompileRequest(frame(), 2000), null);
  assert.equal(parseCompileRequest({ type: "other" }, 1000), null);
});

test("a reply lists errors first and keeps only the contract keys", () => {
  const reply = compileReply({ pdf: new Uint8Array(3), diagnostics: [
    { severity: "warning", message: "w", extra: 1 },
    { severity: "error", message: "e", file: "paper.tex", line: 2 },
  ] }, "rev");
  assert.equal(reply.output, "pdf");
  assert.equal(reply.revision, "rev");
  assert.equal(reply.diagnostics[0].severity, "error");
  assert.equal("extra" in reply.diagnostics[1], false);
});

test("the log is cut to its last 16 KiB without splitting a character", () => {
  const log = `${"x".repeat(100)}${"é".repeat(20000)}`;
  const reply = compileReply({ html: "<p>", log }, "rev");
  const bytes = new TextEncoder().encode(reply.log).byteLength;
  assert.ok(bytes <= 16 * 1024);
  assert.ok(bytes > 16 * 1024 - 2);
  assert.equal(reply.log.includes("�"), false);
  assert.equal(reply.output, "html");
});

test("a result with no page has output none", () => {
  const reply = compileReply({ ok: false, diagnostics: [], log: "boom" }, "rev");
  assert.equal(reply.output, "none");
  assert.equal(reply.ok, false);
});

test("a second request while one runs is answered busy, and a repeated id is ignored", async () => {
  const posted = [];
  let release;
  const gate = new Promise((resolve) => { release = resolve; });
  const responder = createCompileResponder({
    compile: async () => { await gate; return { ok: true }; },
    post: async (to, body) => { posted.push([to, body]); },
  });
  const other = `compile_${"b".repeat(32)}`;
  const first = responder.handle({ id });
  await responder.handle({ id: other });
  await responder.handle({ id });
  assert.deepEqual(posted, [[other, errorReply("busy", posted[0][1].message)]]);
  release();
  await first;
  assert.equal(posted.length, 2);
  assert.deepEqual(posted[1], [id, { ok: true }]);
});

test("a compile that throws is answered failed", async () => {
  const posted = [];
  const responder = createCompileResponder({
    compile: async () => { throw new Error("nope"); },
    post: async (to, body) => { posted.push(body); },
  });
  await responder.handle({ id });
  assert.deepEqual(posted, [errorReply("failed", "nope")]);
});
