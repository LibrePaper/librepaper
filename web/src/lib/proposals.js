// A proposed edit is a branch: a forked LoroDoc that collects ordinary edits,
// reviewed as the diff between where it forked and where it has reached. This
// replaces the deleted track-changes.js, and is NOT a port of it — read SPEC
// §1.2 for why that mechanism is gone.
//
// The browser holds one branch per author per session (§10). Typing into it
// accumulates edits; flushing sends the whole branch to the server, which
// stores its bytes and its review state in Postgres and decides which of its
// hunks reach the document.
//
// The diff is computed on both sides and travels on neither: the server counts
// text in code points where a browser counts UTF-16, so the two disagree about
// every offset and §5.2 keeps offsets off the wire. A hunk is named by its
// index within the diff of a named tip against a named base, which is the one
// name that means the same thing to both.

import { encodeFrontiers, decodeFrontiers } from "loro-crdt";

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
      // Filled in by the caller, which is the only place that knows which
      // container this delta run came from.
      file: /** @type {string | null} */ (null),
    });
    cursor += deleted;
    at = range.end;
  }
  return hunks;
}

/// The hunks of one proposal, in this browser's own UTF-16 basis.
///
/// Both sides compute this and neither sends it: the server counts text in
/// code points and the browser in UTF-16, so a delta run means different
/// things to each of them and §5.2 keeps it off the wire entirely. What
/// crosses is three identifiers -- a proposal, a base and a tip -- and this is
/// what they name here.
///
/// `[]` when the branch cannot be rebuilt, which is what a proposal whose base
/// the room has since moved past looks like from here.
export function hunksOfProposal(doc, proposalData) {
  if (!doc || !proposalData) return [];
  const { base, tip, bytes } = proposalData;
  if (!base || !tip || !bytes) return [];

  try {
    // Rebuild the branch: fork the room where the proposal forked, then
    // replay the operations the author made on it.
    const branch = doc.forkAt(base);
    branch.import(bytes);

    // false = Diff rather than JsonDiff, which is what carries TextDelta.
    const diff = branch.diff(base, tip, false);

    // Which file each hunk is in. A hunk without one cannot be drawn: its
    // offsets are into one text, and an editor shows one text.
    const fileOf = new Map();
    const files = doc.getMap("files");
    for (const id of files.keys()) {
      const text = files.get(id);
      if (text?.kind?.() === "Text") fileOf.set(String(text.id), id);
    }

    const hunks = [];
    let index = 0;
    for (const [cid, diffValue] of diff) {
      if (diffValue.type !== "text") continue;
      for (const hunk of hunkify(diffValue.diff)) {
        hunk.index = index++;
        hunk.file = fileOf.get(String(cid)) ?? null;
        hunks.push(hunk);
      }
    }

    branch.destroy?.();
    return hunks;
  } catch {
    return [];
  }
}

/// One proposal as it arrives on `proposal-list`, ready to work with.
///
/// `tipBytes` is kept exactly as it came. A decision has to name the tip it
/// was computed against and the server compares those bytes rather than the
/// frontier they mean (`room/proposals.rs`), so the string is echoed back
/// rather than decoded and encoded again -- two encodings of one frontier
/// need not be the same bytes, and a decision that misses is refused as stale
/// with nothing to show for it.
export function decodeProposal(raw) {
  if (!raw?.id) return null;
  return {
    id: String(raw.id),
    author: raw.author || "",
    base: decodeFrontiers(decodeBase64(raw.base)),
    tip: decodeFrontiers(decodeBase64(raw.tip)),
    tipBytes: raw.tip || "",
    bytes: raw.branch ? decodeBase64(raw.branch) : null,
  };
}

/// The branch this browser is drafting: one per author per session (§10).
///
/// Only the draft. What everyone else has proposed is the Reader's, because
/// answering one means naming a tip on a socket, and the socket is the
/// Reader's -- see `decideProposal` there. What the two share is
/// `hunksOfProposal` above, so the hunks a reviewer reads and the hunks an
/// author is accumulating are numbered by the same code.
export function createProposals({ session, send, mayEdit }) {
  let proposal = null;

  return {
    // Fork the session document at its current frontier and remember the base.
    // Returns the existing proposal if one is already open.
    start() {
      if (proposal) return proposal;
      if (!mayEdit) return null;

      const doc = session.doc;
      const base = doc.frontiers();
      const branch = doc.fork();

      proposal = {
        id: null, // named by the server, on `proposal-opened`
        base,
        // The branch's own operations are everything after this. Each flush
        // exports from here rather than from the last flush: see `flush`.
        baseVersion: branch.oplogVersion(),
        tip: base,
        branch,
        // A flush that happened before the server named this proposal. There
        // is nothing to address an update to until then, so it is repeated
        // when the name arrives rather than dropped.
        unsent: false,
      };

      // Tell the server we've opened a proposal at this frontier.
      send({
        type: "proposal-open",
        base: encodeBase64(encodeFrontiers(base)),
      });

      return proposal;
    },

    /// Whether this browser is drafting one.
    drafting() {
      return Boolean(proposal);
    },

    // Get the text at a file id from the branch. The caller types into it.
    text(fileId) {
      if (!proposal) return null;
      const files = proposal.branch.getMap("files");
      const text = files.get(fileId);
      return text?.kind?.() === "Text" ? text : null;
    },

    // Send the branch to the server, and say where its tip now is.
    flush() {
      if (!proposal) return;

      // Everything since the fork, not everything since the last flush.
      //
      // `update_proposal` replaces the stored branch rather than appending to
      // it -- deliberately, so that the stored blob and the stored tip can
      // never disagree -- so an incremental update leaves the server holding
      // only the tail. The proposal then reads as though the author had
      // written their last few keystrokes and nothing before them.
      const update = proposal.branch.export({ mode: "update", from: proposal.baseVersion });
      if (!update.byteLength) return;

      proposal.tip = proposal.branch.frontiers();

      // An update has to say which proposal it belongs to, and there is no
      // name for it until the server sends one back.
      if (!proposal.id) {
        proposal.unsent = true;
        return;
      }

      send({
        type: "proposal-update",
        proposal_id: proposal.id,
        tip: encodeBase64(encodeFrontiers(proposal.tip)),
        update: encodeBase64(update),
      });
    },

    // Close the local branch. The server owns the decision from here on.
    stop() {
      if (!proposal) return;
      proposal.branch.destroy?.();
      proposal = null;
    },

    // Handle incoming messages from the server.
    apply(message) {
      if (message.type === "proposal-opened") {
        // The name for the branch this browser forked. Everything sent about
        // a proposal is addressed by it, so anything typed while the reply
        // was in flight goes up now.
        if (proposal && !proposal.id && message.proposal_id) {
          proposal.id = String(message.proposal_id);
          if (proposal.unsent) {
            proposal.unsent = false;
            this.flush();
          }
        }
        return;
      }
      if (message.type === "proposal-decided") {
        // A proposal is finished with when it resolves, not when one of its
        // hunks is answered: until every hunk has an answer the document has
        // not moved and the rest are still to be decided (§5.1a).
        if (!message.resolved) return;
        // And only close the local branch when the proposal that resolved is
        // the one this browser is drafting -- closing on anyone's decision
        // would throw away unsent work the moment a coauthor answered a
        // different proposal.
        if (proposal && message.proposal_id === proposal.id) this.stop();
      }
    },

    /// The hunks of a proposal the server has told us about, or of any
    /// {base, tip, bytes} triple. Numbered across every file it touches, in
    /// the order `proposal-decide` names them.
    hunksOf(proposalData) {
      return hunksOfProposal(session.doc, proposalData);
    },
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
