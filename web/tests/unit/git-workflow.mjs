// The workflow and the seeding commands the web Git panel hands out are the
// ones docs/git.md prints. The docs are the source; this fails when the two
// drift apart.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { WORKFLOW_FILE, seedingCommands, workflowYaml } from "../../src/lib/git-workflow.js";

const docs = readFileSync(new URL("../../../docs/git.md", import.meta.url), "utf8");
const placeholder = "https://app.librepaper.org/docs/<slug>";

// The fenced block that starts with `first`, without its fences.
function block(language, first) {
  const fence = new RegExp("```" + language + "\\n(" + first.replace(/[.*+?^${}()|[\]\\]/g, "\\$&") + "[\\s\\S]*?)```");
  const match = fence.exec(docs);
  assert.ok(match, `docs/git.md has a ${language} block that starts with ${first}`);
  return match[1];
}

const yaml = block("yaml", "on:");
assert.equal(workflowYaml(placeholder), yaml, "the workflow is the docs' workflow");
assert.ok(docs.includes(`\`${WORKFLOW_FILE}\``), "and the docs name the same file");

const seeding = block("sh", "# Once, on your computer, in a clone of the repository");
assert.equal(seedingCommands(placeholder), seeding, "the seeding commands are the docs' commands");

// The address goes in as given, once in the workflow and once in the pull line.
const address = "https://app.example/docs/paper";
assert.ok(workflowYaml(address).includes(`project: ${address}\n`));
assert.ok(seedingCommands(address).includes(`librepaper pull ${address}   #`));
assert.ok(!workflowYaml(address).includes("<slug>") && !seedingCommands(address).includes("<slug>"));
assert.ok(workflowYaml(address).includes("token: ${{ secrets.LIBREPAPER_TOKEN }}"), "the secret is named, never filled in");

console.log("git-workflow: the workflow and the seeding commands match docs/git.md");
