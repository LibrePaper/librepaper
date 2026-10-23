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
const dec = new TextDecoder("utf-8", { ignoreBOM: true });
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
  pushBodies = [];
  moveOnPush = 0;
  invalidNames = new Set();
  connectionId = null;
  forgets = 0;

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
      else if (content?.mode) map.set(path, { id: this.putBlob(bytesOf(content.content)), mode: content.mode });
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
        return { git: "2.43.0", supported: true, connected: !!this.connectionId, connection_id: this.connectionId };
      case "connect":
        this.connectionId = body.connection_id;
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
            ...(this.invalidNames.has(path) ? { invalid_name: true } : {}),
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
        this.pushBodies.push(body);
        if (body.connection_id !== this.connectionId) {
          throw httpError(409, { error: "Another connection.", kind: "stale-connection" }, { kind: "stale-connection" });
        }
        if ((body.expected ?? null) !== this.head) {
          throw httpError(409, { error: "The branch moved.", kind: "moved" }, { kind: "moved", name: "Refused" });
        }
        if (this.moveOnPush > 0) {
          this.moveOnPush -= 1;
          this.advance({ [`moved-${this.moveOnPush}.md`]: "moved\n" });
        }
        if ((this.commits.get(body.commit).parents[0] ?? null) !== this.head) {
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
        if (body.upsert) {
          this.journal = this.journal.filter((entry) => entry.id !== body.upsert.id);
          this.journal.push(JSON.parse(JSON.stringify(body.upsert)));
          return { ok: true };
        }
        if (body.remove) {
          this.journal = this.journal.filter((entry) => entry.id !== body.remove);
          return { ok: true };
        }
        return { entries: JSON.parse(JSON.stringify(this.journal)) };
      case "forget":
        this.forgets += 1;
        this.connectionId = null;
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
    unsent: 0,
    unacknowledgedCount() {
      return this.unsent;
    },
    has: (path) => entries.has(path),
    sha: (path) => entries.get(path)?.sha ?? null,
    paths: () => [...entries.keys()].sort(),
  };
}

function makeApi(session) {
  const state = { link: null, calls: [], uploads: [], beforeApply: null, version: null };
  const stale = () => httpError(409, { reason: "stale", error: "stale", link: state.link });
  const api = {
    state,
    getLink: async () => ({ link: state.link ? structuredClone(state.link) : null, version: state.version }),
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
      state.uploads.push(await sha256Hex(bytes));
      session.assets.set(await sha256Hex(bytes), bytes);
      return {};
    },
    async hasAsset(sha) {
      return session.assets.has(sha);
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
      if (body.sync.revision !== state.link.revision || body.sync.connection_id !== state.link.connection_id) throw stale();
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
      return { receipt: {}, link: structuredClone(state.link), version: state.version };
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

// ---------------------------------------------------------------- 1b. a branch without commits

{
  const fresh = () => world({ project: { "intro.qmd": "hello\n", "notes.md": "notes\n" } });

  const compared = await fresh();
  const look = await sync.compare(compared.ctx, draft("paper"));
  assert.deepEqual(look.refusals, []);
  assert.deepEqual(look.incoming, []);
  assert.deepEqual(look.differing, []);
  assert.deepEqual(look.outgoing, ["intro.qmd", "notes.md"]);

  const { repo, api, ctx } = await fresh();
  const pushed = await sync.connect(ctx, draft("paper"), "local");
  assert.equal(pushed.status, "pushed", JSON.stringify(pushed));
  assert.equal(repo.pushes.length, 1);
  assert.deepEqual(repo.commits.get(repo.head).parents, []);
  assert.deepEqual(repo.files(), { "paper/intro.qmd": "hello\n", "paper/notes.md": "notes\n" });
  assert.equal(api.state.link.checkpoint, repo.head);
  assert.deepEqual(api.state.link.synced_paths, ["intro.qmd", "notes.md"]);
  assert.equal((await sync.pull(ctx)).status, "nothing");
  assert.deepEqual(repo.journal, []);
  assert.equal(repo.refs.size, 0);

  // Keeping the repository's copies records the link and sends nothing.
  const kept = await fresh();
  const answer = await sync.connect(kept.ctx, draft("paper"), "remote");
  assert.equal(answer.status, "nothing", JSON.stringify(answer));
  assert.equal(kept.api.state.link.checkpoint, null);
  assert.equal(kept.repo.pushes.length, 0);
  const review = await sync.outgoing(kept.ctx);
  assert.equal(review.status, "outgoing");
  assert.deepEqual(review.files.map((file) => file.path), ["intro.qmd", "notes.md"]);
  assert.equal(review.outgoing.remote, null);
  const second = await sync.push(kept.ctx, "First", { confirmed: review.outgoing });
  assert.equal(second.status, "pushed", JSON.stringify(second));
  assert.equal(kept.repo.commits.get(kept.repo.head).message, "First");
  assert.equal(kept.api.state.link.checkpoint, kept.repo.head);
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
  await ctx.git.journalUpsert({
    id: "op-lost",
    kind: "push",
    phase: "prepared",
    commit,
    parent: planned.outgoing.remote,
    paths: planned.outgoing.paths,
    connection_id: api.state.link.connection_id,
  });
  const settled = await sync.recover(ctx);
  assert.deepEqual(settled.recovered, ["op-lost"]);
  assert.equal(api.state.link.checkpoint, commit);
  assert.deepEqual(repo.journal, []);
}

// ---------------------------------------------------------------- Roborev 1023 fixes

// A synced file the remote deleted is deleted in the project and not pushed back.
{
  const { repo, session, api, ctx } = await connected({ "a.md": "a\n", "b.md": "b\n" });
  repo.advance({ "b.md": null });
  const pulled = await sync.pull(ctx);
  assert.equal(pulled.status, "pulled", JSON.stringify(pulled));
  assert.ok(!session.has("b.md"));
  assert.deepEqual(api.state.link.synced_paths, ["a.md"]);
  const pushes = repo.pushes.length;
  await session.setText("a.md", "a edited\n");
  const pushed = await sync.push(ctx, "edit");
  assert.equal(pushed.status, "pushed", JSON.stringify(pushed));
  assert.equal(repo.pushes.length, pushes + 1);
  assert.deepEqual(Object.keys(repo.files()), ["a.md"]);
}

// A project file at a path the remote holds but excludes is never pushed over it.
{
  const symlink = { mode: "120000", content: "target.md" };
  const { repo, session, ctx } = await world({
    remote: { "intro.qmd": "intro\n", "notes.md": symlink },
    project: { "intro.qmd": "intro\n", "notes.md": "local notes\n" },
  }).then(async (made) => {
    const answer = await sync.connect(made.ctx, draft(), "remote");
    assert.ok(["pulled", "nothing"].includes(answer.status), JSON.stringify(answer));
    return made;
  });
  await session.setText("intro.qmd", "intro edited\n");
  const look = await sync.outgoing(ctx);
  assert.deepEqual(look.files, [{ path: "intro.qmd", status: "M" }]);
  const pushed = await sync.push(ctx, "edit");
  assert.equal(pushed.status, "pushed", JSON.stringify(pushed));
  assert.equal(repo.read("notes.md"), "target.md");
  assert.equal(repo.flat(repo.head).get("notes.md").mode, "120000");
  assert.equal(session.text("notes.md"), "local notes\n");

  // The same for a file over the size limit.
  const big = await world({
    remote: { "notes.md": "x".repeat(40) },
    project: { "notes.md": "local\n", "intro.qmd": "i\n" },
  });
  big.ctx.maxFileBytes = 10;
  const first = await sync.connect(big.ctx, draft(), "local");
  assert.ok(["pushed", "nothing", "pulled"].includes(first.status), JSON.stringify(first));
  assert.equal(big.repo.read("notes.md"), "x".repeat(40));
  assert.equal(big.repo.read("intro.qmd"), "i\n");
}

// compare and connect refuse when the server already holds a link, and leave the sidecar alone.
{
  const { repo, ctx } = await connected({ "intro.qmd": "one\n" });
  const forgets = repo.forgets;
  const connections = repo.connectionId;
  const look = await sync.compare(ctx, draft());
  assert.equal(look.status, "refused");
  const again = await sync.connect(ctx, draft(), "remote");
  assert.equal(again.status, "refused");
  assert.equal(repo.connectionId, connections);
  assert.equal(repo.forgets, forgets);
}

// A failure after the draft is connected forgets the draft; the label comes from the sidecar.
{
  const { repo, api, ctx } = await world({ remote: { "intro.qmd": "x\n" }, project: {} });
  const original = repo.request.bind(repo);
  repo.request = async (op, body) => {
    if (op === "fetch") throw new Error("boom");
    return original(op, body);
  };
  await assert.rejects(() => sync.compare(ctx, { ...draft(), label: "https://user:secret@example.test/r.git" }), /boom/);
  assert.equal(repo.connectionId, null);
  assert.equal(repo.forgets, 1);
  repo.request = original;
  const answer = await sync.connect(ctx, { ...draft(), label: "https://raw.example/url" }, "remote");
  assert.ok(["pulled", "nothing"].includes(answer.status), JSON.stringify(answer));
  assert.equal(api.state.link.remote_label, "example.test/r.git");
}

// A sidecar connected to another repository is refused with reconnect.
{
  const { repo, api, ctx } = await connected({ "intro.qmd": "one\n" });
  repo.advance({ "intro.qmd": "two\n" });
  repo.connectionId = "other-connection";
  const calls = api.state.calls.length;
  for (const answer of [await sync.pull(ctx), await sync.outgoing(ctx), await sync.push(ctx, "x")]) {
    assert.equal(answer.status, "refused");
    assert.equal(answer.reconnect, true);
  }
  assert.equal((await sync.applyResolution(ctx, { id: "x", conflicts: [] }, {})).reconnect, true);
  assert.equal(api.state.calls.length, calls);
  repo.connectionId = api.state.link.connection_id;
  assert.equal((await sync.pull(ctx)).status, "pulled");
  assert.equal(api.state.calls.at(-1).sync.connection_id, api.state.link.connection_id);
}

// Resolutions are saved on the conflict session and come back from status.
{
  const { repo, session, ctx } = await connected({ "intro.qmd": "same\n" });
  await session.setText("intro.qmd", "local\n");
  repo.advance({ "intro.qmd": "remote\n" });
  const conflict = await sync.pull(ctx);
  assert.equal(conflict.status, "conflict");
  const resolutions = { "intro.qmd": { text: "both\n" } };
  await sync.saveResolutions(ctx, conflict.session, resolutions);
  assert.equal(repo.journal.filter((entry) => entry.kind === "conflict").length, 1);
  const seen = await sync.status(ctx);
  assert.deepEqual(seen.conflict.resolutions, resolutions);
  const done = await sync.applyResolution(ctx, seen.conflict);
  assert.equal(done.status, "pulled", JSON.stringify(done));
  assert.equal(session.text("intro.qmd"), "both\n");
}

// A resent pull is settled only by a clear answer.
{
  const { repo, api, ctx } = await connected({ "intro.qmd": "one\n" });
  const body = { operation: "v2.1.aa", change: {}, sync: { revision: 1 } };
  await ctx.git.journalUpsert({ id: "op-sent", kind: "pull", phase: "sent", revision: 1, checkpoint: null, body });
  await ctx.git.keep("op-sent", { base: repo.head });
  const original = api.sourceChange;
  api.sourceChange = async () => {
    throw httpError(503, { error: "down" });
  };
  const first = await sync.recover(ctx);
  assert.deepEqual(first.pending, ["op-sent"]);
  assert.deepEqual(first.recovered, []);
  assert.equal(repo.journal.length, 1);
  assert.ok(repo.refs.has("op-sent/base"));
  api.sourceChange = async () => {
    throw httpError(409, { reason: "stale" });
  };
  const second = await sync.recover(ctx);
  assert.deepEqual(second.recovered, ["op-sent"]);
  assert.deepEqual(repo.journal, []);
  api.sourceChange = original;
}

// An LFS-managed asset is excluded from every input.
{
  const pointer = "version https://git-lfs.github.com/spec/v1\noid sha256:abc\nsize 123\n";
  const { repo, session, ctx } = await world({
    remote: { "intro.qmd": "intro\n", "fig/a.png": pointer },
    project: { "intro.qmd": "intro\n" },
  });
  const look = await sync.compare(ctx, draft());
  assert.deepEqual(look.incoming, []);
  const answer = await sync.connect(ctx, draft(), "remote");
  assert.ok(["pulled", "nothing"].includes(answer.status), JSON.stringify(answer));
  assert.deepEqual(answer.skipped, [{ path: "fig/a.png", reason: "a Git LFS pointer" }]);
  assert.ok(!session.has("fig/a.png"));
  assert.equal(repo.read("fig/a.png"), pointer);
}

// ---------------------------------------------------------------- Roborev 1024 fixes

// A page that has not received every server change refuses outgoing work.
{
  const { repo, session, api, ctx } = await connected({ "intro.qmd": "one\n" });
  await session.setText("intro.qmd", "one edited\n");
  const other = new LoroDoc();
  other.getText("elsewhere").insert(0, "typed on another desktop");
  other.commit();
  api.state.version = Buffer.from(other.oplogVersion().encode()).toString("base64");
  ctx.arrivalMs = 150;
  const behind = await sync.outgoing(ctx);
  assert.equal(behind.status, "refused", JSON.stringify(behind));
  assert.equal(behind.refusals[0].reason, "This page has not received every change yet. Try again in a moment.");
  const pushes = repo.pushes.length;
  assert.equal((await sync.push(ctx, "x")).status, "refused");
  assert.equal(repo.pushes.length, pushes);

  // Unsent local updates refuse as well.
  session.doc.import(other.export({ mode: "update" }));
  session.unsent = 1;
  assert.equal((await sync.outgoing(ctx)).status, "refused");
  session.unsent = 0;
  const ready = await sync.outgoing(ctx);
  assert.equal(ready.status, "outgoing", JSON.stringify(ready));
}

// A confirmed push is bound to the connection it was reviewed against.
{
  const { repo, session, api, ctx } = await connected({ "intro.qmd": "one\n" });
  await session.setText("intro.qmd", "one edited\n");
  const planned = await sync.outgoing(ctx);
  assert.equal(planned.outgoing.connection_id, api.state.link.connection_id);
  api.state.link.connection_id = "second-connection";
  repo.connectionId = "second-connection";
  const pushes = repo.pushes.length;
  const again = await sync.push(ctx, "x", { confirmed: planned.outgoing });
  assert.equal(again.status, "moved", JSON.stringify(again));
  assert.equal(again.outgoing.connection_id, "second-connection");
  assert.equal(repo.pushes.length, pushes);
  const sent = await sync.push(ctx, "x", { confirmed: again.outgoing });
  assert.equal(sent.status, "pushed", JSON.stringify(sent));
  assert.equal(repo.pushBodies.at(-1).expected, planned.outgoing.remote);
  assert.equal(repo.pushBodies.at(-1).connection_id, "second-connection");

  // The sidecar's own refusal of a changed connection is a reconnect refusal.
  await session.setText("intro.qmd", "one edited twice\n");
  const original = repo.request.bind(repo);
  repo.request = async (op, body) => {
    if (op === "push") throw httpError(409, { kind: "stale-connection" }, { kind: "stale-connection" });
    return original(op, body);
  };
  const refusal = await sync.push(ctx, "x");
  repo.request = original;
  assert.equal(refusal.status, "refused");
  assert.equal(refusal.reconnect, true);
  assert.deepEqual(repo.journal, []);
}

// A journal entry is never reconciled into a link of another connection.
{
  const { repo, api, ctx } = await connected({ "intro.qmd": "one\n" });
  const parent = repo.head;
  const commit = repo.advance({ "intro.qmd": "two\n" });
  await ctx.git.journalUpsert({
    id: "op-other",
    kind: "push",
    phase: "pushed",
    commit,
    parent,
    paths: ["intro.qmd"],
    connection_id: "an-older-connection",
  });
  const checkpoint = api.state.link.checkpoint;
  const settled = await sync.recover(ctx);
  assert.deepEqual(settled.pending, ["op-other"]);
  assert.deepEqual(settled.recovered, []);
  assert.equal(api.state.link.checkpoint, checkpoint);
  assert.equal(repo.journal.length, 1);
}

// A decomposed name in the repository is found when the baseline is rebuilt.
{
  const decomposed = "café.md";
  const composed = "café.md";
  const { repo, session, api, ctx } = await connected({ [composed]: "coffee\n", "intro.qmd": "i\n" });
  // The repository spells the name in the decomposed form from the start.
  repo.advance({ [composed]: null, [decomposed]: "coffee\n" });
  const pulledFirst = await sync.pull(ctx);
  assert.ok(["pulled", "nothing"].includes(pulledFirst.status), JSON.stringify(pulledFirst));
  assert.deepEqual(repo.read(decomposed), "coffee\n");
  await session.setText(composed, "coffee edited\n");
  repo.advance({ [decomposed]: null });
  const answer = await sync.pull(ctx);
  // Edited locally, deleted remotely: a conflict, not a silent re-creation.
  assert.equal(answer.status, "conflict", JSON.stringify(answer));
  assert.equal(answer.conflicts[0].path, composed);
  assert.equal(repo.text(answer.conflicts[0].base), "coffee\n");
  assert.ok(api.state.link);
}

// An error that leaves the outcome unknown keeps the entry and its refs.
{
  const { repo, session, ctx } = await connected({ "intro.qmd": "one\n" });
  await session.setText("intro.qmd", "one edited\n");
  const original = repo.request.bind(repo);
  repo.request = async (op, body) => {
    if (op === "push") throw httpError(502, { error: "bad gateway" });
    return original(op, body);
  };
  await assert.rejects(() => sync.push(ctx, "x"), /bad gateway/);
  repo.request = original;
  const [entry] = repo.journal;
  assert.equal(entry.kind, "push");
  assert.equal(entry.phase, "prepared");
  assert.ok(repo.refs.has(`${entry.id}/published`));
}
{
  const { repo, api, ctx } = await connected({ "intro.qmd": "one\n" });
  repo.advance({ "intro.qmd": "two\n" });
  api.sourceChange = async () => {
    throw httpError(503, { error: "down" });
  };
  await assert.rejects(() => sync.pull(ctx), /down/);
  assert.equal(repo.journal.filter((entry) => entry.kind === "pull" && entry.phase === "sent").length, 1);
  assert.ok([...repo.refs.keys()].some((key) => key.endsWith("/merged")));
}

// A local file over the size limit is not sent, whatever its kind.
{
  const { repo, session, ctx } = await connected({ "intro.qmd": "one\n" });
  ctx.maxFileBytes = 20;
  await session.setText("big.md", "x".repeat(40));
  await session.setAsset("fig.png", new Uint8Array(40).fill(7));
  await session.setText("small.md", "small\n");
  const look = await sync.outgoing(ctx);
  assert.equal(look.status, "outgoing", JSON.stringify(look));
  assert.deepEqual(look.files.map((file) => file.path), ["small.md"]);
  const pushed = await sync.push(ctx, "x", { confirmed: look.outgoing });
  assert.equal(pushed.status, "pushed", JSON.stringify(pushed));
  assert.deepEqual(Object.keys(repo.files()).sort(), ["intro.qmd", "small.md"]);

  const compared = await world({ project: { "big.md": "x".repeat(40), "ok.md": "ok\n" } });
  compared.ctx.maxFileBytes = 20;
  const result = await sync.compare(compared.ctx, draft());
  assert.deepEqual(result.outgoing, ["ok.md"]);
  assert.ok(result.unsynced.some((item) => item.path === "big.md" && item.reason === "over the file size limit"));
}

// Files left out for their content do not count against the file limit.
{
  const pointer = "version https://git-lfs.github.com/spec/v1\noid sha256:abc\nsize 123\n";
  const remote = { "real-a.md": "a\n", "real-b.md": "b\n" };
  for (let i = 0; i < 60; i += 1) remote[`lfs-${i}.md`] = pointer;
  const { session, ctx } = await world({ remote, project: {} });
  const answer = await sync.connect(ctx, draft(), "remote");
  assert.ok(["pulled", "nothing"].includes(answer.status), JSON.stringify(answer));
  assert.equal(session.text("real-a.md"), "a\n");

  const many = {};
  for (let i = 0; i < 60; i += 1) many[`f-${i}.md`] = "x\n";
  const refused = await world({ remote: many, project: {} });
  const answered = await sync.connect(refused.ctx, draft(), "remote");
  assert.equal(answered.status, "refused");
  assert.match(answered.refusals[0].reason, /60 syncable files/);
}

// A synced file that grows over the size limit stays on both sides: it is not
// pushed, and its absence from L is not a deletion in either direction.
{
  const { repo, session, ctx } = await connected({ "intro.qmd": "one\n", "notes.md": "short\n" });
  ctx.maxFileBytes = 20;
  await session.setText("notes.md", "x".repeat(40));
  await session.setText("intro.qmd", "two\n");
  const look = await sync.outgoing(ctx);
  assert.equal(look.status, "outgoing", JSON.stringify(look));
  assert.deepEqual(look.files.map((file) => file.path), ["intro.qmd"]);
  const pushed = await sync.push(ctx, "x", { confirmed: look.outgoing });
  assert.equal(pushed.status, "pushed", JSON.stringify(pushed));
  assert.equal(repo.read("notes.md"), "short\n");
  repo.advance({ "intro.qmd": "three\n" });
  const pulled = await sync.pull(ctx);
  assert.equal(pulled.status, "pulled", JSON.stringify(pulled));
  assert.equal(session.text("notes.md"), "x".repeat(40));
  assert.equal(repo.read("notes.md"), "short\n");
}

// ---------------------------------------------------------------- Roborev 1025 fixes

// A synced file with a decomposed repository name that the remote turns into a
// symlink stays in the project, and the remote entry is left alone.
{
  const decomposed = "cafe\u0301.md";
  const composed = "caf\u00e9.md";
  const { repo, session, ctx } = await connected({ [composed]: "coffee\n", "intro.qmd": "i\n" });
  repo.advance({ [composed]: null, [decomposed]: "coffee\n" });
  const renamed = await sync.pull(ctx);
  assert.ok(["pulled", "nothing"].includes(renamed.status), JSON.stringify(renamed));
  repo.advance({ [decomposed]: { mode: "120000", content: "target.md" } });
  const pulled = await sync.pull(ctx);
  assert.ok(["pulled", "nothing"].includes(pulled.status), JSON.stringify(pulled));
  assert.equal(session.text(composed), "coffee\n");
  assert.equal(repo.read(decomposed), "target.md");
  assert.equal(repo.flat(repo.head).get(decomposed).mode, "120000");
}

// An unknown local asset over the size limit is never fetched into the sidecar.
{
  const { repo, session, ctx } = await connected({ "intro.qmd": "one\n" });
  ctx.maxFileBytes = 20;
  const bytes = new Uint8Array(40).fill(5);
  await session.setAsset("fig.png", bytes);
  const sha = session.sha("fig.png");
  const uploaded = [];
  const original = repo.request.bind(repo);
  repo.request = async (op, body) => {
    if (op === "put-blobs") uploaded.push(...body.blobs.map((item) => item.sha256));
    return original(op, body);
  };
  const look = await sync.outgoing(ctx);
  repo.request = original;
  assert.ok(look.status === "nothing" || look.status === "pulled", JSON.stringify(look));
  assert.ok(!uploaded.includes(sha));
  assert.ok(!repo.known.has(sha));
}

// Pulling again keeps the resolutions saved on the older conflict session.
{
  const { repo, session, ctx } = await connected({ "intro.qmd": "same\n" });
  await session.setText("intro.qmd", "local\n");
  repo.advance({ "intro.qmd": "remote\n" });
  const first = await sync.pull(ctx);
  assert.equal(first.status, "conflict");
  const resolutions = { "intro.qmd": { text: "both\n" } };
  await sync.saveResolutions(ctx, first.session, resolutions);
  const second = await sync.pull(ctx);
  assert.equal(second.status, "conflict");
  assert.deepEqual(second.session.carried, resolutions);
  assert.equal(repo.journal.filter((entry) => entry.kind === "conflict").length, 1);
  assert.notEqual(second.session.id, first.session.id);
}

// A prepared push of another connection stays pending before any remote is asked.
{
  const { repo, api, ctx } = await connected({ "intro.qmd": "one\n" });
  const parent = repo.head;
  const commit = repo.advance({ "intro.qmd": "two\n" });
  await ctx.git.journalUpsert({
    id: "op-old",
    kind: "push",
    phase: "prepared",
    commit,
    parent,
    paths: ["intro.qmd"],
    connection_id: "an-older-connection",
  });
  await ctx.git.keep("op-old", { published: commit });
  const asked = [];
  const original = repo.request.bind(repo);
  repo.request = async (op, body) => {
    if (op === "fetch" || op === "ancestor") asked.push(op);
    return original(op, body);
  };
  const checkpoint = api.state.link.checkpoint;
  const settled = await sync.recover(ctx);
  repo.request = original;
  assert.deepEqual(settled.pending, ["op-old"]);
  assert.deepEqual(settled.recovered, []);
  assert.deepEqual(asked, []);
  assert.equal(api.state.link.checkpoint, checkpoint);
  assert.equal(repo.journal.length, 1);
  assert.ok(repo.refs.has("op-old/published"));
}

// A conflict resolution that was sent without a known answer keeps the saved
// session, and a sent pull too large to journal stays pending until a pull ends.
{
  const { repo, session, api, ctx } = await connected({ "intro.qmd": "same\n" });
  await session.setText("intro.qmd", "local\n");
  repo.advance({ "intro.qmd": "remote\n" });
  const conflict = await sync.pull(ctx);
  const resolutions = { "intro.qmd": { text: "both\n" } };
  await sync.saveResolutions(ctx, conflict.session, resolutions);
  const original = api.sourceChange;
  api.sourceChange = async () => {
    throw httpError(503, { error: "down" });
  };
  await assert.rejects(sync.applyResolution(ctx, (await sync.status(ctx)).conflict));
  api.sourceChange = original;
  const kept = repo.journal.find((entry) => entry.kind === "conflict");
  assert.equal(typeof kept.session.resolutions["intro.qmd"].blob, "string");
  assert.deepEqual((await sync.status(ctx)).conflict.resolutions, resolutions);
  assert.ok(repo.journal.some((entry) => entry.kind === "pull" && entry.phase === "sent"));
}
{
  const { repo, session, api, ctx } = await connected({ "intro.qmd": "same\n" });
  await session.setText("intro.qmd", "local\n");
  repo.advance({ "intro.qmd": "remote\n" });
  const conflict = await sync.pull(ctx);
  const resolutions = { "intro.qmd": { text: "both\n" } };
  await sync.saveResolutions(ctx, conflict.session, resolutions);
  await ctx.git.journalUpsert({
    id: "op-big",
    kind: "pull",
    phase: "sent",
    revision: 1,
    connection_id: api.state.link.connection_id,
    checkpoint: null,
    body: null,
  });
  await ctx.git.keep("op-big", { base: repo.head });
  const settled = await sync.recover(ctx);
  assert.deepEqual(settled.pending, ["op-big"]);
  assert.deepEqual(settled.recovered, []);
  assert.match(settled.reasons["op-big"], /Pull again/);
  assert.ok(settled.conflict);
  assert.equal(repo.journal.length, 2);
  assert.ok(repo.refs.has("op-big/base"));
  const done = await sync.applyResolution(ctx, (await sync.status(ctx)).conflict);
  assert.equal(done.status, "pulled", JSON.stringify(done));
  assert.deepEqual(repo.journal, []);
  assert.ok(!repo.refs.has("op-big/base"));
}

// A resolution whose answer was lost, then applied on the server, finishes its
// conflict session when recovery sends it again.
{
  const { repo, session, api, ctx } = await connected({ "intro.qmd": "same\n" });
  await session.setText("intro.qmd", "local\n");
  repo.advance({ "intro.qmd": "remote\n" });
  const conflict = await sync.pull(ctx);
  await sync.saveResolutions(ctx, conflict.session, { "intro.qmd": { text: "both\n" } });
  const original = api.sourceChange;
  api.sourceChange = async (body) => {
    await original(body);
    throw new TypeError("network error");
  };
  await assert.rejects(sync.applyResolution(ctx, (await sync.status(ctx)).conflict));
  // The server remembers the operation id and answers the same request 200.
  api.sourceChange = async () => ({ receipt: {}, link: structuredClone(api.state.link), version: api.state.version });
  assert.ok(repo.journal.some((entry) => entry.kind === "conflict"));
  const settled = await sync.recover(ctx);
  assert.equal(settled.conflict, null);
  assert.equal(settled.pending.length, 0);
  assert.equal(settled.recovered.length, 1);
  assert.ok(!repo.journal.some((entry) => entry.kind === "conflict"));
  assert.deepEqual(repo.journal, []);
  assert.equal((await sync.status(ctx)).conflict, null);
}

// A file that starts with a byte order mark keeps it when its conflict is resolved.
{
  const bom = "\uFEFF";
  const { repo, session, api, ctx } = await connected({ "intro.qmd": `${bom}same\n` });
  await session.setText("intro.qmd", `${bom}local\n`);
  repo.advance({ "intro.qmd": `${bom}remote\n` });
  const conflict = await sync.pull(ctx);
  assert.equal(conflict.status, "conflict");
  const done = await sync.applyResolution(ctx, conflict.session, { "intro.qmd": { text: `${bom}both\n` } });
  assert.equal(done.status, "pulled", JSON.stringify(done));
  assert.equal(session.text("intro.qmd"), `${bom}both\n`);
  const [dependency] = api.state.calls.at(-1).change.dependencies;
  assert.equal(dependency.file_hash, await sha256HexOfText(`${bom}local\n`));
}

// An asset changed in different places on both sides is a conflict, even when
// Git's text merge calls it clean, and nothing is applied.
{
  const bytes = (text) => enc.encode(text);
  const made = await world({
    remote: { "intro.qmd": "same\n", "fig.png": bytes("a\nb\nc\n") },
    project: { "intro.qmd": "same\n" },
    assets: { "fig.png": bytes("a\nb\nc\n") },
  });
  const { repo, session, api, ctx } = made;
  assert.ok(["pulled", "nothing"].includes((await sync.connect(ctx, draft(), "remote")).status));
  await session.setAsset("fig.png", bytes("A\nb\nc\n"));
  repo.advance({ "fig.png": bytes("a\nb\nC\n") });
  const calls = api.state.calls.length;
  const answer = await sync.pull(ctx);
  assert.equal(answer.status, "conflict", JSON.stringify(answer));
  assert.equal(api.state.calls.length, calls);
  const [conflict] = answer.session.conflicts;
  assert.equal(conflict.path, "fig.png");
  assert.equal(conflict.kind, "asset");
  assert.equal(repo.text(conflict.base), "a\nb\nc\n");
  assert.equal(repo.text(conflict.ours), "A\nb\nc\n");
  assert.equal(repo.text(conflict.theirs), "a\nb\nC\n");
  assert.equal(session.sha("fig.png"), await sha256Hex(bytes("A\nb\nc\n")));
}

// A saved session of another connection is refused and left as it is.
{
  const { repo, api, session, ctx } = await connected({ "intro.qmd": "same\n" });
  await session.setText("intro.qmd", "local\n");
  repo.advance({ "intro.qmd": "remote\n" });
  const conflict = await sync.pull(ctx);
  await sync.saveResolutions(ctx, conflict.session, { "intro.qmd": { text: "both\n" } });
  repo.journal.find((entry) => entry.kind === "conflict").session.connection_id = "someone-else";
  const before = JSON.stringify(repo.journal);
  const calls = api.state.calls.length;
  assert.equal((await sync.status(ctx)).conflict, null, "another connection's session is not offered");
  const foreign = repo.journal.find((entry) => entry.kind === "conflict").session;
  const answer = await sync.applyResolution(ctx, foreign);
  assert.equal(answer.status, "refused");
  assert.equal(answer.reconnect, true);
  assert.equal(JSON.stringify(repo.journal), before);
  assert.equal(api.state.calls.length, calls);
  assert.equal(session.text("intro.qmd"), "local\n");
}

// A refused apply keeps the session and its saved resolutions, and returns the session.
{
  const { repo, session, api, ctx } = await connected({ "intro.qmd": "same\n" });
  await session.setText("intro.qmd", "local\n");
  repo.advance({ "intro.qmd": "remote\n" });
  const conflict = await sync.pull(ctx);
  const resolutions = { "intro.qmd": { text: "both\n" } };
  await sync.saveResolutions(ctx, conflict.session, resolutions);
  api.sourceChange = async () => {
    throw httpError(409, { reason: "suggestion", path: "intro.qmd", change: "c1" });
  };
  const answer = await sync.applyResolution(ctx, (await sync.status(ctx)).conflict);
  assert.equal(answer.status, "refused");
  assert.equal(answer.session.id, conflict.session.id);
  assert.deepEqual(answer.session.resolutions, resolutions);
  assert.equal(repo.journal.filter((entry) => entry.kind === "conflict").length, 1);
  assert.deepEqual((await sync.status(ctx)).conflict.resolutions, resolutions);
}

// A large resolution text is stored as a Git object, not in the journal.
{
  const { repo, session, ctx } = await connected({ "intro.qmd": "same\n" });
  await session.setText("intro.qmd", "local\n");
  repo.advance({ "intro.qmd": "remote\n" });
  const conflict = await sync.pull(ctx);
  const text = "\uFEFF" + "a line of the resolution\n".repeat(90_000);
  assert.ok(text.length > 2 * 1024 * 1024);
  const resolutions = { "intro.qmd": { text }, "other.md": { delete: true } };
  await sync.saveResolutions(ctx, conflict.session, resolutions);
  assert.ok(JSON.stringify(repo.journal).length < 10_000);
  assert.ok(repo.refs.has(`${conflict.session.id}/resolutions`));
  assert.deepEqual((await sync.status(ctx)).conflict.resolutions, resolutions);
}

// Large non-ASCII texts are batched by serialized bytes and split when refused.
{
  const sizes = [];
  const cjk = (count, fill) => fill.repeat(count);
  const fake = (cap) => async (op, body) => {
    const size = Buffer.byteLength(JSON.stringify(body));
    sizes.push(size);
    if (size > cap) throw httpError(413, { error: "too large" });
    return { ids: body.blobs.map((item, i) => `id${sizes.length}-${i}`) };
  };
  const big = [0, 1, 2].map((i) => ({ text: cjk(4 * 1024 * 1024, String.fromCharCode(0x4e00 + i)) }));
  const first = await createBridge(fake(32 * 1024 * 1024)).putBlobs(big);
  assert.equal(first.length, 3);
  assert.ok(sizes.every((size) => size <= 32 * 1024 * 1024));
  sizes.length = 0;
  const small = [0, 1, 2, 3, 4, 5].map((i) => ({ text: cjk(100_000, String.fromCharCode(0x4e00 + i)) }));
  const ids = await createBridge(fake(1024 * 1024)).putBlobs(small);
  assert.equal(ids.length, 6);
  assert.ok(sizes.some((size) => size > 1024 * 1024));
  assert.equal(sizes.filter((size) => size <= 1024 * 1024).length >= 2, true);
  await assert.rejects(createBridge(fake(10)).putBlobs([{ text: "abc" }]));
}

// ---------------------------------------------------------------- Roborev 1028 fixes

// A retried pull does not upload an asset the server already has.
{
  const { repo, session, api, ctx } = await connected({ "intro.qmd": "one\ntwo\nthree\n" });
  repo.advance({ "intro.qmd": "one\ntwo\nthree!\n", "new.png": new Uint8Array([4, 5, 6]) });
  api.state.beforeApply = () => session.setText("intro.qmd", "ONE\ntwo\nthree\n");
  const calls = api.state.calls.length;
  const answer = await sync.pull(ctx);
  assert.equal(answer.status, "pulled", JSON.stringify(answer));
  assert.equal(api.state.calls.length, calls + 2, "the change was built and sent twice");
  assert.equal(api.state.uploads.length, 1, "the asset was uploaded once");
  assert.ok(session.has("new.png"));
}

// A conflict session keeps its merged tree reachable through a merged commit.
{
  const { repo, session, ctx } = await connected({ "intro.qmd": "same\n" });
  await session.setText("intro.qmd", "local\n");
  repo.advance({ "intro.qmd": "remote\n" });
  const answer = await sync.pull(ctx);
  assert.equal(answer.status, "conflict", JSON.stringify(answer));
  const { session: saved } = answer;
  const commit = repo.refs.get(`${saved.id}/merged`);
  assert.ok(commit, "the merged commit is kept");
  assert.equal(saved.merged, commit);
  assert.equal(repo.commits.get(commit).tree, saved.tree);
  assert.deepEqual(repo.commits.get(commit).parents, [saved.local, saved.theirs]);
  assert.equal((await sync.status(ctx)).conflict.merged, commit);
}

// Recovery offers only the current connection's conflict and leaves the others.
{
  const { repo, session, ctx } = await connected({ "intro.qmd": "same\n" });
  await session.setText("intro.qmd", "local\n");
  repo.advance({ "intro.qmd": "remote\n" });
  const conflict = await sync.pull(ctx);
  const entry = repo.journal.find((item) => item.kind === "conflict");
  const mine = await sync.recover(ctx);
  assert.equal(mine.conflict.id, conflict.session.id);
  entry.session.connection_id = "someone-else";
  const before = JSON.stringify(repo.journal);
  const other = await sync.recover(ctx);
  assert.equal(other.conflict, null, "another connection's session is not offered");
  assert.equal(JSON.stringify(repo.journal), before);
}

// ---------------------------------------------------------------- names that collide in LibrePaper

const SAME_NAME = "These files would have the same name in LibrePaper";

// A clean pull that would create two names differing only in case is refused before anything is sent.
{
  const { repo, session, api, ctx } = await connected({ "intro.qmd": "i\n" });
  await session.setText("Notes.md", "mine\n");
  repo.advance({ "notes.md": "theirs\n" });
  const calls = api.state.calls.length;
  const answer = await sync.pull(ctx);
  assert.equal(answer.status, "refused", JSON.stringify(answer));
  assert.ok(answer.refusals[0].reason.startsWith(SAME_NAME), JSON.stringify(answer));
  assert.deepEqual(answer.refusals[0].paths, ["Notes.md", "notes.md"]);
  assert.equal(api.state.calls.length, calls);
  assert.equal(session.text("Notes.md"), "mine\n");
}

// A case-only rename met by an edit of the old name: keeping the repository's file is refused, and the session survives.
{
  const { repo, session, api, ctx } = await connected({ "new-l6-0.md": "one\n" });
  session.remove("new-l6-0.md");
  await session.setText("New-l6-0.md", "one\n");
  repo.advance({ "new-l6-0.md": "remote\n" });
  const conflict = await sync.pull(ctx);
  assert.equal(conflict.status, "conflict", JSON.stringify(conflict));
  const calls = api.state.calls.length;

  const refused = await sync.applyResolution(ctx, conflict.session, { "new-l6-0.md": { keep: "theirs" } });
  assert.equal(refused.status, "refused", JSON.stringify(refused));
  assert.ok(refused.refusals[0].reason.startsWith(SAME_NAME), JSON.stringify(refused));
  assert.deepEqual(refused.refusals[0].paths, ["New-l6-0.md", "new-l6-0.md"]);
  assert.ok(refused.session, "the refusal carries the session");
  assert.equal(api.state.calls.length, calls);

  const done = await sync.applyResolution(ctx, refused.session, { "new-l6-0.md": { delete: true } });
  assert.ok(["pulled", "nothing"].includes(done.status), JSON.stringify(done));
  assert.equal(session.text("New-l6-0.md"), "one\n");
  assert.equal(session.has("new-l6-0.md"), false);
}

// ---------------------------------------------------------------- held entries and invalid names

// A project file inside a remote path that the remote holds as a link is not written over it.
{
  const { repo, ctx } = await world({
    remote: { "intro.qmd": "i\n", fig: { mode: "120000", content: "target.md" } },
    project: { "intro.qmd": "i\n", "fig/notes.md": "notes\n" },
  });
  const filter = await sync.filterAt(ctx, { folder: "" }, repo.head);
  assert.ok(filter.held.has("fig"));
  const local = await sync.snapshot(ctx, filter, null);
  assert.ok(!local.paths.includes("fig/notes.md"));
  assert.ok(local.paths.includes("intro.qmd"));
}

// A remote folder that holds only synced files does not block a new file inside it.
{
  const { repo, ctx } = await world({
    remote: { "fig/a.md": "a\n" },
    project: { "fig/a.md": "a\n", "fig/notes.md": "notes\n" },
  });
  const filter = await sync.filterAt(ctx, { folder: "" }, repo.head);
  assert.equal(filter.held.size, 0);
  const local = await sync.snapshot(ctx, filter, null);
  assert.ok(local.paths.includes("fig/notes.md"));
  assert.ok(local.paths.includes("fig/a.md"));
}

// A remote name that is not valid UTF-8 is never synced, and a project file with its lossy name is left out.
{
  const lossy = "bad�.md";
  const { repo, session, ctx } = await world({
    remote: { "intro.qmd": "i\n", [lossy]: "raw\n" },
    project: { "intro.qmd": "i\n", [lossy]: "local\n" },
  });
  repo.invalidNames.add(lossy);
  const filter = await sync.filterAt(ctx, { folder: "" }, repo.head);
  assert.ok(filter.unsynced.some((item) => item.path === lossy && item.reason === "a name that is not valid UTF-8"));
  assert.ok(!filter.synced.has(lossy));
  const local = await sync.snapshot(ctx, filter, null);
  assert.ok(!local.paths.includes(lossy));
  assert.ok(local.paths.includes("intro.qmd"));
  assert.equal(session.text(lossy), "local\n");
}

// ---------------------------------------------------------------- panel checks

// The remote check and the local changes read only: no journal entry and no change to the shared record.
{
  const unlinked = await world({ remote: { "intro.qmd": "i\n" }, project: { "intro.qmd": "i\n" } });
  assert.equal((await sync.checkRemote(unlinked.ctx)).status, "unknown");
  assert.equal((await sync.localChanges(unlinked.ctx)).status, "unknown");

  const { repo, session, api, ctx } = await connected({ "intro.qmd": "i\n" });
  const journal = JSON.stringify(repo.journal);
  const record = JSON.stringify(api.state.link);
  assert.deepEqual(await sync.checkRemote(ctx), { status: "current", head: repo.head });
  assert.deepEqual(await sync.localChanges(ctx), { status: "none", files: [] });

  repo.advance({ "intro.qmd": "remote\n" });
  assert.deepEqual(await sync.checkRemote(ctx), { status: "behind", head: repo.head });

  await session.setText("intro.qmd", "edited\n");
  await session.setText("new.md", "added\n");
  assert.deepEqual(await sync.localChanges(ctx), {
    status: "changes",
    files: [
      { path: "intro.qmd", status: "M" },
      { path: "new.md", status: "A" },
    ],
  });
  assert.equal(JSON.stringify(repo.journal), journal);
  assert.equal(JSON.stringify(api.state.link), record);
}

// A project file over an entry that only its content rules out, here an LFS
// pointer, is not a local change: a push would leave it out too.
{
  const pointer = "version https://git-lfs.github.com/spec/v1\noid sha256:abc\nsize 123\n";
  const { session, ctx } = await connected({ "intro.qmd": "i\n", "data/big.csv": pointer });
  await session.setText("data/big.csv", "a project copy\n");
  assert.deepEqual(await sync.localChanges(ctx), { status: "none", files: [] });
  const planned = await sync.outgoing(ctx);
  assert.notEqual(planned.status, "outgoing", JSON.stringify(planned));
}

console.log("gitsync sync ok");
