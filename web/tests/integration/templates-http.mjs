// A custom template is a copy of a project, marked: it leaves the project
// listing, is offered as a template, counts as storage, forks into ordinary
// projects, and goes to the trash like any project.
//
// Requires a built librepaper binary and the optional PostgreSQL test URL.
// Only those established missing prerequisites are skipped.
import assert from "node:assert/strict";
import { randomUUID } from "node:crypto";
import { existsSync } from "node:fs";
import { deploymentBinary, startDeployment } from "../helpers/deployment.mjs";

if (!existsSync(deploymentBinary())) {
  console.log("templates-http: no librepaper binary; skipping (run `cargo build`)");
  process.exit(0);
}
const deployment = await startDeployment({ label: "templates_http" });
if (!deployment || deployment.unavailable) {
  console.log(`templates-http: ${deployment?.unavailable || "no librepaper binary"}; skipping`);
  process.exit(0);
}

const { base, cookie } = deployment;
const api = async (path, { method = "GET", body, signedIn = true } = {}) => {
  const headers = { "x-librepaper-client": "1", ...(signedIn ? { cookie } : {}) };
  if (body !== undefined) headers["content-type"] = "application/json";
  const response = await fetch(`${base}${path}`, { method, headers, body: body === undefined ? undefined : JSON.stringify(body) });
  return { status: response.status, body: await response.json().catch(() => ({})) };
};
const slugs = (rows) => rows.map((row) => row.slug);

try {
  const form = new FormData();
  form.append("file", new Blob(["= Hello\n#include \"chapters/one.typ\"\n"]), "main.typ");
  form.append("file", new Blob(["Chapter one\n"]), "chapters/one.typ");
  form.append("title", "Source");
  form.append("main", "main.typ");
  const created = await fetch(`${base}/api/documents`, { method: "POST", headers: { "x-librepaper-client": "1", cookie }, body: form });
  assert.ok(created.ok, `upload: ${created.status}`);
  const source = await created.json();

  const currentLabel = (request_id) => api(`/api/documents/${source.slug}/history/current`, {
    method: "PATCH", body: { label: "First saved state", request_id },
  });
  assert.equal((await api(`/api/documents/${source.slug}/history/current`, {
    method: "PATCH", body: { label: "First saved state" },
  })).status, 400, "current labels require a retry identity");
  const labelRequestId = randomUUID();
  const firstLabel = await currentLabel(labelRequestId);
  const replayedLabel = await currentLabel(labelRequestId);
  assert.equal(firstLabel.status, 200, JSON.stringify(firstLabel.body));
  assert.equal(replayedLabel.status, 200, JSON.stringify(replayedLabel.body));
  assert.equal(replayedLabel.body.sha, firstLabel.body.sha, "an ambiguous retry returns the original current label");

  assert.equal((await api(`/api/documents/${source.slug}/template`, { method: "POST", body: { title: "  " } })).status, 400);
  const requestId = randomUUID();
  const saveBody = { title: "My template", request_id: requestId };
  const saved = await api(`/api/documents/${source.slug}/template`, { method: "POST", body: saveBody });
  assert.equal(saved.status, 200, JSON.stringify(saved.body));
  const template = saved.body;
  assert.notEqual(template.slug, source.slug, "a template is a copy, not the project itself");
  assert.equal(template.title, "My template");
  const replay = await api(`/api/documents/${source.slug}/template`, { method: "POST", body: saveBody });
  assert.equal(replay.status, 200, JSON.stringify(replay.body));
  assert.equal(replay.body.slug, template.slug, "retrying a lost response returns the same copy");
  assert.equal(
    (await api(`/api/documents/${source.slug}/template`, {
      method: "POST",
      body: { title: "Different title", request_id: requestId },
    })).status,
    409,
    "an operation id cannot be reused for different input",
  );

  const listed = (await api("/api/templates")).body.templates;
  assert.deepEqual(slugs(listed), [template.slug]);
  assert.equal(listed[0].source_format, "typst");
  const templatePage = await api("/api/templates?limit=1");
  assert.equal(templatePage.status, 200);
  assert.match(templatePage.body.next_cursor.after_id, /^[0-9a-f-]{36}$/i);
  const templateCursor = new URLSearchParams(templatePage.body.next_cursor);
  const nextTemplatePage = await api(`/api/templates?${templateCursor}`);
  assert.deepEqual(nextTemplatePage.body.templates, []);

  const projects = (await api("/api/list")).body.documents;
  assert.ok(slugs(projects).includes(source.slug), "the source stays a project");
  assert.ok(!slugs(projects).includes(template.slug), "the template leaves the project listing");

  const storage = (await api("/api/account/storage")).body.usage.documents;
  assert.equal(storage.find((row) => row.slug === template.slug)?.template, true, "the template counts as storage");
  assert.equal(storage.find((row) => row.slug === source.slug)?.template, false);

  const forked = await api(`/api/documents/${template.slug}/fork`, { method: "POST", body: { title: "From template" } });
  assert.equal(forked.status, 200, JSON.stringify(forked.body));
  const after = (await api("/api/list")).body.documents;
  const copy = after.find((row) => row.slug === forked.body.slug);
  assert.ok(copy, "a project made from a template is an ordinary project");
  const tree = await api(`/api/documents/${copy.slug}/project`);
  assert.deepEqual(Object.keys(tree.body.tree.files).sort(), ["chapters/one.typ", "main.typ"], "it carries every file of the template");
  assert.deepEqual(slugs((await api("/api/templates")).body.templates), [template.slug]);

  const deleted = await api(`/api/documents/${template.slug}/delete`, { method: "POST" });
  assert.ok(deleted.status < 300, `delete: ${deleted.status} ${JSON.stringify(deleted.body)}`);
  assert.deepEqual((await api("/api/templates")).body.templates, []);
  const trash = (await api("/api/trash")).body.documents;
  assert.equal(trash.find((row) => row.slug === template.slug)?.template, true, "the trash says it was a template");

  assert.equal((await api("/api/templates", { signedIn: false })).status, 401);
  console.log("templates-http: save, list, hide, storage, fork, trash and sign-in verified");
} finally {
  await deployment.stop();
}
