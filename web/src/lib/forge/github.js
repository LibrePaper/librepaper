// The GitHub REST calls that sync makes, as the connected user.
//
// Every call carries the user's token in the Authorization header and never in
// a URL. api.github.com answers browser requests, so there is no relay in the
// path. `repo` is always "owner/name".

/// GitHub refused the token: it is expired, revoked or missing.
export class AuthError extends Error {
  constructor(message = "GitHub did not accept the saved sign-in.") {
    super(message);
    this.name = "AuthError";
    this.status = 401;
  }
}

/// A branch moved between reading its head and moving it.
export class MovedError extends Error {
  constructor(message = "The branch moved on GitHub.") {
    super(message);
    this.name = "MovedError";
    this.status = 422;
  }
}

/// Any other refusal from GitHub, with the HTTP status and GitHub's message.
export class GitHubError extends Error {
  constructor(status, message) {
    super(message || `GitHub answered ${status}`);
    this.name = "GitHubError";
    this.status = status;
  }
}

/// Base64 with or without line breaks, as bytes.
export function decodeBase64(text) {
  const binary = atob(String(text).replace(/\s+/g, ""));
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i += 1) bytes[i] = binary.charCodeAt(i);
  return bytes;
}

function encodeBase64(bytes) {
  let binary = "";
  for (let i = 0; i < bytes.length; i += 0x8000) {
    binary += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  }
  return btoa(binary);
}

// Encodes each segment of a slash-separated name and keeps the slashes.
const encodePath = (path) => String(path).split("/").map(encodeURIComponent).join("/");

/// A client for one token. `api` is the REST base from /api/config.
export function gitHub({ token, api = "https://api.github.com", fetch = globalThis.fetch } = {}) {
  const base = String(api).replace(/\/+$/, "");

  async function request(method, path, body, { allow = [] } = {}) {
    const response = await fetch(`${base}${path}`, {
      method,
      headers: {
        Authorization: `Bearer ${token}`,
        Accept: "application/vnd.github+json",
        "X-GitHub-Api-Version": "2022-11-28",
        ...(body === undefined ? {} : { "Content-Type": "application/json" }),
      },
      ...(body === undefined ? {} : { body: JSON.stringify(body) }),
    });
    if (response.status === 401) throw new AuthError();
    if (allow.includes(response.status)) return { status: response.status, data: null };
    const data = await response.json().catch(() => null);
    if (!response.ok) throw new GitHubError(response.status, data?.message);
    return { status: response.status, data };
  }

  const getJson = async (path, options) => (await request("GET", path, undefined, options)).data;

  // Reads every page of a list. `pick` takes the array out of one page.
  async function paged(path, pick) {
    const out = [];
    for (let page = 1; ; page += 1) {
      const items = pick(await getJson(`${path}${path.includes("?") ? "&" : "?"}per_page=100&page=${page}`)) || [];
      out.push(...items);
      if (items.length < 100) return out;
    }
  }

  return {
    /// Repositories the user may push to, across every installation of the app.
    async repositories() {
      const installations = await paged("/user/installations", (page) => page?.installations);
      const out = [];
      for (const installation of installations) {
        const repositories = await paged(`/user/installations/${installation.id}/repositories`, (page) => page?.repositories);
        for (const repo of repositories) {
          if (!repo.permissions?.push) continue;
          out.push({
            id: repo.id,
            name: repo.full_name,
            defaultBranch: repo.default_branch,
            private: Boolean(repo.private),
            installation: installation.id,
          });
        }
      }
      return out;
    },

    /// Branch names of a repository.
    async branches(repo) {
      return (await paged(`/repos/${repo}/branches`, (page) => page)).map((branch) => branch.name);
    },

    /// The commit a branch points at.
    async head(repo, branch) {
      const ref = await getJson(`/repos/${repo}/git/ref/heads/${encodePath(branch)}`);
      return ref.object.sha;
    },

    /// The recursive tree of a commit: its entries, whether GitHub cut the
    /// listing short, and the id of the root tree.
    async treeOf(repo, commitSha) {
      const commit = await getJson(`/repos/${repo}/git/commits/${commitSha}`);
      const tree = await getJson(`/repos/${repo}/git/trees/${commit.tree.sha}?recursive=1`);
      return { entries: tree.tree || [], truncated: Boolean(tree.truncated), tree: commit.tree.sha };
    },

    /// The bytes of a blob.
    async blob(repo, sha) {
      const blob = await getJson(`/repos/${repo}/git/blobs/${sha}`);
      return decodeBase64(blob.content);
    },

    /// A text file at a ref, or null when it does not exist.
    async textFile(repo, ref, path) {
      const { status, data } = await request(
        "GET",
        `/repos/${repo}/contents/${encodePath(path)}?ref=${encodeURIComponent(ref)}`,
        undefined,
        { allow: [404] },
      );
      if (status === 404 || !data || Array.isArray(data) || typeof data.content !== "string") return null;
      return new TextDecoder().decode(decodeBase64(data.content));
    },

    async createBlob(repo, bytes) {
      const { data } = await request("POST", `/repos/${repo}/git/blobs`, { content: encodeBase64(bytes), encoding: "base64" });
      return data.sha;
    },

    /// `entries` are `{ path, mode, type, sha }`; a null sha deletes the path.
    async createTree(repo, baseTree, entries) {
      const { data } = await request("POST", `/repos/${repo}/git/trees`, {
        base_tree: baseTree,
        tree: entries.map((entry) => ({
          path: entry.path,
          mode: entry.mode || "100644",
          type: entry.type || "blob",
          sha: entry.sha,
        })),
      });
      return data.sha;
    },

    async createCommit(repo, { message, tree, parents, author }) {
      const { data } = await request("POST", `/repos/${repo}/git/commits`, { message, tree, parents, author });
      return data.sha;
    },

    /// Moves a branch forward without force. Throws MovedError when the branch
    /// no longer points at the commit's parent.
    async moveBranch(repo, branch, sha) {
      try {
        await request("PATCH", `/repos/${repo}/git/refs/heads/${encodePath(branch)}`, { sha, force: false });
      } catch (error) {
        if (error?.status === 422) throw new MovedError(error.message);
        throw error;
      }
    },

    async createBranch(repo, name, sha) {
      await request("POST", `/repos/${repo}/git/refs`, { ref: `refs/heads/${name}`, sha });
    },

    /// How `head` relates to `base`: "ahead", "behind", "identical" or "diverged".
    async compare(repo, base, head) {
      const result = await getJson(`/repos/${repo}/compare/${encodeURIComponent(base)}...${encodeURIComponent(head)}`);
      return result.status;
    },
  };
}
