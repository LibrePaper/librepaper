// Folder projects against the real detached local service and isolated state:
// list, tree, read, `PUT folder` (edit, rename, trash), outside changes and
// forgetting. The native folder chooser cannot run in a test, so the project
// binding is written into the config home before the service starts, in the
// shape `BindingStore::open_project` persists. Never touches the user's
// installation, trash, or documents.
import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { createHash, randomBytes } from "node:crypto";
import { existsSync } from "node:fs";
import { mkdtemp, mkdir, readFile, readdir, realpath, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { resolve, join } from "node:path";
import { createServer } from "node:net";
const exec = promisify(execFile);
const repository = resolve(import.meta.dirname, "../../..");
const binary = resolve(process.env.LIBREPAPER_TEST_BINARY || join(repository, "target/debug/librepaper"));
if (!existsSync(binary)) {
  console.log("local-projects-service: no librepaper binary; skipping (run `cargo build`)");
  process.exit(0);
}
const root = await realpath(await mkdtemp(join(tmpdir(), "librepaper-projects-")));
const sha = (bytes) => createHash("sha256").update(bytes).digest("hex");
const site = "https://papers.example";

// Step 1: the project folder. The data directory sits beside it so the
// freedesktop trash stays inside the test and on the same device.
const folder = join(root, "paper");
const one = Buffer.from("\\chapter{One}\r\nfirst line\r\nsecond line\r\n");
const png = Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);
const csv = Buffer.from("a,b\n1,2\n");
const main1 = Buffer.from("\\documentclass{article}\n\\begin{document}\nHello\n\\end{document}\n");
await mkdir(join(folder, "chapters"), { recursive: true });
await mkdir(join(folder, ".hidden"));
await mkdir(join(folder, "node_modules"));
await writeFile(join(folder, "main.tex"), main1);
await writeFile(join(folder, "chapters/one.tex"), one);
await writeFile(join(folder, "fig.png"), png);
await writeFile(join(folder, "data.csv"), csv);
await writeFile(join(folder, ".hidden/x.tex"), "hidden\n");
await writeFile(join(folder, "node_modules/y.tex"), "module\n");

const tools = join(root, "bin");
await mkdir(tools);
const env = {
  ...process.env,
  HOME: root,
  XDG_STATE_HOME: join(root, "state"),
  XDG_CACHE_HOME: join(root, "cache"),
  XDG_CONFIG_HOME: join(root, "config"),
  XDG_DATA_HOME: join(root, "data"),
  PATH: `${tools}:${process.env.PATH}`,
  LIBREPAPER_SERVER: "https://app.example/",
};
delete env.DISPLAY;
delete env.WAYLAND_DISPLAY;
const local = (...args) => exec(binary, ["local", ...args], { env, timeout: 20000 });
const listener = createServer();
await new Promise((done) => listener.listen(0, "127.0.0.1", done));
const port = listener.address().port;
await new Promise((done) => listener.close(done));
env.LIBREPAPER_LOCAL_PORT = String(port);

// Seed the binding file the way `open_project` writes it, before the service
// starts, so the service loads it.
const id = `q-${randomBytes(16).toString("hex")}`;
const project = `local-${id.slice(2)}`;
const now = Math.floor(Date.now() / 1000);
const bindingsDir = join(env.XDG_CONFIG_HOME, "librepaper/local");
await mkdir(bindingsDir, { recursive: true });
await writeFile(
  join(bindingsDir, "quarto-bindings.json"),
  JSON.stringify({ bindings: [{ id, origin: site, project, root: folder, entrypoint: "", created_at: now, execution_granted: true, kind: "project", opened_at: now }] }),
);

try {
  const found = JSON.parse((await local("endpoint")).stdout.trim().split("\n").at(-1));
  const base = `${found.address}librepaper/local`;
  const shell = { Authorization: `Bearer ${found.token}`, "X-LibrePaper-Origin": site };
  const get = (path, init = {}) => fetch(`${base}/${path}`, { headers: shell, ...init });
  const json = async (response, status = 200) => {
    assert.equal(response.status, status, await response.clone().text());
    return response.json();
  };
  const sorted = (list) => [...list].sort();

  // Step 2: list, tree, skip rules and the cursor.
  const listed = await json(await get("projects"));
  assert.equal(listed.length, 1, "the seeded folder project is listed");
  assert.equal(listed[0].id, id);
  assert.equal(listed[0].project, project);
  assert.equal(listed[0].name, "paper");
  assert.equal(listed[0].present, true);
  const tree = await json(await get(`projects/${id}/tree`));
  assert.deepEqual(sorted(tree.files.map((file) => file.path)), ["chapters/one.tex", "data.csv", "fig.png", "main.tex"]);
  const expected = { "main.tex": main1, "chapters/one.tex": one, "fig.png": png, "data.csv": csv };
  for (const file of tree.files) {
    assert.equal(file.size, expected[file.path].length, `${file.path} size`);
    assert.equal(file.sha256, sha(expected[file.path]), `${file.path} digest`);
  }
  const skipped = tree.skipped.map((entry) => entry.path);
  assert.ok(skipped.includes(".hidden") && skipped.includes("node_modules"), "hidden and node_modules are skipped");
  assert.equal(typeof tree.cursor, "number", "the tree answers a cursor");
  const baseline = tree.cursor;

  // Step 3: exact bytes, CRLF kept, and a path with `..` refused. The slash
  // is encoded so the URL parser does not fold the dot segment away.
  const read = await get(`projects/${id}/files/chapters/one.tex`);
  assert.equal(read.status, 200);
  assert.ok(Buffer.from(await read.arrayBuffer()).equals(one), "the file's bytes are exact");
  assert.equal((await get(`projects/${id}/files/chapters/..%2Fmain.tex`)).status, 400, "a path with .. is refused");

  // Step 4: the first mirror. Only main.tex carries bytes.
  const entry = (path, bytes) => ({ path, sha256: sha(bytes), size: bytes.length });
  const put = async (manifest, uploads) => {
    const form = new FormData();
    form.append("manifest", new Blob([JSON.stringify({ manifest })], { type: "application/json" }), "manifest.json");
    for (const [path, bytes] of uploads) form.append("file", new Blob([bytes]), path);
    return json(await fetch(`${base}/folder?project=${encodeURIComponent(project)}&binding=${encodeURIComponent(id)}`, { method: "PUT", headers: shell, body: form }));
  };
  const main2 = Buffer.from("\\documentclass{article}\n\\begin{document}\nHello from the editor\n\\end{document}\n");
  const first = await put([entry("main.tex", main2), entry("chapters/one.tex", one), entry("fig.png", png)], [["main.tex", main2]]);
  assert.deepEqual(first.trashed, [], "nothing was trashed");
  assert.deepEqual(first.renamed, []);
  assert.deepEqual(first.kept, []);
  assert.ok((await readFile(join(folder, "main.tex"))).equals(main2), "main.tex holds the new content");
  assert.ok(existsSync(join(folder, "data.csv")), "data.csv was never in a manifest and stays");

  // Step 5: drop fig.png and rename one.tex to two.tex.
  const second = await put([entry("main.tex", main2), entry("chapters/two.tex", one)], []);
  assert.deepEqual(second.renamed, [{ from: "chapters/one.tex", to: "chapters/two.tex" }]);
  assert.deepEqual(second.trashed, ["fig.png"]);
  assert.ok((await readFile(join(folder, "chapters/two.tex"))).equals(one), "the renamed file keeps its bytes");
  assert.ok(!existsSync(join(folder, "chapters/one.tex")), "the old name is gone");
  assert.ok(!existsSync(join(folder, "fig.png")), "fig.png left the folder");
  assert.ok(existsSync(join(folder, "data.csv")), "data.csv still exists");
  if (process.platform === "linux") {
    const trashed = await readdir(join(env.XDG_DATA_HOME, "Trash/files")).catch(() => []);
    assert.ok(trashed.includes("fig.png"), "fig.png is in the test's trash");
  }

  // Step 6: an outside edit is reported with its new digest. The cursor is
  // the latest tree's, so LibrePaper's own writes of step 4 sit after it and
  // must not appear as changes of main.tex.
  const outside = Buffer.from("\\documentclass{article}\n\\begin{document}\nEdited outside, longer text\n\\end{document}\n");
  await writeFile(join(folder, "main.tex"), outside);
  const seen = [];
  const deadline = Date.now() + 10000;
  let since = baseline;
  while (Date.now() < deadline && !seen.some((change) => change.path === "main.tex")) {
    const answer = await json(await get(`projects/${id}/changes?since=${since}`, { signal: AbortSignal.timeout(Math.max(1000, deadline - Date.now())) }));
    since = answer.cursor;
    seen.push(...answer.changes);
  }
  const edited = seen.filter((change) => change.path === "main.tex");
  assert.equal(edited.length, 1, `main.tex is reported once: ${JSON.stringify(seen)}`);
  assert.equal(edited[0].kind, "changed");
  assert.equal(edited[0].sha256, sha(outside));
  assert.equal(edited[0].size, outside.length);
  assert.ok(!seen.some((change) => change.sha256 === sha(main2)), "LibrePaper's own write is not reported");

  // Step 7: forgetting leaves the folder alone.
  const forgotten = await get(`projects/${id}`, { method: "DELETE" });
  assert.equal(forgotten.status, 200);
  assert.deepEqual(await json(await get("projects")), [], "the project is no longer listed");
  assert.ok((await readFile(join(folder, "main.tex"))).equals(outside), "main.tex is untouched");
  assert.ok((await readFile(join(folder, "chapters/two.tex"))).equals(one));
  assert.ok(existsSync(join(folder, "data.csv")));
  console.log("local-projects-service: list, tree, read, folder mirror with rename and trash, outside change and forget passed");
} finally {
  await local("stop").catch(() => {});
  await rm(root, { recursive: true, force: true });
}
