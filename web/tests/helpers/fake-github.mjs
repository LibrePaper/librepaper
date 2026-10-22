// An in-memory GitHub for sync tests: the REST endpoints and JSON shapes that
// web/src/lib/forge/github.js uses, plus the OAuth pages, on a local port.
//
// Objects are real git objects. Blob, tree and commit ids are hashed the way
// git hashes them, so a blob id equals `git hash-object` of the same bytes.
// Nothing here touches the network beyond 127.0.0.1.

import { createHash } from "node:crypto";
import { createServer } from "node:http";

const sha1 = (...parts) => {
  const hash = createHash("sha1");
  for (const part of parts) hash.update(part);
  return hash.digest();
};

const toBuffer = (value) => (typeof value === "string" ? Buffer.from(value, "utf8") : Buffer.from(value));

// Hashes an object of `type` and returns its id.
function objectId(type, data) {
  return sha1(`${type} ${data.length}\0`, data).toString("hex");
}

// Git orders tree entries by name, with a directory compared as "name/".
function sortEntries(entries) {
  const key = (entry) => (entry.type === "tree" ? `${entry.name}/` : entry.name);
  return [...entries].sort((a, b) => Buffer.compare(Buffer.from(key(a)), Buffer.from(key(b))));
}

function treeBytes(entries) {
  return Buffer.concat(
    sortEntries(entries).map((entry) =>
      Buffer.concat([Buffer.from(`${entry.mode} ${entry.name}\0`), Buffer.from(entry.sha, "hex")]),
    ),
  );
}

class Repo {
  constructor(fullName, id, defaultBranch) {
    this.fullName = fullName;
    this.id = id;
    this.defaultBranch = defaultBranch;
    this.objects = new Map();
    this.branches = new Map();
    this.clock = 1700000000;
  }

  put(type, data, extra = {}) {
    const sha = objectId(type, data);
    if (!this.objects.has(sha)) this.objects.set(sha, { type, data, ...extra });
    return sha;
  }

  putBlob(bytes) {
    return this.put("blob", toBuffer(bytes));
  }

  // Builds nested trees from a flat Map of path -> { mode, sha }.
  putTreeFromFlat(flat) {
    const root = { files: new Map(), dirs: new Map() };
    for (const [path, entry] of flat) {
      const parts = path.split("/");
      let node = root;
      for (const part of parts.slice(0, -1)) {
        if (!node.dirs.has(part)) node.dirs.set(part, { files: new Map(), dirs: new Map() });
        node = node.dirs.get(part);
      }
      node.files.set(parts.at(-1), entry);
    }
    const write = (node) => {
      const entries = [];
      for (const [name, entry] of node.files) entries.push({ name, mode: entry.mode, type: "blob", sha: entry.sha });
      for (const [name, child] of node.dirs) entries.push({ name, mode: "40000", type: "tree", sha: write(child) });
      const sorted = sortEntries(entries);
      return this.put("tree", treeBytes(sorted), { entries: sorted });
    };
    return write(root);
  }

  // A tree as a flat Map of path -> { mode, sha }, blobs only.
  flatten(treeSha, prefix = "", out = new Map()) {
    const tree = this.objects.get(treeSha);
    if (!tree || tree.type !== "tree") return out;
    for (const entry of tree.entries) {
      const path = prefix + entry.name;
      if (entry.type === "tree") this.flatten(entry.sha, `${path}/`, out);
      else out.set(path, { mode: entry.mode, sha: entry.sha });
    }
    return out;
  }

  // The recursive listing GitHub gives, tree entries included.
  listing(treeSha, prefix = "", out = []) {
    const tree = this.objects.get(treeSha);
    for (const entry of tree.entries) {
      const path = prefix + entry.name;
      if (entry.type === "tree") {
        out.push({ path, mode: entry.mode, type: "tree", sha: entry.sha });
        this.listing(entry.sha, `${path}/`, out);
      } else {
        out.push({ path, mode: entry.mode, type: "blob", sha: entry.sha, size: this.objects.get(entry.sha).data.length });
      }
    }
    return out;
  }

  putCommit({ message, tree, parents = [], author }) {
    const when = author?.date ? Math.floor(new Date(author.date).getTime() / 1000) : (this.clock += 60);
    const who = `${author?.name || "Fake Author"} <${author?.email || "fake@example.invalid"}> ${when} +0000`;
    const lines = [`tree ${tree}`, ...parents.map((parent) => `parent ${parent}`), `author ${who}`, `committer ${who}`];
    const data = Buffer.from(`${lines.join("\n")}\n\n${message}`);
    return this.put("commit", data, { tree, parents, message });
  }

  commitOf(sha) {
    const object = this.objects.get(sha);
    return object?.type === "commit" ? object : null;
  }

  // A branch name or a commit id, as a commit id, or null.
  resolve(ref) {
    if (this.branches.has(ref)) return this.branches.get(ref);
    return this.commitOf(ref) ? ref : null;
  }

  ancestors(sha) {
    const seen = new Set();
    const stack = [sha];
    while (stack.length) {
      const next = stack.pop();
      if (seen.has(next)) continue;
      seen.add(next);
      stack.push(...this.commitOf(next).parents);
    }
    return seen;
  }

  applyFiles(flat, files) {
    for (const [path, value] of Object.entries(files || {})) {
      if (value === null) flat.delete(path);
      else flat.set(path, { mode: "100644", sha: this.putBlob(value) });
    }
    return flat;
  }
}

const CORS = {
  "Access-Control-Allow-Origin": "*",
  "Access-Control-Allow-Headers": "Authorization, Content-Type, X-GitHub-Api-Version, Accept",
  "Access-Control-Allow-Methods": "GET, POST, PATCH, OPTIONS",
  "Access-Control-Max-Age": "600",
};

/// Starts the fake on 127.0.0.1 and a free port. `api` and `web` are both the
/// base URL; the paths tell the REST calls and the OAuth pages apart.
export async function startFakeGitHub({ token = "fake-token" } = {}) {
  const repos = new Map();
  const requests = [];
  let truncate = false;
  let nextId = 1000;

  const getRepo = (fullName) => {
    const repo = repos.get(fullName);
    if (!repo) throw new Error(`fake github has no repository ${fullName}`);
    return repo;
  };

  function seed(fullName, { branch = "main", files = {}, message = "Initial commit" } = {}) {
    let repo = repos.get(fullName);
    if (!repo) {
      repo = new Repo(fullName, (nextId += 1), branch);
      repos.set(fullName, repo);
    }
    const flat = repo.applyFiles(new Map(), files);
    const sha = repo.putCommit({ message, tree: repo.putTreeFromFlat(flat) });
    repo.branches.set(branch, sha);
    return sha;
  }

  function commit(fullName, branch, { files = {}, message = "Update" } = {}) {
    const repo = getRepo(fullName);
    const head = repo.branches.get(branch);
    if (!head) throw new Error(`fake github has no branch ${branch} in ${fullName}`);
    const flat = repo.applyFiles(repo.flatten(repo.commitOf(head).tree), files);
    const sha = repo.putCommit({ message, tree: repo.putTreeFromFlat(flat), parents: [head] });
    repo.branches.set(branch, sha);
    return sha;
  }

  function read(fullName, ref, path) {
    const repo = getRepo(fullName);
    const sha = repo.resolve(ref);
    if (!sha) return null;
    const entry = repo.flatten(repo.commitOf(sha).tree).get(path);
    return entry ? Buffer.from(repo.objects.get(entry.sha).data) : null;
  }

  function list(fullName, ref) {
    const repo = getRepo(fullName);
    const sha = repo.resolve(ref);
    return sha ? [...repo.flatten(repo.commitOf(sha).tree).keys()].sort() : [];
  }

  function headOf(fullName, branch) {
    return getRepo(fullName).branches.get(branch) ?? null;
  }

  // A merge commit of `base` (a branch, which then advances) and `head`.
  function merge(fullName, { base, head, files = {}, message } = {}) {
    const repo = getRepo(fullName);
    const baseSha = repo.resolve(base);
    const headSha = repo.resolve(head);
    if (!baseSha || !headSha) throw new Error(`fake github cannot resolve ${base} and ${head} in ${fullName}`);
    const flat = repo.applyFiles(repo.flatten(repo.commitOf(baseSha).tree), files);
    const sha = repo.putCommit({
      message: message || `Merge ${head} into ${base}`,
      tree: repo.putTreeFromFlat(flat),
      parents: [baseSha, headSha],
    });
    if (repo.branches.has(base)) repo.branches.set(base, sha);
    return sha;
  }

  // ---- HTTP ----

  function send(res, status, body, headers = {}) {
    const text = body === undefined ? "" : JSON.stringify(body);
    res.writeHead(status, { ...CORS, "Content-Type": "application/json; charset=utf-8", ...headers });
    res.end(text);
  }

  const missing = (res) => send(res, 404, { message: "Not Found" });
  const invalid = (res, message) => send(res, 422, { message });

  const wrap60 = (buffer) => buffer.toString("base64").replace(/(.{60})/g, "$1\n");

  function page(items, query) {
    const per = Number(query.get("per_page")) || 30;
    const at = Math.max(1, Number(query.get("page")) || 1);
    return items.slice((at - 1) * per, at * per);
  }

  async function body(req) {
    const chunks = [];
    for await (const chunk of req) chunks.push(chunk);
    return Buffer.concat(chunks).toString("utf8");
  }

  function oauthBody(text, contentType) {
    if (!text) return {};
    if (String(contentType).includes("json")) {
      try {
        return JSON.parse(text);
      } catch {
        return {};
      }
    }
    return Object.fromEntries(new URLSearchParams(text));
  }

  async function rest(req, res, url, method) {
    const query = url.searchParams;
    const path = url.pathname;
    let m;

    if (method === "GET" && path === "/user/installations") {
      return send(res, 200, { total_count: 1, installations: page([{ id: 1 }], query) });
    }
    if (method === "GET" && path === "/user/installations/1/repositories") {
      const all = [...repos.values()].map((repo) => ({
        id: repo.id,
        full_name: repo.fullName,
        default_branch: repo.defaultBranch,
        private: true,
        permissions: { push: true },
      }));
      return send(res, 200, { total_count: all.length, repositories: page(all, query) });
    }

    m = path.match(/^\/repos\/([^/]+\/[^/]+)\/(.*)$/);
    if (!m) return missing(res);
    const repo = repos.get(decodeURIComponent(m[1]));
    if (!repo) return missing(res);
    const rest = m[2];

    if (method === "GET" && rest === "branches") {
      const all = [...repo.branches].map(([name, sha]) => ({ name, commit: { sha } }));
      return send(res, 200, page(all, query));
    }
    if (method === "GET" && (m = rest.match(/^git\/ref\/heads\/(.+)$/))) {
      const branch = decodeURIComponent(m[1]);
      const sha = repo.branches.get(branch);
      if (!sha) return missing(res);
      return send(res, 200, { ref: `refs/heads/${branch}`, object: { sha, type: "commit" } });
    }
    if (method === "GET" && (m = rest.match(/^git\/commits\/([0-9a-f]+)$/))) {
      const found = repo.commitOf(m[1]);
      if (!found) return missing(res);
      return send(res, 200, {
        sha: m[1],
        tree: { sha: found.tree },
        parents: found.parents.map((sha) => ({ sha })),
        message: found.message,
      });
    }
    if (method === "GET" && (m = rest.match(/^git\/trees\/([0-9a-f]+)$/))) {
      const found = repo.objects.get(m[1]);
      if (!found || found.type !== "tree") return missing(res);
      const recursive = query.get("recursive");
      const cut = recursive && truncate;
      if (cut) truncate = false;
      const tree = recursive
        ? repo.listing(m[1])
        : found.entries.map((entry) => ({
            path: entry.name,
            mode: entry.mode,
            type: entry.type,
            sha: entry.sha,
            ...(entry.type === "blob" ? { size: repo.objects.get(entry.sha).data.length } : {}),
          }));
      return send(res, 200, { sha: m[1], tree: cut ? tree.slice(0, Math.ceil(tree.length / 2)) : tree, truncated: Boolean(cut) });
    }
    if (method === "GET" && (m = rest.match(/^git\/blobs\/([0-9a-f]+)$/))) {
      const found = repo.objects.get(m[1]);
      if (!found || found.type !== "blob") return missing(res);
      return send(res, 200, { sha: m[1], size: found.data.length, content: wrap60(found.data), encoding: "base64" });
    }
    if (method === "GET" && (m = rest.match(/^contents\/(.+)$/))) {
      const ref = query.get("ref") || repo.defaultBranch;
      const sha = repo.resolve(ref);
      const entry = sha && repo.flatten(repo.commitOf(sha).tree).get(decodeURIComponent(m[1]));
      if (!entry) return missing(res);
      return send(res, 200, { content: wrap60(repo.objects.get(entry.sha).data), encoding: "base64", sha: entry.sha });
    }
    if (method === "GET" && (m = rest.match(/^compare\/(.+)$/))) {
      const spec = decodeURIComponent(m[1]);
      const at = spec.indexOf("...");
      if (at < 0) return missing(res);
      const base = repo.resolve(spec.slice(0, at));
      const head = repo.resolve(spec.slice(at + 3));
      if (!base || !head) return missing(res);
      const baseSet = repo.ancestors(base);
      const headSet = repo.ancestors(head);
      const ahead = [...headSet].filter((sha) => !baseSet.has(sha)).length;
      const behind = [...baseSet].filter((sha) => !headSet.has(sha)).length;
      const status = ahead && behind ? "diverged" : ahead ? "ahead" : behind ? "behind" : "identical";
      return send(res, 200, { status, ahead_by: ahead, behind_by: behind });
    }

    const text = method === "GET" ? "" : await body(req);
    let json;
    try {
      json = text ? JSON.parse(text) : {};
    } catch {
      return send(res, 400, { message: "Problems parsing JSON" });
    }

    if (method === "POST" && rest === "git/blobs") {
      const bytes = json.encoding === "base64" ? Buffer.from(String(json.content), "base64") : Buffer.from(String(json.content ?? ""), "utf8");
      return send(res, 201, { sha: repo.putBlob(bytes) });
    }
    if (method === "POST" && rest === "git/trees") {
      const flat = json.base_tree ? repo.flatten(json.base_tree) : new Map();
      if (json.base_tree && repo.objects.get(json.base_tree)?.type !== "tree") return invalid(res, "base_tree not found");
      for (const entry of json.tree || []) {
        const path = String(entry.path);
        if (entry.sha === null) {
          flat.delete(path);
          for (const key of [...flat.keys()]) if (key.startsWith(`${path}/`)) flat.delete(key);
        } else if (entry.type === "tree") {
          if (repo.objects.get(entry.sha)?.type !== "tree") return invalid(res, `tree ${entry.sha} not found`);
          for (const [sub, value] of repo.flatten(entry.sha)) flat.set(`${path}/${sub}`, value);
        } else {
          if (repo.objects.get(entry.sha)?.type !== "blob") return invalid(res, `GitRPC::BadObjectState: blob ${entry.sha} not found`);
          flat.set(path, { mode: entry.mode || "100644", sha: entry.sha });
        }
      }
      return send(res, 201, { sha: repo.putTreeFromFlat(flat) });
    }
    if (method === "POST" && rest === "git/commits") {
      if (repo.objects.get(json.tree)?.type !== "tree") return invalid(res, "Tree SHA does not exist");
      const parents = json.parents || [];
      if (parents.some((parent) => !repo.commitOf(parent))) return invalid(res, "Parent SHA does not exist");
      return send(res, 201, { sha: repo.putCommit({ message: json.message ?? "", tree: json.tree, parents, author: json.author }) });
    }
    if (method === "PATCH" && (m = rest.match(/^git\/refs\/heads\/(.+)$/))) {
      const branch = decodeURIComponent(m[1]);
      const current = repo.branches.get(branch);
      if (!current) return missing(res);
      if (!repo.commitOf(json.sha)) return invalid(res, "Object does not exist");
      if (!json.force && !repo.ancestors(json.sha).has(current)) return invalid(res, "Update is not a fast forward");
      repo.branches.set(branch, json.sha);
      return send(res, 200, { ref: `refs/heads/${branch}`, object: { sha: json.sha, type: "commit" } });
    }
    if (method === "POST" && rest === "git/refs") {
      const match = String(json.ref).match(/^refs\/heads\/(.+)$/);
      if (!match) return invalid(res, "Reference name is invalid");
      if (repo.branches.has(match[1])) return invalid(res, "Reference already exists");
      if (!repo.commitOf(json.sha)) return invalid(res, "Object does not exist");
      repo.branches.set(match[1], json.sha);
      return send(res, 201, { ref: json.ref, object: { sha: json.sha, type: "commit" } });
    }
    return missing(res);
  }

  async function handle(req, res) {
    const url = new URL(req.url, "http://127.0.0.1");
    const method = req.method;
    if (method === "OPTIONS") {
      res.writeHead(204, CORS);
      return res.end();
    }
    requests.push({ method, path: url.pathname + url.search, authorization: req.headers.authorization ?? null });

    if (method === "GET" && url.pathname === "/login/oauth/authorize") {
      const redirect = url.searchParams.get("redirect_uri");
      if (!redirect) return send(res, 422, { message: "redirect_uri is required" });
      const target = new URL(redirect);
      target.searchParams.set("code", "fake-code");
      const state = url.searchParams.get("state");
      if (state !== null) target.searchParams.set("state", state);
      res.writeHead(302, { ...CORS, Location: target.toString() });
      return res.end();
    }
    if (method === "POST" && url.pathname === "/login/oauth/access_token") {
      const form = oauthBody(await body(req), req.headers["content-type"]);
      if (form.code === "fake-code" || form.grant_type === "refresh_token") {
        return send(res, 200, {
          access_token: token,
          token_type: "bearer",
          expires_in: 28800,
          refresh_token: "fake-refresh",
          refresh_token_expires_in: 15897600,
        });
      }
      return send(res, 200, { error: "bad_verification_code" });
    }

    if (req.headers.authorization !== `Bearer ${token}`) {
      return send(res, 401, { message: "Bad credentials" });
    }
    return rest(req, res, url, method);
  }

  const server = createServer((req, res) => {
    handle(req, res).catch((error) => {
      if (!res.headersSent) send(res, 500, { message: String(error?.message || error) });
      else res.end();
    });
  });
  await new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", resolve);
  });
  const url = `http://127.0.0.1:${server.address().port}`;

  return {
    api: url,
    web: url,
    url,
    requests,
    repo: getRepo,
    seed,
    commit,
    read,
    list,
    headOf,
    merge,
    truncateNextTree() {
      truncate = true;
    },
    async stop() {
      server.closeAllConnections?.();
      await new Promise((resolve) => server.close(resolve));
    },
  };
}
