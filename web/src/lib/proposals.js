// A proposed edit is a branch: a forked LoroDoc that collects ordinary edits,
// reviewed as the diff between where it forked and where it has reached. This
// replaces the deleted track-changes.js, and is NOT a port of it — read SPEC
// §1.2 for why that mechanism is gone.
//
// The browser holds one branch per author per session (§10). Typing into it
// accumulates edits; flushing exports the operations since the last flush and
// sends them to the server. The server holds the branch bytes and review state
// in Postgres and makes decisions about which hunks to apply.

import { LoroDoc, encodeFrontiers, decodeFrontiers } from "loro-crdt";

// How much untouched text may sit between two changes before they stop being
// one decision. This must match crates/librepaper/src/document/hunks.rs:SAME_DECISION_WITHIN
// or a reviewer's decision will revert the wrong hunks.
const SAME_DECISION_WITHIN = 8;

// Groups a run of deltas into hunks: maximal runs of non-retain deltas, with
// short gaps (≤ SAME_DECISION_WITHIN) bridging deltas together into one hunk.
// This mirrors hunks.rs:runs() exactly.
function hunkRanges(deltas) {
  const ranges = [];
  let i = 0;
  while (i < deltas.length) {
    // Skip leading retains; they don't start a run.
    if (deltas[i].retain !== undefined) {
      i++;
      continue;
    }
    const start = i;
    while (i < deltas.length) {
      const delta = deltas[i];
      if (delta.retain !== undefined) {
        // Look ahead to decide if this retain bridges changes together.
        // A retain at the very end of the deltas is trailing context and does
        // not bridge, so check whether anything after it is non-retain.
        const hasMoreChanges = deltas.slice(i + 1).some((d) => d.retain === undefined);
        const bridges = delta.retain <= SAME_DECISION_WITHIN && hasMoreChanges;
        if (!bridges) {
          break;
        }
      }
      i++;
    }
    ranges.push({ start, end: i });
  }
  return ranges;
}

// Sum the old-side length of a range of deltas (delete and retain, not insert).
function oldSideLength(deltas) {
  return deltas.reduce((sum, d) => {
    if (d.delete !== undefined) return sum + d.delete;
    if (d.retain !== undefined) return sum + d.retain;
    return sum; // insert adds 0 on the old side
  }, 0);
}

// Split text deltas into hunks, numbered in order.
function hunkify(deltas) {
  const hunks = [];
  let cursor = 0; // position in the old side of the diff
  let at = 0; // position in the delta array
  for (const range of hunkRanges(deltas)) {
    // Everything before this hunk moves the cursor forward.
    cursor += oldSideLength(deltas.slice(at, range.start));
    let deleted = 0;
    let inserted = "";
    for (let i = range.start; i < range.end; i++) {
      const d = deltas[i];
      if (d.delete !== undefined) {
        deleted += d.delete;
      } else if (d.insert !== undefined) {
        inserted += d.insert;
      } else if (d.retain !== undefined) {
        // Text retained inside a hunk is untouched on both sides, so it counts
        // towards how far the hunk reaches on the old side.
        deleted += d.retain;
      }
    }
    hunks.push({
      index: hunks.length,
      start: cursor,
      deleted,
      inserted,
    });
    cursor += deleted;
    at = range.end;
  }
  return hunks;
}

// Create the proposals API.
export function createProposals({ session, send, mayEdit }) {
  // The open proposal: one per author per session. The browser sends incremental
  // updates to the server, which holds the full branch bytes and review state.
  let proposal = null;

  // Track the version vector from the last flush so we can compute incrementals.
  let lastFlushedVV = null;

  return {
    // Fork the session document at its current frontier and remember the base.
    // Returns the existing proposal if one is already open.
    start() {
      if (proposal) return proposal;

      const doc = session.doc;
      const base = doc.frontiers();
      const branch = doc.fork();

      proposal = {
        id: null, // assigned by the server
        base,
        tip: base,
        branch,
      };

      lastFlushedVV = branch.oplogVersion();

      // Tell the server we've opened a proposal at this frontier.
      send({
        type: "proposal-open",
        base: encodeBase64(encodeFrontiers(base)),
      });

      return proposal;
    },

    // Get the text at a file id from the branch. The caller types into it.
    text(fileId) {
      if (!proposal) return null;
      const files = proposal.branch.getMap("files");
      const text = files.get(fileId);
      return text?.kind?.() === "Text" ? text : null;
    },

    // Export the operations accumulated since the last flush and send them to the server.
    flush() {
      if (!proposal) return;

      // Only send if there's been a change. The branch's current version vector
      // tells us what we haven't sent yet.
      const currentVV = proposal.branch.oplogVersion();
      const update = proposal.branch.export({
        mode: "update",
        from: lastFlushedVV,
      });

      if (update.byteLength > 0) {
        // Remember where we are now for the next flush.
        lastFlushedVV = currentVV;

        // Update the tip frontier.
        proposal.tip = proposal.branch.frontiers();

        // Tell the server.
        send({
          type: "proposal-update",
          update: encodeBase64(update),
        });
      }
    },

    // Close the local branch. The server owns the decision from here on.
    stop() {
      if (!proposal) return;
      proposal.branch.destroy?.();
      proposal = null;
      lastFlushedVV = null;
    },

    // Handle incoming messages from the server.
    apply(message) {
      if (message.type === "proposal-list") {
        // The server is telling us about open proposals. Each has author, base, tip, and branch bytes.
        // Store them for rendering.
        if (!this._openProposals) this._openProposals = new Map();
        this._openProposals.clear();
        for (const p of message.proposals || []) {
          this._openProposals.set(p.id, {
            id: p.id,
            author: p.author,
            base: decodeFrontiers(decodeBase64(p.base)),
            tip: decodeFrontiers(decodeBase64(p.tip)),
            bytes: p.branch ? decodeBase64(p.branch) : null,
          });
        }
      } else if (message.type === "proposal-decided") {
        // Somebody decided a hunk. Only close the local branch when the
        // proposal that resolved is the one this browser is drafting --
        // closing on anyone's decision would throw away unsent work the
        // moment a coauthor answered a different proposal.
        if (message.resolved && proposal && message.proposal_id === proposal.id) {
          this.stop();
        }
        this._openProposals?.delete(message.proposal_id);
      }
    },

    // The proposals the server has told us about, for rendering.
    open() {
      return this._openProposals ? Array.from(this._openProposals.values()) : [];
    },

    // Compute the hunks of a proposal. Pass either a proposal object (from
    // proposal-list) with base, tip, and bytes, or use hunksOfLocal() for the
    // current branch being edited. Returns hunks numbered globally across all
    // files, each with index, start (old-side offset), deleted (count), inserted (text).
    hunksOf(proposalData) {
      if (!proposalData) return [];
      const { base, tip, bytes } = proposalData;
      if (!base || !tip || !bytes) return [];

      try {
        // Rebuild the branch from the session document and the proposal's bytes.
        const doc = session.doc;
        const branch = doc.forkAt(base);
        branch.import(bytes);

        // Compute the diff between the base and the tip of the branch.
        // false = return Diff format (not JsonDiff), which includes TextDelta objects.
        const diff = branch.diff(base, tip, false);

        // Walk the diff batch and extract hunks from text containers.
        // Hunks are numbered globally across all containers, not per-file.
        const hunks = [];
        let hunkIndex = 0;

        // Which file each hunk is in. A hunk without that cannot be drawn:
        // its offsets are into one text, and the editor is showing one text.
        const fileOf = new Map();
        const files = session.doc.getMap("files");
        for (const id of files.keys()) {
          const text = files.get(id);
          if (text?.kind?.() === "Text") fileOf.set(text.id, id);
        }
        for (const [cid, diffValue] of diff) {
          if (diffValue.type !== "text") continue;
          const textHunks = hunkify(diffValue.diff);
          for (const hunk of textHunks) {
            hunk.index = hunkIndex++;
            hunk.file = fileOf.get(cid) ?? null;
          }
          hunks.push(...textHunks);
        }

        branch.destroy?.();
        return hunks;
      } catch (error) {
        // Diff might fail if the base, tip, or bytes are invalid. Return empty.
        return [];
      }
    },

    // Compute the hunks of the local proposal we're currently editing.
    // This is a convenience wrapper around hunksOf() for the proposal this
    // browser is drafting.
    hunksOfLocal() {
      if (!proposal) return [];
      return this.hunksOf(proposal);
    },

    // A reviewer is deciding about a hunk in a proposal.
    // Send the decision to the server. The server will check the tip against
    // the decision's tip to ensure the decision is still valid.
    decide(proposalId, hunkIndex, accepted, note) {
      if (!proposal) return;

      send({
        type: "proposal-decide",
        proposalId,
        hunkIndex,
        accepted,
        note: note || "",
        tip: encodeBase64(encodeFrontiers(proposal.tip)),
      });
    },

    // Private state
    _openProposals: new Map(),
  };
}

// Encode a Uint8Array to base64. Used to wrap Loro's encodeFrontiers result
// and to encode update bytes for transport.
function encodeBase64(bytes) {
  let binary = "";
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return btoa(binary);
}

// Decode a base64 string back to Uint8Array. Used to unwrap encoded frontiers
// and update bytes received from the server.
function decodeBase64(encoded) {
  if (!encoded) return new Uint8Array();
  return Uint8Array.from(atob(encoded), (c) => c.charCodeAt(0));
}
