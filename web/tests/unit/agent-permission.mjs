import assert from "node:assert/strict";
import test from "node:test";
import { humanizeTool, permissionAction, permissionButtons } from "../../src/lib/agent-permission.js";

test("humanizeTool rewrites known LibrePaper MCP tools into plain words", () => {
  assert.equal(humanizeTool("Run mcp__librepaper__document_read?"), "Run Read the current document?");
  assert.equal(humanizeTool("mcp__librepaper__document_apply"), "Edit the document");
  assert.equal(humanizeTool("mcp__librepaper__document_comment"), "Comment on the document");
});

test("humanizeTool rewrites a bare LibrePaper tool name with no mcp__ prefix", () => {
  assert.equal(humanizeTool("Call document_propose now"), "Call Suggest changes to the document now");
});

test("humanizeTool falls back to plain words for an unknown mcp__ tool", () => {
  assert.equal(humanizeTool("mcp__github__create_issue"), "Use create issue (github)");
});

test("humanizeTool leaves unrelated text untouched", () => {
  assert.equal(humanizeTool("Run the command?"), "Run the command?");
  assert.equal(humanizeTool(""), "");
  assert.equal(humanizeTool(null), "");
});

test("permissionAction humanizes the input message", () => {
  assert.equal(permissionAction({ message: "mcp__librepaper__document_apply" }), "Edit the document");
});

test("permissionAction falls back when there is no message", () => {
  assert.equal(permissionAction({}), "Allow the agent to continue");
  assert.equal(permissionAction({ message: "" }), "Allow the agent to continue");
  assert.equal(permissionAction(null), "Allow the agent to continue");
});

test("permissionButtons orders and labels the agent's own options", () => {
  const options = [
    { id: "b", kind: "reject_always", label: "Never" },
    { id: "a", kind: "allow_once", label: "Sure" },
    { id: "c", kind: "reject_once", label: "No" },
    { id: "d", kind: "allow_always", label: "Always" },
  ];
  assert.deepEqual(permissionButtons(options), [
    { id: "a", label: "Allow", primary: true },
    { id: "d", label: "Always allow", primary: false },
    { id: "c", label: "Reject", primary: false },
    { id: "b", label: "Always reject", primary: false },
  ]);
});

test("permissionButtons keeps an unrecognized kind and its own label, sorted last", () => {
  const options = [
    { id: "x", kind: "snooze", label: "Ask later" },
    { id: "a", kind: "allow_once", label: "Allow this run" },
  ];
  assert.deepEqual(permissionButtons(options), [
    { id: "a", label: "Allow", primary: true },
    { id: "x", label: "Ask later", primary: false },
  ]);
});

test("permissionButtons never invents an option the agent did not offer", () => {
  assert.deepEqual(permissionButtons([]), []);
  assert.deepEqual(permissionButtons(undefined), []);
});
