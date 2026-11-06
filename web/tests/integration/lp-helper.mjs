// The agent helper `skills/librepaper/scripts/lp`, end to end: each command runs
// as a real Python process against a real deployment. Every role gets its own
// HOME, so ~/.cache holds only the tokens that role connected, and the owner
// inspects the pending changes the way a page does, over the websocket.
// Requires a built librepaper binary and LIBREPAPER_TEST_POSTGRES_URL.
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { existsSync, readdirSync, statSync } from "node:fs";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { LoroDoc } from "loro-crdt";
import { listChanges } from "../../src/lib/track-changes.js";
import { startDeployment } from "../helpers/deployment.mjs";

const repository = resolve(import.meta.dirname, "../../..");
const lpPath = join(repository, "skills/librepaper/scripts/lp");

const deployment = await startDeployment({ label: "lp_helper" });
if (!deployment || deployment.unavailable) {
  console.log(`lp-helper: ${deployment?.unavailable || "no librepaper binary; run cargo build"}; skipping`);
  process.exit(0);
}

const homes = [];
const source = "# Résumé 🎉\n\nThe first paragraph.\n\nA repeated line.\n\nA repeated line.\n\nCafé ☕ ends here.\n";
const manifestArgs = JSON.stringify({ queries: [{ kind: "manifest" }] });

// A fresh HOME per role, so the helper can only see the tokens connected in it.
async function freshHome() {
  const dir = await mkdtemp(join(tmpdir(), "librepaper-lp-helper-"));
  homes.push(dir);
  return dir;
}

// Runs the helper. `env` adds variables on top of PATH and HOME only.
function lp(args, { input, home, env = {} } = {}) {
  if (!home) throw new Error("lp needs a home directory");
  const result = spawnSync("python3", [lpPath, ...args], {
    input,
    encoding: "utf8",
    env: { PATH: process.env.PATH, HOME: home, ...env },
  });
  return { status: result.status, stdout: result.stdout, stderr: result.stderr };
}

const tokenDirOf = (home) => join(home, ".cache", "librepaper-agent");

// Every stored token file under the helper's cache directory, at any depth.
function storedTokens(home) {
  const found = [];
  const walk = (directory) => {
    if (!existsSync(directory)) return;
    for (const entry of readdirSync(directory, { withFileTypes: true })) {
      const path = join(directory, entry.name);
      if (entry.isDirectory()) walk(path);
      else if (entry.name.endsWith(".token")) found.push(path);
    }
  };
  walk(tokenDirOf(home));
  return found;
}

function jsonOf(text, what) {
  try {
    return JSON.parse(text);
  } catch {
    assert.fail(`${what} is not JSON: ${String(text).slice(0, 300)}`);
  }
}

// The published markdown file's name, read from the manifest the owner's
// token sees through document_read.
function sourcePathOf(result) {
  assert.equal(result.status, 0, `document_read manifest failed: ${result.stderr}`);
  const names = [];
  const walk = (value) => {
    if (typeof value === "string") {
      if (value.endsWith(".md")) names.push(value);
    } else if (Array.isArray(value)) {
      value.forEach(walk);
    } else if (value && typeof value === "object") {
      Object.values(value).forEach(walk);
    }
  };
  walk(jsonOf(result.stdout, "the manifest"));
  assert.ok(names.length > 0, `the manifest names a markdown file: ${result.stdout.slice(0, 400)}`);
  return names[0];
}

try {
  const { slug } = await deployment.publish({ title: "Agent helper", source, source_format: "markdown" });
  const ownerHeaders = { "content-type": "application/json", "x-librepaper-client": "1", cookie: deployment.cookie };

  const shareFor = async (projectSlug, body) => {
    const response = await fetch(`${deployment.base}/api/documents/${projectSlug}/share`, {
      method: "POST", headers: ownerHeaders, body: JSON.stringify(body),
    });
    const payload = await response.json();
    assert.equal(response.status, 200, `share ${JSON.stringify(body)} refused: ${response.status} ${JSON.stringify(payload)}`);
    return payload;
  };
  const mintFor = async (projectSlug, role, name, budget = null) => {
    const payload = await shareFor(projectSlug, { create: { kind: "token", role, name, until: "1d", budget } });
    assert.ok(/^lpt_/.test(payload.created?.token || ""), `no token minted: ${JSON.stringify({ ...payload, created: undefined }).slice(0, 200)}`);
    return payload.created;
  };
  const commentsOf = async (projectSlug = slug) => {
    const stored = await fetch(`${deployment.base}/api/documents/${projectSlug}/comments`, { headers: ownerHeaders }).then((response) => response.json());
    return stored.comments || stored.items || stored;
  };

  // The owner's pending changes, read the way a page reads them: join the
  // room, import the rows, and list the changes in the mirror.
  const pendingChanges = async (projectSlug = slug) => {
    const page = new WebSocket(`${deployment.base.replace("http", "ws")}/ws/${projectSlug}`, { headers: { cookie: deployment.cookie } });
    const frames = [];
    page.addEventListener("message", (event) => frames.push(JSON.parse(String(event.data))));
    try {
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
      const rows = await waitFor("the doc-open reply", (frame) => frame.type === "doc-rows");
      const mirror = new LoroDoc();
      for (const update of rows.updates || []) mirror.import(Uint8Array.from(Buffer.from(update, "base64")));
      return listChanges(mirror);
    } finally {
      page.close();
    }
  };

  // The owner's copy of the project, read through the helper's own route.
  const pathOf = (projectSlug, home) => sourcePathOf(lp(["--project", projectSlug, "call", "document_read", manifestArgs], { home }));

  // 1. A garbage token, well formed for this origin, is refused and stored nowhere.
  const editToken = await mintFor(slug, "edit", "Agent");
  const tail = editToken.token.slice(-4);
  const garbled = editToken.token.slice(0, -4) + [...tail].map((c) => (c === "A" ? "B" : "A")).join("");
  assert.ok(garbled !== editToken.token, "the garbled token differs from the minted one");
  const home1 = await freshHome();
  const refused = lp(["connect"], { input: garbled, home: home1 });
  assert.notEqual(refused.status, 0, `a garbage token is not connected: ${refused.stderr}`);
  assert.match(refused.stderr, /refused/, `the refusal says so: ${refused.stderr}`);
  assert.deepEqual(storedTokens(home1), [], `a refused token is not stored: ${refused.stderr}`);

  // 2. A real edit token connects, and its file is private to this user.
  const connected = lp(["connect"], { input: editToken.token, home: home1 });
  assert.equal(connected.status, 0, `connect with an edit token: ${connected.stderr}`);
  const path = pathOf(slug, home1);
  assert.ok(connected.stdout.includes(slug), `connect names the project's slug: ${connected.stdout}`);
  assert.ok(connected.stdout.includes("Access: Edit"), `connect names the access: ${connected.stdout}`);
  assert.ok(connected.stdout.includes(path), `connect names the source file ${path}: ${connected.stdout}`);
  assert.ok(connected.stdout.includes("--project"), `connect explains --project: ${connected.stdout}`);
  assert.ok(!connected.stdout.includes(editToken.token) && !connected.stderr.includes(editToken.token), "connect never prints the token");
  const stored = storedTokens(home1);
  assert.equal(stored.length, 1, `connect stores one token file: ${stored.length}`);
  assert.equal(statSync(stored[0]).mode & 0o777, 0o600, "the stored token file is mode 0600");
  assert.equal(statSync(tokenDirOf(home1)).mode & 0o777, 0o700, "the token directory is mode 0700");

  // 3. Plain read returns the exact source.
  const read = lp(["read", path], { home: home1 });
  assert.equal(read.status, 0, `read: ${read.stderr}`);
  assert.equal(read.stdout, source, `read returns the source: ${read.stderr}`);

  // 4. A suggestion is committed and becomes one pending change.
  assert.equal((await pendingChanges()).length, 0, "the project starts with no pending changes");
  const suggested = lp(["suggest", path, "--find", "first paragraph", "--replace", "opening paragraph"], { home: home1 });
  assert.equal(suggested.status, 0, `suggest: ${suggested.stderr}`);
  assert.equal(jsonOf(suggested.stdout, "suggest's output").status, "committed", `suggest reports committed: ${suggested.stdout}`);
  assert.equal((await pendingChanges()).length, 1, "the suggestion is one pending change");

  // 5. Text that is absent changes nothing.
  const absent = lp(["suggest", path, "--find", "no such sentence here", "--replace", "x"], { home: home1 });
  assert.equal(absent.status, 1, `a missing find text exits 1: ${absent.stdout}`);
  assert.match(absent.stderr, /not found/i, `the missing text is reported: ${absent.stderr}`);
  assert.match(absent.stderr, /nothing was changed/i, `the refusal says nothing changed: ${absent.stderr}`);
  assert.equal((await pendingChanges()).length, 1, "a missing find text records no change");

  // 6. Text that occurs twice is refused, and the lines are named.
  const ambiguous = lp(["suggest", path, "--find", "A repeated line.", "--replace", "x"], { home: home1 });
  assert.equal(ambiguous.status, 1, `an ambiguous find exits 1: ${ambiguous.stdout}`);
  assert.match(ambiguous.stderr, /2 times/, `the repeat count is reported: ${ambiguous.stderr}`);
  assert.match(ambiguous.stderr, /\b5\b/, `line 5 is named: ${ambiguous.stderr}`);
  assert.match(ambiguous.stderr, /\b7\b/, `line 7 is named: ${ambiguous.stderr}`);
  assert.equal((await pendingChanges()).length, 1, "an ambiguous find records no change");

  // 7. A zero-length insertion after a non-ASCII anchor adds exactly its text.
  const beforeAfter = await pendingChanges();
  const afterText = "\n\nAfter the coffee.";
  const inserted = lp(["insert", path, "--after", "Café ☕ ends here.", "--text", afterText], { home: home1 });
  assert.equal(inserted.status, 0, `insert --after: ${inserted.stderr}`);
  const afterInsert = await pendingChanges();
  assert.equal(afterInsert.length, beforeAfter.length + 1, "the insertion is one new pending change");
  const insertion = afterInsert.find((change) => change.inserted === afterText);
  assert.ok(insertion, `the pending insertion adds exactly the text: ${JSON.stringify(afterInsert.map((c) => c.inserted))}`);
  assert.equal(insertion.deleted, "", "the insertion deletes nothing");

  // 8. Insert at the end, with the text from stdin.
  const beforeEnd = (await pendingChanges()).length;
  const ended = lp(["insert", path, "--end", "--text-file", "-"], { home: home1, input: "Last line.\n" });
  assert.equal(ended.status, 0, `insert --end: ${ended.stderr}`);
  const afterEnd = await pendingChanges();
  assert.equal(afterEnd.length, beforeEnd + 1, "the end insertion is one new pending change");
  assert.ok(afterEnd.some((change) => change.inserted.includes("Last line.")), "the end insertion carries the stdin text");

  // 9. A comment on non-ASCII anchor text is stored under the token's name.
  const commented = lp(["comment", path, "--on", "Résumé 🎉", "--body", "Nice."], { home: home1 });
  assert.equal(commented.status, 0, `comment: ${commented.stderr}`);
  const nice = (await commentsOf()).find((item) => item.body === "Nice.");
  assert.ok(nice, "the comment is stored with its body");
  assert.equal(nice.creator, "Agent", "the comment displays under the token's name");
  const anchorFields = [];
  const findAnchors = (value, key = "") => {
    if (typeof value === "string" && /anchor|quote|selected|selection/i.test(key)) anchorFields.push([key, value]);
    else if (value && typeof value === "object") for (const [k, v] of Object.entries(value)) findAnchors(v, k);
  };
  findAnchors(nice);
  for (const [key, value] of anchorFields) {
    assert.equal(value, "Résumé 🎉", `the comment's ${key} is the anchor text`);
  }

  // 10. A multi-line body from stdin is stored exactly.
  const multiline = "He said \"it's\" fine.\nA backslash: \\ and \\n stay literal.\nLast line 'quoted'";
  const bodied = lp(["comment", path, "--on", "Café ☕ ends here.", "--body-file", "-"], { home: home1, input: multiline });
  assert.equal(bodied.status, 0, `comment --body-file: ${bodied.stderr}`);
  assert.ok((await commentsOf()).some((item) => item.body === multiline), "the multi-line body is stored exactly");

  // 11. Create a file from stdin, then read it back.
  const newText = "# New\n\nWritten by the helper.\n";
  const created = lp(["create", "new.md", "--text-file", "-"], { home: home1, input: newText });
  assert.equal(created.status, 0, `create: ${created.stderr}`);
  const readNew = lp(["read", "new.md"], { home: home1 });
  assert.equal(readNew.status, 0, `read new.md: ${readNew.stderr}`);
  assert.equal(readNew.stdout, newText, "the created file reads back");

  // 12. Tool schemas: a known tool prints its JSON, an unknown one exits non-zero.
  const schema = lp(["tools", "document_propose"], { home: home1 });
  assert.equal(schema.status, 0, `tools document_propose: ${schema.stderr}`);
  assert.equal(jsonOf(schema.stdout, "the tool schema").name, "document_propose", "the schema names document_propose");
  const unknownTool = lp(["tools", "nope"], { home: home1 });
  assert.notEqual(unknownTool.status, 0, "an unknown tool exits non-zero");

  // 13. A comment token connects with comment access only, and cannot suggest.
  const commentToken = await mintFor(slug, "comment", "Reader");
  const home2 = await freshHome();
  const commentConnect = lp(["connect"], { input: commentToken.token, home: home2 });
  assert.equal(commentConnect.status, 0, `connect with a comment token: ${commentConnect.stderr}`);
  assert.ok(commentConnect.stdout.includes("Access: Comment"), `the comment access is named: ${commentConnect.stdout}`);
  assert.match(commentConnect.stdout, /\bcomment\b/, "the comment tool is listed");
  assert.doesNotMatch(commentConnect.stdout, /\bsuggest\b/, "suggest is not listed");
  assert.doesNotMatch(commentConnect.stdout, /\binsert\b/, "insert is not listed");
  assert.doesNotMatch(commentConnect.stdout, /\bcreate\b/, "create is not listed");
  assert.ok(!commentConnect.stdout.includes(commentToken.token), "connect never prints the comment token");
  const beforeRefused = (await pendingChanges()).length;
  const commentSuggest = lp(["suggest", path, "--find", "first paragraph", "--replace", "refused"], { home: home2 });
  assert.notEqual(commentSuggest.status, 0, `a comment token cannot suggest: ${commentSuggest.stdout}`);
  assert.equal((await pendingChanges()).length, beforeRefused, "a refused suggestion records no change");

  // Tracked suggestions and insertions already land in the document, so the
  // first project's current source no longer equals `source`.
  const latest = lp(["read", path], { home: home1 }).stdout;
  assert.ok(latest.includes("Café ☕ ends here."), `the current source is read: ${latest.slice(0, 200)}`);

  // 14. A second project in the same home makes plain commands ambiguous.
  const second =await deployment.publish({
    title: "Second", source: "# Second\n\nA different project entirely.\n", source_format: "markdown",
  });
  const slug2 = second.slug;
  const secondToken = await mintFor(slug2, "edit", "Agent");
  const connectedSecond = lp(["connect"], { input: secondToken.token, home: home1 });
  assert.equal(connectedSecond.status, 0, `connect the second project: ${connectedSecond.stderr}`);
  const path2 = pathOf(slug2, home1);
  const source2 = "# Second\n\nA different project entirely.\n";
  const several = lp(["read", path], { home: home1 });
  assert.notEqual(several.status, 0, `plain read is ambiguous: ${several.stdout}`);
  assert.match(several.stderr, /several projects/, `the ambiguity is named: ${several.stderr}`);
  assert.ok(several.stderr.includes(slug) && several.stderr.includes(slug2), `both slugs are listed: ${several.stderr}`);

  const flagBefore = lp(["--project", slug, "read", path], { home: home1 });
  assert.equal(flagBefore.status, 0, `--project before the command: ${flagBefore.stderr}`);
  assert.equal(flagBefore.stdout, latest, "--project before the command reads the first project");
  const flagAfter = lp(["read", path, "--project", slug], { home: home1 });
  assert.equal(flagAfter.status, 0, `--project after the command: ${flagAfter.stderr}`);
  assert.equal(flagAfter.stdout, latest, "--project after the command reads the first project");
  const secondBefore = lp(["--project", slug2, "read", path2], { home: home1 });
  assert.equal(secondBefore.status, 0, `--project before the command, second: ${secondBefore.stderr}`);
  assert.equal(secondBefore.stdout, source2, "--project before the command reads the second project");
  const secondAfter = lp(["read", path2, "--project", slug2], { home: home1 });
  assert.equal(secondAfter.status, 0, `--project after the command, second: ${secondAfter.stderr}`);
  assert.equal(secondAfter.stdout, source2, "--project after the command reads the second project");

  // 15. An unknown project has no stored token.
  const unknown = lp(["--project", "nope", "read", path], { home: home1 });
  assert.notEqual(unknown.status, 0, "an unknown project exits non-zero");
  assert.match(unknown.stderr, /no stored token/, `the missing token is named: ${unknown.stderr}`);

  // 16. Disconnecting the second project removes its file, and plain read works again.
  assert.equal(storedTokens(home1).length, 2, "two token files are stored before disconnecting");
  const disconnected = lp(["disconnect", "--project", slug2], { home: home1 });
  assert.equal(disconnected.status, 0, `disconnect: ${disconnected.stderr}`);
  assert.equal(storedTokens(home1).length, 1, "disconnect removes the second project's file");
  const plainAgain = lp(["read", path], { home: home1 });
  assert.equal(plainAgain.status, 0, `plain read with one project left: ${plainAgain.stderr}`);
  assert.equal(plainAgain.stdout, latest, "plain read returns the remaining project");

  // 17. LIBREPAPER_AGENT_TOKEN overrides the stored tokens.
  const envToken = await mintFor(slug2, "read", "Env reader");
  const overridden = lp(["read", path2], { home: home1, env: { LIBREPAPER_AGENT_TOKEN: envToken.token } });
  assert.equal(overridden.status, 0, `read with LIBREPAPER_AGENT_TOKEN: ${overridden.stderr}`);
  assert.equal(overridden.stdout, source2, "the environment token reads the second project, not the stored one");

  // 18. A long file is read back whole, through several pages.
  const bigLines = [];
  for (let line = 1, bytes = 0; bytes < 120_000; line += 1) {
    const text = `${line}. Long line with é☕ and padding text.\n`;
    bigLines.push(text);
    bytes += Buffer.byteLength(text);
  }
  const big = `${bigLines.join("")}The very last line ☕.\n`;
  const bigCreated = lp(["create", "big.md", "--text-file", "-"], { home: home1, input: big });
  assert.equal(bigCreated.status, 0, `create big.md: ${bigCreated.stderr}`);
  const bigRead = lp(["read", "big.md"], { home: home1 });
  assert.equal(bigRead.status, 0, `read big.md: ${bigRead.stderr}`);
  assert.equal(bigRead.stdout, big, "the long file reads back exactly");
  const bigCommented = lp(["comment", "big.md", "--on", "The very last line ☕.", "--body", "End."], { home: home1 });
  assert.equal(bigCommented.status, 0, `comment on the last line of big.md: ${bigCommented.stderr}`);

  // 19. A leading-dash find text is passed as its own argument.
  const dashCreated = lp(["create", "dash.md", "--text-file", "-"], { home: home1, input: "Before\n\n---\n\nAfter\n" });
  assert.equal(dashCreated.status, 0, `create dash.md: ${dashCreated.stderr}`);
  const dashed = lp(["suggest", "dash.md", "--find", "---", "--replace", "***"], { home: home1 });
  assert.equal(dashed.status, 0, `suggest a leading-dash find text: ${dashed.stderr}`);
  assert.equal(jsonOf(dashed.stdout, "the dash suggest output").status, "committed", `the dash suggest is committed: ${dashed.stdout}`);

  // 20. A literal --project as the value of --text is text, not an option.
  const flagText = lp(["create", "flag.md", "--text", "--project"], { home: home1 });
  assert.equal(flagText.status, 0, `create flag.md with --project as text: ${flagText.stderr}`);
  const flagRead = lp(["read", "flag.md"], { home: home1 });
  assert.equal(flagRead.stdout, "--project", "the --project text reads back as written");

  console.log("lp-helper: connect, read, suggest, insert, comment, create, tools, roles, several projects, disconnect, env override, long files and leading dashes verified");
} finally {
  await deployment.stop();
  for (const dir of homes) await rm(dir, { recursive: true, force: true });
}
