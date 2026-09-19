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
  const changesAfter = new Array(deltas.length).fill(false);
  let laterChange = false;
  for (let at = deltas.length - 1; at >= 0; at--) {
    changesAfter[at] = laterChange;
    if (deltas[at].retain === undefined) laterChange = true;
  }
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
        const hasMoreChanges = changesAfter[i];
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
//
// `oldText` is the text the deltas are against. It is needed because a hunk
// may bridge a short retain (`SAME_DECISION_WITHIN`), and a retain carries a
// length and not the words it retained: without them, "cat sat" becoming
// "owl ran" reads as "cat sat" -> "owlran", which is not what anybody typed.
// The server has the same problem and leaves it to its callers
// (`document/hunks.rs`); here the base is in hand, so it is solved once.
function hunkify(deltas, oldText = "") {
  const hunks = [];
  let cursor = 0; // position in the old side of the diff
  let at = 0; // position in the delta array
  for (const range of hunkRanges(deltas)) {
    // Everything before this hunk moves the cursor forward.
    cursor += oldSideLength(deltas.slice(at, range.start));
    let deleted = 0;
    let inserted = "";
    let oldAt = cursor;
    for (let i = range.start; i < range.end; i++) {
      const d = deltas[i];
      if (d.delete !== undefined) {
        deleted += d.delete;
        oldAt += d.delete;
      } else if (d.insert !== undefined) {
        inserted += d.insert;
      } else if (d.retain !== undefined) {
        // Text retained inside a hunk is untouched on both sides: it counts
        // towards how far the hunk reaches on the old side, and it is part of
        // the new side too, in the place it sits.
        inserted += oldText.slice(oldAt, oldAt + d.retain);
        deleted += d.retain;
        oldAt += d.retain;
      }
    }
    hunks.push({
      index: hunks.length,
      start: cursor,
      deleted,
      inserted,
      before: oldText.slice(cursor, cursor + deleted),
      prefix: oldText.slice(Math.max(0, cursor - 24), cursor),
      suffix: oldText.slice(cursor + deleted, cursor + deleted + 24),
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
    // replay the operations the author made on it. A second fork stays at the
    // base, because that is the text the deltas are against and `hunkify`
    // needs it to fill in what a retain bridged over.
    const atBase = doc.forkAt(base);
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
      for (const hunk of hunkify(diffValue.diff, atBase.getContainerById(cid)?.toString?.() ?? "")) {
        hunk.index = index++;
        hunk.file = fileOf.get(String(cid)) ?? null;
        hunks.push(hunk);
      }
    }

    atBase.destroy?.();
    branch.destroy?.();
    return hunks;
  } catch {
    return [];
  }
}

/// What the branch in front of the author has changed, in the branch's own
/// basis.
///
/// This is the other side of `hunksOfProposal`. That one answers "what would
/// this proposal do to the paper", and its offsets are into the base, which is
/// what a reviewer is looking at. An author drafting is looking at the *tip*
/// instead -- the binding is attached to the branch (§3.3) -- so the same hunks
/// drawn there would land in the wrong place: the words they deleted are gone
/// from the text in front of them, and the words they typed are already in it.
///
/// So the deltas are walked with two cursors and reported against the new
/// side: an insertion as a range of text that is really there, a deletion as a
/// point with the vanished words to hang at it. Delta granularity rather than
/// hunk granularity, because a hunk bridges short retains
/// (`SAME_DECISION_WITHIN`) and counts them as removed -- the right grouping
/// for one decision, and the wrong drawing for text nobody touched.
///
/// `[]` when the base cannot be reached, which is what nothing to draw looks
/// like from here.
export function draftMarksOf(doc, base, branch) {
  if (!doc || !base || !branch) return [];
  let old = null;
  try {
    old = doc.forkAt(base);
    const diff = branch.diff(base, branch.frontiers(), false);

    // Which file each delta run belongs to. A mark without one cannot be
    // drawn: its offsets are into one text, and an editor shows one text.
    const fileOf = new Map();
    const files = branch.getMap("files");
    for (const id of files.keys()) {
      const text = files.get(id);
      if (text?.kind?.() === "Text") fileOf.set(String(text.id), id);
    }
    const marks = [];
    // Hunks are numbered across files, the way `hunksOfProposal` numbers them
    // and `proposal-decide` names them. The author is not deciding hunks here,
    // but a mark that knows which one it belongs to can be pointed at from the
    // Changes panel.
    let index = 0;
    for (const [cid, value] of diff) {
      if (value.type !== "text") continue;
      const file = fileOf.get(String(cid)) ?? null;
      // The words a deletion took out. Read from the base by container id,
      // because a delete delta carries a length and not the text it removed,
      // and the container is the same one either side of a fork.
      const oldText = old.getContainerById(cid)?.toString?.() ?? "";
      const hunkAt = new Map();
      for (const range of hunkRanges(value.diff)) {
        for (let i = range.start; i < range.end; i++) hunkAt.set(i, index);
        index += 1;
      }
      let oldAt = 0;
      let newAt = 0;
      value.diff.forEach((delta, i) => {
        const hunk = hunkAt.get(i) ?? 0;
        if (delta.retain !== undefined) {
          oldAt += delta.retain;
          newAt += delta.retain;
          return;
        }
        if (delta.delete !== undefined) {
          marks.push({ kind: "del", file, at: newAt, text: oldText.slice(oldAt, oldAt + delta.delete), hunk });
          oldAt += delta.delete;
          return;
        }
        if (typeof delta.insert === "string" && delta.insert) {
          marks.push({ kind: "ins", file, at: newAt, length: delta.insert.length, hunk });
          newAt += delta.insert.length;
        }
      });
    }
    return marks;
  } catch {
    return [];
  } finally {
    old?.destroy?.();
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
      // Commit before reading the frontier, so the base names only operations
      // that have already been handed to `subscribeLocalUpdates` -- and so
      // sent, on this same socket, ahead of the `proposal-open` below. The
      // server forks at this base and refuses a base it cannot reach; an
      // uncommitted keystroke here would be exactly such a base, and the
      // refusal would arrive as "tracking would not turn on", intermittently.
      doc.commit();
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

    /// The name the server gave this browser's branch, or `""` before the
    /// reply to `proposal-open` has arrived. What it is for: the author is
    /// typing into the branch, so the diff between base and tip is already on
    /// their screen as ordinary text, and drawing it over itself would show
    /// every insertion twice. The editor asks for this to leave its own
    /// draft out of what it paints.
    id() {
      return proposal?.id || "";
    },

    /// What this browser's own draft has changed, ready to draw over the
    /// branch text the author is looking at.
    ///
    /// The branch is committed first for the reason `flush` commits it: the
    /// binding writes into the branch's text and commits the room document, so
    /// without this the frontier would name text the author cannot see yet and
    /// the last few keystrokes would go undrawn.
    draftMarks() {
      if (!proposal) return [];
      proposal.branch.commit();
      return draftMarksOf(session.doc, proposal.base, proposal.branch);
    },

    /// The branch itself, or `null` when nothing is being drafted.
    ///
    /// The editor's binding needs it, not just the text inside it: a binding
    /// commits the document it was given and applies what arrives in it, and
    /// while somebody is drafting, that document is the fork.
    doc() {
      return proposal?.branch || null;
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

      // The binding writes into the branch's text but commits the room
      // document -- it was handed one doc and a getter for the other -- so the
      // branch is carrying uncommitted operations by the time we get here.
      // Committing them is what makes `frontiers()` below name the text the
      // author can see.
      proposal.branch.commit();

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

/// Which review rows are answers to the same question.
///
/// Two proposals that change the same words are not a conflict to be resolved
/// -- they are two answers, and the useful presentation is side by side with
/// one choice to make (SPEC-loro.md §5.3). This finds them.
///
/// The test is: same file, different proposals, overlapping extents. A row's
/// extent is `[position, position + before.length)`, which is the same basis
/// `proposal-marks.js` draws in, so two rows contend here exactly when they
/// would be drawn over each other there.
///
/// Touching ends do not overlap. One proposal replacing "cat" and another
/// inserting after it are adjacent edits, not competing ones, and grouping
/// them would ask the reviewer to choose between two things that can both
/// happen.
///
/// A stale row is left out. Its extent is a claim about text that is no longer
/// there, so it cannot be said to overlap anything, and a reviewer cannot
/// answer it anyway.
///
/// Returns the rows, each with `contested` set to a group id shared by every
/// row in the group, or `""`. Rows are not reordered: the caller decides how
/// to present a group, and the queue's own order is somebody's reading order.
export function markContention(rows) {
  const live = rows.filter((row) => !row.stale && row.file_id);
  // Group ids come from the rows themselves rather than a counter, so the
  // same queue always produces the same ids and a re-render does not look
  // like a change.
  const group = new Map(); // row id -> group id
  const union = new Map(); // group id -> group id it was merged into

  const find = (id) => {
    let at = id;
    while (union.has(at)) at = union.get(at);
    return at;
  };

  for (let i = 0; i < live.length; i++) {
    for (let j = i + 1; j < live.length; j++) {
      const a = live[i];
      const b = live[j];
      if (a.proposal === b.proposal) continue;
      if (a.file_id !== b.file_id) continue;
      const aEnd = a.position + a.before.length;
      const bEnd = b.position + b.before.length;
      // Strict overlap. Two zero-width rows at one point are two insertions
      // in the same gap, which is not a choice between them either.
      if (a.position >= bEnd || b.position >= aEnd) continue;
      const aGroup = group.has(a.id) ? find(group.get(a.id)) : null;
      const bGroup = group.has(b.id) ? find(group.get(b.id)) : null;
      if (aGroup && bGroup) {
        if (aGroup !== bGroup) union.set(bGroup, aGroup);
      } else if (aGroup) {
        group.set(b.id, aGroup);
      } else if (bGroup) {
        group.set(a.id, bGroup);
      } else {
        group.set(a.id, a.id);
        group.set(b.id, a.id);
      }
    }
  }

  return rows.map((row) => ({
    ...row,
    contested: group.has(row.id) ? find(group.get(row.id)) : "",
  }));
}

/// The document as a set of proposals would leave it, as text.
///
/// Because a proposal is a fork rather than an annotation, any subset of them
/// can be merged into a scratch document and read as finished prose (§5.3):
/// "the paper with Alice's introduction and Bob's conclusion", without
/// committing to either.
///
/// This is computed here rather than asked of the server. §5.3 proposed a
/// `proposal-preview` message for it, and that is unnecessary: `proposal-list`
/// already carries every open proposal's branch bytes, so this browser can
/// fork and merge locally. That also keeps §5.2's rule intact, since nothing
/// about a diff crosses the wire.
///
/// The scratch document is thrown away. Nothing syncs to it, nothing is
/// persisted, and the room document is not touched -- `fork()` is what makes
/// that true rather than a discipline to maintain.
///
/// Returns a Map of file id to text, or null if the merge could not be done.
export function previewTexts(doc, proposals, chosen) {
  if (!doc || !chosen?.length) return null;
  const wanted = new Set(chosen);
  let scratch = null;
  try {
    scratch = doc.fork();
    let applied = 0;
    for (const proposal of proposals) {
      if (!wanted.has(proposal.id) || !proposal.bytes) continue;
      // Importing a branch brings its operations into the scratch document
      // and merges them with everything already there, which is what makes a
      // subset readable as one piece of prose rather than a list of patches.
      scratch.import(proposal.bytes);
      applied += 1;
    }
    if (!applied) return null;
    const out = new Map();
    const files = scratch.getMap("files");
    for (const id of files.keys()) {
      const text = files.get(id);
      if (text?.kind?.() === "Text") out.set(id, text.toString());
    }
    return out;
  } catch {
    return null;
  } finally {
    scratch?.destroy?.();
  }
}
