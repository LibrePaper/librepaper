import { gitRequest } from "../desktop/client.js";

// One function per operation of the desktop app's Git service. Each takes
// plain arguments and returns the parsed answer. The service holds no sync
// logic: it fetches, pushes, builds trees and commits, and merges.

/// The most content one `blobs` call may carry, from the service's own limit.
const BLOB_BATCH_BYTES = 24 * 1024 * 1024;
/// How many blobs go in one call when their sizes are not known.
const BLOB_BATCH_COUNT = 64;
/// How much a `put-blobs` call carries before it is split. The service takes
/// 32 MiB of JSON and base64 is a third larger than the bytes.
const PUT_BATCH_BYTES = 16 * 1024 * 1024;
const KNOWN_BATCH = 512;

export function bytesToBase64(bytes) {
  let binary = "";
  const step = 0x8000;
  for (let i = 0; i < bytes.length; i += step) {
    binary += String.fromCharCode(...bytes.subarray(i, i + step));
  }
  return btoa(binary);
}

export function base64ToBytes(text) {
  const binary = atob(text);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i += 1) bytes[i] = binary.charCodeAt(i);
  return bytes;
}

/// Splits `ids` into batches whose known sizes add up to at most the limit.
/// An id without a known size counts as a share of a full batch of
/// `BLOB_BATCH_COUNT`.
export function blobBatches(ids, sizes = null) {
  const batches = [];
  let batch = [];
  let weight = 0;
  const unknown = BLOB_BATCH_BYTES / BLOB_BATCH_COUNT;
  for (const id of ids) {
    const size = sizes?.get(id);
    const cost = typeof size === "number" ? size : unknown;
    if (batch.length && weight + cost > BLOB_BATCH_BYTES) {
      batches.push(batch);
      batch = [];
      weight = 0;
    }
    batch.push(id);
    weight += cost;
  }
  if (batch.length) batches.push(batch);
  return batches;
}

/// The bridge over `request(op, body)`, which is `gitRequest` in the app and a
/// fake in tests.
export function createBridge(request = gitRequest) {
  return {
    status: () => request("status", {}),
    chooseLocal: () => request("choose-local", {}),
    branches: (remote) => request("branches", { remote }),
    connect: ({ connection_id, remote, branch }) => request("connect", { connection_id, remote, branch }),
    fetch: (also = []) => request("fetch", { also }),
    tree: (commit) => request("tree", { commit }),

    /// The bytes of each blob, as a `Map` from id to `Uint8Array`. `sizes`, a
    /// `Map` from id to byte size, lets the batches stay under the limit.
    async blobs(ids, sizes = null) {
      const found = new Map();
      const read = async (batch) => {
        let answer;
        try {
          answer = await request("blobs", { ids: batch });
        } catch (error) {
          // The service refuses a call over its content limit. Halve and ask again.
          if (error?.status !== 413 || batch.length < 2) throw error;
          const half = Math.ceil(batch.length / 2);
          await read(batch.slice(0, half));
          await read(batch.slice(half));
          return;
        }
        for (const id of batch) {
          if (typeof answer.blobs?.[id] !== "string") throw new Error(`The Git service did not return blob ${id}.`);
          found.set(id, base64ToBytes(answer.blobs[id]));
        }
      };
      for (const batch of blobBatches([...new Set(ids)], sizes)) await read(batch);
      return found;
    },

    /// A `Map` from sha256 to blob id for the assets the service has seen.
    async knownAssets(sha256) {
      const known = new Map();
      for (let i = 0; i < sha256.length; i += KNOWN_BATCH) {
        const answer = await request("known-assets", { sha256: sha256.slice(i, i + KNOWN_BATCH) });
        for (const [sha, id] of Object.entries(answer.known || {})) known.set(sha, id);
      }
      return known;
    },

    /// Writes blobs and answers their ids in order. An item is `{ text }`,
    /// `{ bytes, sha256? }` or `{ base64, sha256? }`.
    async putBlobs(items) {
      const ids = [];
      let batch = [];
      let weight = 0;
      const flush = async () => {
        if (!batch.length) return;
        const answer = await request("put-blobs", { blobs: batch });
        ids.push(...answer.ids);
        batch = [];
        weight = 0;
      };
      for (const item of items) {
        let wire;
        if (typeof item.text === "string") {
          wire = { text: item.text };
        } else {
          wire = item.sha256
            ? { base64: item.base64 ?? bytesToBase64(item.bytes), sha256: item.sha256 }
            : { base64: item.base64 ?? bytesToBase64(item.bytes) };
        }
        const cost = (wire.text ?? wire.base64).length;
        if (batch.length && weight + cost > PUT_BATCH_BYTES) await flush();
        batch.push(wire);
        weight += cost;
      }
      await flush();
      return ids;
    },

    treeEdit: async (base, edits) => (await request("tree-edit", { base, edits })).tree,
    commitTree: async (tree, parents, message, author = null) =>
      (await request("commit-tree", { tree, parents, message, author })).commit,
    merge: (ours, theirs) => request("merge", { ours, theirs }),
    diff: async (from, to) => (await request("diff", { from, to })).changes,
    ancestor: async (ancestor, descendant) => (await request("ancestor", { ancestor, descendant })).is_ancestor,
    push: (commit) => request("push", { commit }),
    keep: (operation, refs) => request("keep", { operation, refs }),
    release: (operation) => request("release", { operation }),
    journal: async () => (await request("journal", {})).entries,
    /// An entry replaces the one with its id or is appended; neither call
    /// rewrites the whole file, so another window's entries are never lost.
    journalUpsert: (entry) => request("journal", { upsert: entry }),
    journalRemove: (id) => request("journal", { remove: id }),
    forget: () => request("forget", {}),
  };
}

/// The bridge the app uses.
export const bridge = createBridge();
