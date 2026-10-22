// Pull, push and the first connection, against an in-memory Git service and an
// in-memory server. The fake service keeps trees as maps from path to blob id,
// names objects by the hash of their content, and merges text by lines, which
// is honest enough for the cases below.

import assert from "node:assert/strict";
import { createHash } from "node:crypto";

import { LoroDoc, LoroText } from "loro-crdt";

import { sha256Hex, sha256HexOfText } from "../../src/lib/digest.js";
import { createBridge } from "../../src/lib/gitsync/bridge.js";
import * as sync from "../../src/lib/gitsync/sync.js";
import { unsuggestedText } from "../../src/lib/track-changes.js";

globalThis.location = { hostname: "example.test" };

const enc = new TextEncoder();
const dec = new TextDecoder();
const sha1 = (data) => createHash("sha1").update(data).digest("hex");
const bytesOf = (content) => (typeof content === "string" ? enc.encode(content) : content);

function httpError(status, body, extra = {}) {
  return Object.assign(new Error(body.error || `status ${status}`), { status, body, ...extra });
}

// ---------------------------------------------------------------- fake Git

const lines = (text) => text.match(/[^\n]*\n|[^\n]+/g) || [];

function span(base, other) {
  let start = 0;
  while (start < base.length && start < other.length && base[start] === other[start]) start += 1;
  let tail = 0;
  while (
    tail < base.length - start
    && tail < other.length - start
    && base[base.length - 1 - tail] === other[other.length - 1 - tail]
  ) tail += 1;
  return { start, end: base.length - tail, replacement: other.slice(start, other.length - tail) };
}

/// Line merge for one changed region on each side; null when they touch.
function merge3(base, ours, theirs) {
  const b = lines(base);
  const o = span(b, lines(ours));
  const t = span(b, lines(theirs));
  if (t.start <= o.end && o.start <= t.end) return null;
  const [first, second] = o.start <= t.start ? [o, t] : [t, o];
  return [
    ...b.slice(0, first.start),
    ...first.replacement,
    ...b.slice(first.end, second.start),
    ...second.replacement,
    ...b.slice(second.end),
  ].join("");
}

class Repo {
  blobs = new Map();
  trees = new Map();
  commits = new Map();
  head = null;
  journal = [];
  refs = new Map();
  known = new Map();
  commitCalls = [];
  treeEdits = [];
  mergeCalls = [];
  pushes = [];
  moveOnPush = 0;

  putBlob(bytes) {
    const id = sha1(Buffer.concat([Buffer.from("blob "), Buffer.from(bytes)]));
    this.blobs.set(id, Uint8Array.from(bytes));
    return id;
  }

  makeTree(map) {
    const sorted = [...map.entries()].sort(([a], [b]) => (a < b ? -1 : 1));
    const id = sha1(JSON.stringify(sorted));
    this.trees.set(id, new Map(sorted));
    return id;
  }

  makeCommit(tree, parents, message, author) {
    const id = sha1(JSON.stringify({ tree, parents, message, author }));
    this.commits.set(id, { tree, parents, message, author });
    return id;
  }

  flat(treeOrCommit) {
    const commit = this.commits.get(treeOrCommit);
    return this.trees.get(commit ? commit.tree : treeOrCommit);
  }

  /// Moves the remote head to a commit that applies `changes` (repository path
  /// to content, or null to delete) on top of it.
  advance(changes, message = "remote") {
    const map = new Map(this.head ? this.flat(this.head) : []);
    for (const [path, content] of Object.entries(changes)) {
      if (content === null) map.delete(path);
      else map.set(path, { id: this.putBlob(bytesOf(content)), mode: "100644" });
    }
    this.head = this.makeCommit(this.makeTree(map), this.head ? [this.head] : [], message, { name: "Remote" });
    return this.head;
  }

  read(path, commit = this.head) {
    const entry = this.flat(commit).get(path);
    return entry ? dec.decode(this.blobs.get(entry.id)) : null;
  }

  idOf(path, commit = this.head) {
    return this.flat(commit).get(path)?.id ?? null;
  }

  files(commit = this.head) {
    return Object.fromEntries([...this.flat(commit).keys()].map((path) => [path, this.read(path, commit)]));
  }

  ancestors(commit) {
    const seen = new Set();
    const stack = [commit];
    while (stack.length) {
      const id = stack.pop();
      if (seen.has(id)) continue;
      seen.add(id);
      stack.push(...this.commits.get(id).parents);
    }
    return seen;
  }

  mergeBase(a, b) {
    const mine = this.ancestors(a);
    const queue = [b];
    while (queue.length) {
      const id = queue.shift();
      if (mine.has(id)) return id;
      queue.push(...this.commits.get(id).parents);
    }
    return null;
  }

  text(id) {
    return id ? dec.decode(this.blobs.get(id)) : null;
  }

  async request(op, body) {
    switch (op) {
      case "status":
        return { git: "2.43.0", supported: true, connected: true };
      case "connect":
        return { ok: true, label: "example.test/r.git" };
      case "fetch":
        return { head: this.head, missing: (body.also || []).filter((id) => !this.commits.has(id)) };
      case "tree": {
        const commit = this.commits.get(body.commit);
        const map = this.trees.get(commit.tree);
        return {
          tree: commit.tree,
          entries: [...map.entries()].map(([path, entry]) => ({
            path,
            mode: entry.mode,
            type: "blob",
            id: entry.id,
            size: this.blobs.get(entry.id).length,
          })),
        };
      }
      case "blobs": {
        const blobs = {};
        for (const id of body.ids) blobs[id] = Buffer.from(this.blobs.get(id)).toString("base64");
        return { blobs };
      }
      case "known-assets": {
        const known = {};
        for (const sha of body.sha256) if (this.known.has(sha)) known[sha] = this.known.get(sha);
        return { known };
      }
      case "put-blobs": {
        const ids = [];
        for (const item of body.blobs) {
          const bytes = item.text !== undefined ? enc.encode(item.text) : Buffer.from(item.base64, "base64");
          const id = this.putBlob(bytes);
          if (item.sha256) this.known.set(item.sha256, id);
          ids.push(id);
        }
        return { ids };
      }
      case "tree-edit": {
        this.treeEdits.push(body);
        const map = new Map(body.base ? this.trees.get(body.base) : []);
        for (const edit of body.edits) {
          if (edit.delete) map.delete(edit.path);
          else map.set(edit.path, { id: edit.id, mode: edit.mode || map.get(edit.path)?.mode || "100644" });
        }
        return { tree: this.makeTree(map) };
      }
      case "commit-tree": {
        this.commitCalls.push({ ...body, files: this.treeIds(body.tree) });
        return { commit: this.makeCommit(body.tree, body.parents, body.message, body.author) };
      }
      case "merge": {
        this.mergeCalls.push(body);
        return this.merge(body.ours, body.theirs);
      }
      case "diff": {
        const from = this.flat(body.from);
        const to = this.flat(body.to);
        const changes = [];
        for (const path of new Set([...from.keys(), ...to.keys()])) {
          const a = from.get(path)?.id ?? null;
          const b = to.get(path)?.id ?? null;
          if (a === b) continue;
          changes.push({ path, status: !a ? "A" : !b ? "D" : "M", from: a, to: b });
        }
        return { changes: changes.sort((x, y) => (x.path < y.path ? -1 : 1)) };
      }
      case "ancestor":
        return { is_ancestor: this.ancestors(body.descendant).has(body.ancestor) };
      case "push": {
        this.pushes.push(body.commit);
        if (this.moveOnPush > 0) {
          this.moveOnPush -= 1;
          this.advance({ [`moved-${this.moveOnPush}.md`]: "moved\n" });
        }
        if (this.commits.get(body.commit).parents[0] !== this.head) {
          throw httpError(409, { error: "The branch moved.", kind: "moved" }, { kind: "moved", name: "Refused" });
        }
        this.head = body.commit;
        return { pushed: true };
      }
      case "keep":
        for (const [name, commit] of Object.entries(body.refs)) this.refs.set(`${body.operation}/${name}`, commit);
        return { ok: true };
      case "release":
        for (const key of [...this.refs.keys()]) if (key.startsWith(`${body.operation}/`)) this.refs.delete(key);
        return { ok: true };
      case "journal":
        if (body.entries) {
          this.journal = JSON.parse(JSON.stringify(body.entries));
          return { ok: true };
        }
        return { entries: JSON.parse(JSON.stringify(this.journal)) };
      case "forget":
        return { ok: true };
      default:
        throw new Error(`the fake service has no ${op}`);
    }
  }

  treeIds(tree) {
    return Object.fromEntries([...this.trees.get(tree).entries()].map(([path, entry]) => [path, entry.id]));
  }

  merge(oursCommit, theirsCommit) {
    const baseCommit = this.mergeBase(oursCommit, theirsCommit);
    const base = baseCommit ? this.flat(baseCommit) : new Map();
    const ours = this.flat(oursCommit);
    const theirs = this.flat(theirsCommit);
    const merged = new Map();
    const conflicts = [];
    for (const path of new Set([...base.keys(), ...ours.keys(), ...theirs.keys()])) {
      const b = base.get(path)?.id ?? null;
      const o = ours.get(path)?.id ?? null;
      const t = theirs.get(path)?.id ?? null;
      let pick;
      if (o === t) pick = o;
      else if (b === o) pick = t;
      else if (b === t) pick = o;
      else {
        const text = b && o && t ? merge3(this.text(b), this.text(o), this.text(t)) : null;
        if (text === null) {
          conflicts.push({ path, base: b, ours: o, theirs: t, mode: "100644" });
          pick = o;
        } else {
          pick = this.putBlob(enc.encode(text));
        }
      }
      if (pick) merged.set(path, { id: pick, mode: "100644" });
    }
    const tree = this.makeTree(merged);
    return conflicts.length ? { clean: false, tree, conflicts } : { clean: true, tree };
  }
}

// ---------------------------------------------------------------- fake project and server

function makeSession() {
  const doc = new LoroDoc();
  const entries = new Map();
  const assets = new Map();
  let next = 0;
  return {
    doc,
    assets,
    list: () => [...entries.entries()].map(([path, entry]) => ({ id: entry.id, path, kind: entry.kind, sha: entry.sha })),
    async setText(path, text) {
      let entry = entries.get(path);
      if (!entry || entry.kind !== "text") {
        next += 1;
        entry = { id: `f${next}`, kind: "text" };
        entries.set(path, entry);
        doc.getMap("files").setContainer(entry.id, new LoroText());
      }
      const body = doc.getMap("files").get(entry.id);
      body.delete(0, body.length);
      body.insert(0, text);
      doc.commit();
    },
    async setAsset(path, bytes) {
      const sha = await sha256Hex(bytes);
      assets.set(sha, bytes);
      next += 1;
      entries.set(path, { id: `f${next}`, kind: "asset", sha });
    },
    remove: (path) => entries.delete(path),
    text(path) {
      const entry = entries.get(path);
      return entry?.kind === "text" ? unsuggestedText(doc, entry.id) : null;
    },
    has: (path) => entries.has(path),
    sha: (path) => entries.get(path)?.sha ?? null,
    paths: () => [...entries.keys()].sort(),
  };
}

function makeApi(session) {
  const state = { link: null, calls: [], beforeApply: null };
  const stale = () => httpError(409, { reason: "stale", error: "stale", link: state.link });
  const api = {
    state,
    getLink: async () => ({ link: state.link ? structuredClone(state.link) : null }),
    async putLink(body) {
      if (state.link ? body.revision !== state.link.revision : body.revision !== null) throw stale();
      state.link = {
        remote_kind: body.remote_kind,
        remote_label: body.remote_label,
        connection_id: body.connection_id,
        branch: body.branch,
        folder: body.folder,
        checkpoint: body.checkpoint,
        synced_paths: body.synced_paths,
        revision: (state.link?.revision ?? 0) + 1,
      };
      return { link: structuredClone(state.link) };
    },
    async deleteLink() {
      state.link = null;
    },
    async uploadAsset(bytes) {
      session.assets.set(await sha256Hex(bytes), bytes);
      return {};
    },
    async assetBytes(sha) {
      return session.assets.get(sha);
    },
    async sourceChange(body) {
      state.calls.push(body);
      if (state.beforeApply) {
        const hook = state.beforeApply;
        state.beforeApply = null;
        await hook();
      }
      if (body.sync.revision !== state.link.revision) throw stale();
      const changed = [];
      for (const dependency of body.change.dependencies) {
        const text = session.text(dependency.path);
        if (text === null || (await sha256HexOfText(text)) !== dependency.file_hash) changed.push(dependency.path);
      }
      if (changed.length) throw httpError(409, { reason: "changed", files: changed });
      const { change } = body;
      for (const { path, text } of change.create) await session.setText(path, text);
      const byPath = new Map();
      for (const patch of change.patches) byPath.set(patch.path, [...(byPath.get(patch.path) || []), patch]);
      for (const [path, patches] of byPath) {
        let bytes = enc.encode(session.text(path));
        for (const patch of patches.sort((a, b) => b.start - a.start)) {
          assert.equal(dec.decode(bytes.slice(patch.start, patch.end)), patch.exact);
          bytes = new Uint8Array([...bytes.slice(0, patch.start), ...enc.encode(patch.replacement), ...bytes.slice(patch.end)]);
        }
        await session.setText(path, dec.decode(bytes));
      }
      for (const { path, expect } of change.delete) {
        if (expect !== undefined) assert.equal(session.sha(path), expect);
        session.remove(path);
      }
      for (const { path, hash, expect } of change.put_binary) {
        assert.equal(session.sha(path), expect);
        await session.setAsset(path, session.assets.get(hash));
      }
      state.link = {
        ...state.link,
        checkpoint: body.sync.checkpoint,
        synced_paths: body.sync.synced_paths,
        revision: state.link.revision + 1,
      };
      return { receipt: {}, link: structuredClone(state.link) };
    },
  };
  return api;
}

const rules = {
  text_extensions: [".md", ".qmd", ".bib", ".csv"],
  asset_extensions: [".png"],
  derived_extensions: [],
  max_path: 80,
  max_files: 50,
};

async function world({ remote = {}, project = {}, assets = {} }) {
  const repo = new Repo();
  if (Object.keys(remote).length) repo.advance(remote, "initial");
  const session = makeSession();
  for (const [path, text] of Object.entries(project)) await session.setText(path, text);
  for (const [path, bytes] of Object.entries(assets)) await session.setAsset(path, bytes);
  const api = makeApi(session);
  const ctx = {
    slug: "doc",
    session,
    rules,
    git: createBridge((op, body) => repo.request(op, body)),
    api,
    me: { name: "Ann", handle: "ann" },
    arrivalMs: 300,
  };
  return { repo, session, api, ctx };
}

const draft = (folder = "") => ({
  remote: { url: "https://example.test/r.git" },
  label: "example.test/r.git",
  branch: "main",
  folder,
});

/// A connected world: the project and the remote hold the same files.
async function connected(files, folder = "") {
  const prefixed = Object.fromEntries(Object.entries(files).map(([path, text]) => [folder ? `${folder}/${path}` : path, text]));
  const made = await world({ remote: prefixed, project: files });
  const answer = await sync.connect(made.ctx, draft(folder), "remote");
  assert.ok(["pulled", "nothing"].includes(answer.status), JSON.stringify(answer));
  return made;
}

const baseFiles = (repo) => {
  const call = repo.commitCalls.find((item) => item.message === "LibrePaper first connection base");
  return Object.fromEntries(Object.entries(call.files).map(([path, id]) => [path, repo.text(id)]));
};

// ---------------------------------------------------------------- 1. first connection

{
  const fresh = async () => {
    const made = await world({
      remote: {
        "paper/intro.qmd": "repo intro\n",
        "paper/refs.bib": "@a\n",
        "paper/fig/a.png": new Uint8Array([1, 2, 3]),
        "README.md": "outside\n",
      },
      project: { "intro.qmd": "lp intro\n", "notes.md": "notes\n" },
      assets: { "logo.png": new Uint8Array([9, 9]) },
    });
    return made;
  };

  const compared = await fresh();
  const look = await sync.compare(compared.ctx, draft("paper"));
  assert.deepEqual(look.incoming, ["fig/a.png", "refs.bib"]);
  assert.deepEqual(look.outgoing, ["logo.png", "notes.md"]);
  assert.deepEqual(look.differing, ["intro.qmd"]);
  assert.ok(look.unsynced.some((item) => item.path === "README.md"));
  assert.deepEqual(look.refusals, []);
  assert.equal(compared.api.state.link, null);

  // Keep the repository's copies.
  const remoteSide = await fresh();
  const pulled = await sync.connect(remoteSide.ctx, draft("paper"), "remote");
  assert.equal(pulled.status, "pulled");
  assert.deepEqual(baseFiles(remoteSide.repo), { "intro.qmd": "lp intro\n" });
  const [ours, theirs] = [remoteSide.repo.mergeCalls[0].ours, remoteSide.repo.mergeCalls[0].theirs];
  const parentOf = (id) => remoteSide.repo.commits.get(id).parents[0];
  assert.equal(parentOf(ours), parentOf(theirs));
  assert.equal(remoteSide.repo.commits.get(parentOf(ours)).message, "LibrePaper first connection base");
  assert.equal(remoteSide.session.text("intro.qmd"), "repo intro\n");
  assert.equal(remoteSide.session.text("refs.bib"), "@a\n");
  assert.deepEqual(remoteSide.session.assets.get(remoteSide.session.sha("fig/a.png")), new Uint8Array([1, 2, 3]));
  assert.equal(remoteSide.session.text("notes.md"), "notes\n");
  const link = remoteSide.api.state.link;
  assert.equal(link.checkpoint, remoteSide.repo.head);
  assert.deepEqual(link.synced_paths, ["fig/a.png", "intro.qmd", "refs.bib"]);
  assert.equal(link.folder, "paper");
  assert.equal(remoteSide.repo.pushes.length, 0);

  // Keep LibrePaper's copies, and push them.
  const localSide = await fresh();
  const before = localSide.repo.head;
  const pushed = await sync.connect(localSide.ctx, draft("paper"), "local");
  assert.equal(pushed.status, "pushed", JSON.stringify(pushed));
  assert.deepEqual(baseFiles(localSide.repo), { "intro.qmd": "repo intro\n" });
  assert.equal(localSide.session.text("intro.qmd"), "lp intro\n");
  const files = localSide.repo.files();
  assert.equal(files["paper/intro.qmd"], "lp intro\n");
  assert.equal(files["paper/notes.md"], "notes\n");
  assert.equal(files["paper/refs.bib"], "@a\n");
  assert.equal(files["README.md"], "outside\n");
  assert.ok(files["paper/logo.png"] !== undefined);
  assert.equal(localSide.repo.commits.get(localSide.repo.head).parents[0], before);
  assert.equal(localSide.repo.commits.get(localSide.repo.head).author.email, "ann@users.noreply.example.test");
  assert.equal(localSide.api.state.link.checkpoint, localSide.repo.head);
  assert.deepEqual(localSide.repo.journal, []);
  assert.equal(localSide.repo.refs.size, 0);
}

// ---------------------------------------------------------------- 2. independent edits merge

{
  const { repo, session, api, ctx } = await connected({ "intro.qmd": "p1\nq1\np2\n" });
  await session.setText("intro.qmd", "P1 edited\nq1\np2\n");
  const checkpoint = api.state.link.checkpoint;
  repo.advance({ "intro.qmd": "p1\nq1\nP2 edited\n" });
  const answer = await sync.pull(ctx);
  assert.equal(answer.status, "pulled");
  assert.equal(session.text("intro.qmd"), "P1 edited\nq1\nP2 edited\n");
  const body = api.state.calls.at(-1);
  assert.equal(body.sync.checkpoint, repo.head);
  assert.notEqual(checkpoint, repo.head);
  assert.equal(body.change.dependencies.length, 1);
  assert.equal(body.change.dependencies[0].file_hash, await sha256HexOfText("P1 edited\nq1\np2\n"));
  assert.ok(body.change.patches.length >= 1);
  const edge = enc.encode("P1 edited\nq1\n").length;
  assert.ok(body.change.patches.every((patch) => patch.path === "intro.qmd" && patch.start >= edge));
}

// ---------------------------------------------------------------- 3. overlapping edits

{
  const { repo, session, api, ctx } = await connected({ "intro.qmd": "same sentence\n" });
  await session.setText("intro.qmd", "local sentence\n");
  repo.advance({ "intro.qmd": "remote sentence\n" });
  const calls = api.state.calls.length;
  const answer = await sync.pull(ctx);
  assert.equal(answer.status, "conflict");
  assert.equal(api.state.calls.length, calls);
  assert.equal(session.text("intro.qmd"), "local sentence\n");
  const [conflict] = answer.session.conflicts;
  assert.equal(conflict.path, "intro.qmd");
  assert.equal(conflict.kind, "text");
  assert.equal(repo.text(conflict.base), "same sentence\n");
  assert.equal(repo.text(conflict.ours), "local sentence\n");
  assert.equal(repo.text(conflict.theirs), "remote sentence\n");
  assert.equal(repo.journal.filter((entry) => entry.kind === "conflict").length, 1);
  assert.ok(repo.refs.size >= 3);

  // The remote moves elsewhere while the resolver is open: the merge is made
  // again and the resolution is carried over, since its blobs are unchanged.
  const resolution = { "intro.qmd": { text: "resolved sentence\n" } };
  repo.advance({ "other.md": "other\n" });
  const again = await sync.applyResolution(ctx, answer.session, resolution);
  assert.equal(again.status, "conflict");
  assert.deepEqual(again.session.carried, resolution);
  assert.equal(repo.journal.filter((entry) => entry.kind === "conflict").length, 1);
  assert.equal(api.state.calls.length, calls);

  const done = await sync.applyResolution(ctx, again.session, again.session.carried);
  assert.equal(done.status, "pulled", JSON.stringify(done));
  assert.equal(session.text("intro.qmd"), "resolved sentence\n");
  assert.equal(session.text("other.md"), "other\n");
  assert.equal(api.state.link.checkpoint, repo.head);
  assert.deepEqual(repo.journal, []);
  assert.equal(repo.refs.size, 0);
}

// ---------------------------------------------------------------- 4. typing during a pull

{
  const { repo, session, api, ctx } = await connected({ "intro.qmd": "one\ntwo\nthree\n" });
  repo.advance({ "intro.qmd": "one\ntwo\nthree!\n" });
  api.state.beforeApply = () => session.setText("intro.qmd", "ONE\ntwo\nthree\n");
  const calls = api.state.calls.length;
  const answer = await sync.pull(ctx);
  assert.equal(answer.status, "pulled", JSON.stringify(answer));
  assert.equal(api.state.calls.length, calls + 2);
  assert.equal(session.text("intro.qmd"), "ONE\ntwo\nthree!\n");
  assert.equal(api.state.link.checkpoint, repo.head);
  assert.deepEqual(repo.journal, []);

  // A file that keeps changing is refused after three tries.
  repo.advance({ "intro.qmd": "one\ntwo\nthree!!\n" });
  let n = 0;
  const original = api.sourceChange;
  api.sourceChange = async (body) => {
    n += 1;
    await session.setText("intro.qmd", `ONE ${n}\ntwo\nthree!\n`);
    return original(body);
  };
  const refused = await sync.pull(ctx);
  api.sourceChange = original;
  assert.equal(refused.status, "refused");
  assert.deepEqual(refused.refusals[0].paths, ["intro.qmd"]);
  assert.equal(n, 3);
}

// ---------------------------------------------------------------- 5. rules change between syncs

{
  const made = await world({
    remote: {
      "intro.qmd": "intro\n",
      "refs.bib": "@a\n",
      "data/a.csv": "x\n",
      "analysis.jl": "println(1)\n",
    },
    project: { "intro.qmd": "intro\n", "refs.bib": "@a\n", "data/a.csv": "x\n" },
  });
  const { repo, session, api, ctx } = made;
  const first = await sync.connect(ctx, draft(), "remote");
  assert.equal(first.status, "nothing", JSON.stringify(first));
  assert.deepEqual(api.state.link.synced_paths, ["data/a.csv", "intro.qmd", "refs.bib"]);
  assert.ok(!session.has("analysis.jl"));

  // The deployment adds .jl. The file reads as a repository addition and is
  // created, not deleted from the repository by the next push.
  ctx.rules = { ...rules, text_extensions: [...rules.text_extensions, ".jl"] };
  await session.setText("intro.qmd", "intro edited\n");
  const pushed = await sync.push(ctx, "edit intro");
  assert.equal(pushed.status, "pushed", JSON.stringify(pushed));
  assert.equal(session.text("analysis.jl"), "println(1)\n");
  assert.equal(repo.read("analysis.jl"), "println(1)\n");
  assert.equal(repo.read("intro.qmd"), "intro edited\n");
  assert.ok(api.state.link.synced_paths.includes("analysis.jl"));
  const edit = repo.treeEdits.at(-1);
  assert.deepEqual(edit.edits.map((item) => item.path), ["intro.qmd"]);

  // A new ignore line removes data/a.csv from every input. It stays on both
  // sides and leaves the synced paths.
  repo.advance({ ".librepaperignore": "data/\n" });
  const ignored = await sync.pull(ctx);
  assert.ok(["pulled", "nothing"].includes(ignored.status), JSON.stringify(ignored));
  assert.ok(session.has("data/a.csv"));
  assert.equal(repo.read("data/a.csv"), "x\n");
  assert.ok(!api.state.link.synced_paths.includes("data/a.csv"));
  const body = api.state.calls.at(-1);
  assert.deepEqual(body.change.delete, []);
}

// ---------------------------------------------------------------- 6. a CRLF file

{
  const made = await world({
    remote: { "refs.bib": "a\r\nb\r\n", "intro.qmd": "x\n" },
    project: { "refs.bib": "a\nb\n", "intro.qmd": "x\n" },
  });
  const { repo, session, ctx } = made;
  const connectedAnswer = await sync.connect(ctx, draft(), "remote");
  assert.equal(connectedAnswer.status, "nothing", JSON.stringify(connectedAnswer));
  const crlfId = repo.idOf("refs.bib");

  await session.setText("intro.qmd", "x\ny\n");
  const pushed = await sync.push(ctx, "");
  assert.equal(pushed.status, "pushed", JSON.stringify(pushed));
  assert.equal(repo.idOf("refs.bib"), crlfId);
  assert.equal(repo.read("refs.bib"), "a\r\nb\r\n");
  assert.equal(repo.read("intro.qmd"), "x\ny\n");
  assert.deepEqual(repo.treeEdits.at(-1).edits.map((item) => item.path), ["intro.qmd"]);
  assert.equal(repo.commits.get(repo.head).message, "Update intro.qmd");

  // Edited in LibrePaper, the file is written with LF.
  await session.setText("refs.bib", "a\nb\nc\n");
  const second = await sync.push(ctx, "bib");
  assert.equal(second.status, "pushed");
  assert.equal(repo.read("refs.bib"), "a\nb\nc\n");
}

// ---------------------------------------------------------------- 7. the remote moves during a push

{
  const { repo, session, api, ctx } = await connected({ "intro.qmd": "one\n" });
  await session.setText("intro.qmd", "one edited\n");
  const planned = await sync.outgoing(ctx);
  assert.equal(planned.status, "outgoing", JSON.stringify(planned));
  assert.deepEqual(planned.files, [{ path: "intro.qmd", status: "M" }]);
  assert.equal(repo.pushes.length, 0);

  // The reviewed set is what goes out, even when the remote moved elsewhere.
  repo.advance({ "other.md": "other\n" });
  const pushed = await sync.push(ctx, "reviewed", { confirmed: planned.outgoing });
  assert.equal(pushed.status, "pushed", JSON.stringify(pushed));
  assert.equal(repo.read("intro.qmd"), "one edited\n");
  assert.equal(repo.read("other.md"), "other\n");
  assert.equal(session.text("other.md"), "other\n");
  assert.equal(api.state.link.checkpoint, repo.head);

  // Moved twice in a row: the panel is asked again and nothing is pushed.
  await session.setText("intro.qmd", "one edited twice\n");
  repo.moveOnPush = 2;
  const head = repo.head;
  const moved = await sync.push(ctx, "again");
  assert.equal(moved.status, "moved", JSON.stringify(moved));
  assert.deepEqual(moved.files, [{ path: "intro.qmd", status: "M" }]);
  assert.equal(repo.read("intro.qmd"), "one edited\n");
  assert.notEqual(repo.head, head);
  assert.equal(repo.refs.size, 0);

  // The panel asks again and the push goes through.
  const final = await sync.push(ctx, "again", { confirmed: moved.outgoing });
  assert.equal(final.status, "pushed", JSON.stringify(final));
  assert.equal(repo.read("intro.qmd"), "one edited twice\n");
}

// ---------------------------------------------------------------- recovery

{
  const { repo, session, api, ctx } = await connected({ "intro.qmd": "one\n" });
  await session.setText("intro.qmd", "two\n");
  // A push that reached the remote but never wrote the record.
  const planned = await sync.outgoing(ctx);
  const tree = await ctx.git.treeEdit(planned.outgoing.remoteTree, planned.outgoing.edits);
  const commit = await ctx.git.commitTree(tree, [planned.outgoing.remote], "lost", { name: "Ann", email: "a@x" });
  repo.head = commit;
  await ctx.git.writeJournal([
    { id: "op-lost", kind: "push", phase: "prepared", commit, parent: planned.outgoing.remote, paths: planned.outgoing.paths },
  ]);
  const settled = await sync.recover(ctx);
  assert.deepEqual(settled.recovered, ["op-lost"]);
  assert.equal(api.state.link.checkpoint, commit);
  assert.deepEqual(repo.journal, []);
}

console.log("gitsync sync ok");
