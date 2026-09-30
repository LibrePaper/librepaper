// A proposed edit is a branch: a forked LoroDoc that collects ordinary edits,
// reviewed as the diff between where it forked and where it has reached. This
// replaces the deleted track-changes.js, and is NOT a port of it -- read SPEC
// §1.2 for why that mechanism is gone.
//
// The browser holds one branch per author per session (§10). Typing into it
// accumulates edits; flushing sends the whole branch to the server, which
// stores its bytes and its review state in Postgres and decides which of its
// hunks reach the document.
//
// Diffs stay local to each side; offsets are UTF-16 in the browser and code
// points on the server. Only proposal id, base and tip cross the wire, so a
// hunk is named by its index within a particular diff.

import { encodeFrontiers, decodeFrontiers } from "loro-crdt";

// How much untouched text may sit between two changes before they stop being
// one decision. This must match crates/librepaper/src/document/hunks.rs:SAME_DECISION_WITHIN
// or a reviewer's decision will revert the wrong hunks.
const SAME_DECISION_WITHIN = 8;

// Groups a run of deltas into hunks: maximal runs of non-retain deltas, with
// short gaps (≤ SAME_DECISION_WITHIN) bridging deltas together into one hunk.
// This mirrors hunks.rs:runs() exactly.
function codePointLength(text) {
  return [...text].length;
}

function hunkRanges(deltas, oldText = "") {
  // JavaScript string and diff offsets are UTF-16, while the server groups
  // retains by Unicode code point. Measure the actual retained slices so an
  // emoji does not make the two sides number decisions differently.
  const retainedCodePoints = new Map();
  let oldAt = 0;
  for (let i = 0; i < deltas.length; i++) {
    const delta = deltas[i];
    if (delta.retain !== undefined) {
      retainedCodePoints.set(i, codePointLength(oldText.slice(oldAt, oldAt + delta.retain)));
      oldAt += delta.retain;
    } else if (delta.delete !== undefined) {
      oldAt += delta.delete;
    }
  }
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
        const bridges = retainedCodePoints.get(i) <= SAME_DECISION_WITHIN && hasMoreChanges;
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
  for (const range of hunkRanges(deltas, oldText)) {
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

  let atBase = null;
  let branch = null;
  try {
    // Rebuild the branch: fork the room where the proposal forked, then
    // replay the operations the author made on it. A second fork stays at the
    // base, because that is the text the deltas are against and `hunkify`
    // needs it to fill in what a retain bridged over.
    atBase = doc.forkAt(base);
    branch = doc.forkAt(base);
    branch.import(bytes);
    if (encodeBase64(encodeFrontiers(branch.frontiers())) !==
        encodeBase64(encodeFrontiers(tip))) throw new Error("proposal bytes extend past the advertised tip");

    // false = Diff rather than JsonDiff, which is what carries TextDelta.
    const diff = branch.diff(base, tip, false);

    // Which file each hunk is in. A hunk without one cannot be drawn: its
    // offsets are into one text, and an editor shows one text.
    const fileOf = new Map();
    const pathOf = branch.getMap("paths");
    const files = branch.getMap("files");
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
        hunk.path = hunk.file ? pathOf.get(hunk.file) ?? null : null;
        hunks.push(hunk);
      }
    }

    return hunks;
  } catch {
    return [];
  } finally {
    atBase?.destroy?.();
    branch?.destroy?.();
  }
}

/// Map a base-relative hunk into the current room and report whether its
/// original text identities still exist. Cursors handle unrelated edits
/// before the hunk; the CRDT diff catches delete-and-retype of identical text.
export function locateProposalHunk(doc, proposalData, hunk) {
  if (!doc || !proposalData?.base || !hunk) return { file_id: hunk?.file ?? null, position: 0, stale: true };
  let base = null;
  try {
    base = doc.forkAt(proposalData.base);
    const fileId = hunk.file_id ?? hunk.file;
    const original = base.getMap("files").get(fileId);
    const current = doc.getMap("files").get(fileId);
    if (original?.kind?.() !== "Text") {
      // A proposal can create a new text file. Until another branch claims the
      // same file id, its initial insertion is a live, zero-width hunk.
      const paths = doc.getMap("paths");
      const pathAlreadyExists = hunk.path && [...paths.keys()]
        .some((id) => String(id) !== String(fileId) && paths.get(id) === hunk.path);
      if (!current && !pathAlreadyExists && !(hunk.before ?? "")) {
        return { file_id: fileId ?? null, position: 0, stale: false, path: hunk.path ?? null };
      }
      return { file_id: fileId ?? null, position: 0, stale: true };
    }
    if (current?.kind?.() !== "Text") return { file_id: fileId ?? null, position: 0, stale: true };
    const from = original.convertPos(hunk.start, "utf16", "unicode");
    const to = original.convertPos(hunk.start + hunk.deleted, "utf16", "unicode");
    if (from === undefined || to === undefined) return { file_id: fileId, position: 0, stale: true };
    // Side 1 sticks to the right of concurrent inserts at the start, while
    // side 0 sticks to the left at the end. Boundary insertions stay adjacent.
    const fromCursor = original.getCursor(from, 1);
    const toCursor = original.getCursor(to, 0);
    const fromResult = doc.getCursorPos(fromCursor);
    const toResult = doc.getCursorPos(toCursor);
    if (!fromResult || !toResult) return { file_id: fileId, position: 0, stale: true };
    const position = current.convertPos(fromResult.offset, "unicode", "utf16");
    const end = current.convertPos(toResult.offset, "unicode", "utf16");
    if (position === undefined || end === undefined) return { file_id: fileId, position: 0, stale: true };

    let identityConflict = false;
    const currentFrontiers = doc.frontiers();
    const delta = doc.diff(proposalData.base, currentFrontiers, false)
      .find(([cid]) => String(cid) === String(original.id))?.[1];
    if (delta?.type === "text") {
      const hunkStart = hunk.start;
      const hunkEnd = hunk.start + hunk.deleted;
      let oldAt = 0;
      for (const part of delta.diff) {
        if (part.retain !== undefined) oldAt += part.retain;
        else if (part.delete !== undefined) {
          const a = oldAt, b = oldAt + part.delete;
          if (a < hunkEnd && hunkStart < b) identityConflict = true;
          oldAt = b;
        } else if (part.insert !== undefined) {
          // Inserts at either edge are adjacent; inserts inside a replacement
          // or at the same point as another insertion compete.
          if (hunk.deleted === 0 ? oldAt === hunkStart : hunkStart < oldAt && oldAt < hunkEnd) {
            identityConflict = true;
          }
        }
      }
    }
    const liveText = current.toString().slice(position, end);
    return {
      file_id: fileId,
      position,
      stale: identityConflict || end < position || liveText !== (hunk.before ?? ""),
    };
  } catch {
    return { file_id: hunk.file_id ?? hunk.file ?? null, position: 0, stale: true };
  } finally {
    base?.destroy?.();
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
      for (const range of hunkRanges(value.diff, oldText)) {
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
    decisions: Array.isArray(raw.decisions) ? raw.decisions : [],
  };
}

/// The branch this browser is drafting: one per author per session (§10).
///
/// Only the draft. What everyone else has proposed is the Reader's, because
/// answering one means naming a tip on a socket, and the socket is the
/// Reader's -- see `decideProposal` there. What the two share is
/// `hunksOfProposal` above, so the hunks a reviewer reads and the hunks an
/// author is accumulating are numbered by the same code.
const proposalManagers = new WeakMap();

export function createProposals({ session, send, mayEdit }) {
  const existing = proposalManagers.get(session);
  if (existing) {
    existing.attach(send, mayEdit);
    return existing;
  }
  let sendMessage = send;
  let canEdit = mayEdit;
  const changeListeners = new Set();
  const changed = () => {
    for (const listener of changeListeners) {
      try { listener(); } catch { /* One view must not block other subscribers. */ }
    }
  };
  let proposal = null;
  const retained = new Map();

  const ownOps = (draft, version = draft.branch.oplogVersion()) =>
    version.get(draft.branch.peerIdStr) ?? 0;
  const allDrafts = () => [...retained.values(), ...(proposal ? [proposal] : [])];
  const dirty = (draft) => ownOps(draft) !== draft.ackOwn;

  const hasTextChanges = (base, branch) => {
    try {
      const diff = branch.diff(base, branch.frontiers(), false);
      for (const [, value] of diff) {
        if (value.type === "text" && value.diff.some((delta) =>
          delta.insert !== undefined || delta.delete !== undefined)) return true;
      }
    } catch { return null; /* Preserve the branch when a base cannot be read. */ }
    return false;
  };

  const destroy = (draft) => {
    clearTimeout(draft.retryTimer);
    draft.retryTimer = null;
    draft.unsubscribe?.();
    draft.branch.destroy?.();
    retained.delete(draft.id);
    if (proposal === draft) proposal = null;
    changed();
  };

  const sendOpen = (draft) => {
    if (draft.opened || draft.openPending || !hasTextChanges(draft.base, draft.branch)) return;
    draft.openBase ??= draft.base;
    const result = sendMessage({
      type: "proposal-open",
      request_id: draft.id,
      base: encodeBase64(encodeFrontiers(draft.openBase)),
    });
    if (result?.ok === false) {
      draft.error = result.error?.message || "offline";
      changed();
      return;
    }
    draft.openPending = true;
    draft.error = "";
    changed();
  };

  const sendUpdate = (draft) => {
    if (!draft.opened || draft.openPending || draft.inflight || draft.discarding) return;
    draft.branch.commit();
    const tip = draft.branch.frontiers();
    const tipBytes = encodeBase64(encodeFrontiers(tip));
    const own = ownOps(draft);
    // Room-only operations are absorbed into the branch but are already in
    // the room. They must not cause full-branch proposal rewrites.
    if (own === draft.ackOwn) return;
    const update = draft.branch.export({ mode: "update", from: draft.baseVersion });
    if (!update.byteLength) return;
    const requestId = crypto.randomUUID();
    const message = {
      type: "proposal-update",
      proposal_id: draft.id,
      request_id: requestId,
      tip: tipBytes,
      update: encodeBase64(update),
    };
    if (Number.isSafeInteger(draft.appliedVersion)) message.expected_version = draft.appliedVersion;
    if (draft.baseMoved) message.base = encodeBase64(encodeFrontiers(draft.base));
    const result = sendMessage(message);
    if (result?.ok === false) {
      draft.error = result.error?.message || "offline";
      changed();
      return;
    }
    draft.inflight = {
      requestId,
      own,
      tipBytes,
      base: draft.base,
    };
    draft.error = "";
    changed();
  };

  const flushDraft = (draft) => {
    if (draft.discarding) return;
    draft.branch.commit();
    const textChanged = hasTextChanges(draft.base, draft.branch);
    if (textChanged === null) {
      draft.error = "the draft base is no longer available";
      changed();
      return;
    }
    if (!textChanged) {
      if ((draft.opened || draft.openPending) && !draft.discarding) {
        const requestId = crypto.randomUUID();
        const result = draft.opened
          ? sendMessage({ type: "proposal-discard", proposal_id: draft.id, request_id: requestId })
          : { ok: true };
        if (result?.ok !== false) {
          draft.discarding = true;
          draft.discardCause = "empty";
          draft.discardRequestId = requestId;
          changed();
        }
      }
      return;
    }
    if (!draft.opened) sendOpen(draft);
    else sendUpdate(draft);
  };

  const finishResolution = (draft) => {
    const resolution = draft.resolutionPending;
    if (!resolution) return;
    const decisions = resolution.decisions || [...draft.decisions].map(([hunk, accepted]) => ({ hunk, accepted }));
    const allRejected = resolution.discarded === true ||
      decisions.length > 0 && decisions.every((decision) => !decision.accepted);
    let resolvedTip;
    let resolvedBase;
    try {
      resolvedTip = decodeFrontiers(decodeBase64(resolution.resolved_tip));
      resolvedBase = decodeFrontiers(decodeBase64(resolution.resolved_base));
    } catch {
      draft.error = "the resolved proposal version could not be read";
      changed();
      return;
    }

    // Accepted text arrives in the room as an update. Wait until that graph is
    // present before rebuilding the local remainder. A wholly rejected row is
    // discarded server-side, so replay its inverse privately instead.
    if (!allRejected) {
      let sourceAtResolution = null;
      try { sourceAtResolution = session.doc.forkAt(resolvedTip); } catch { return; }
      sourceAtResolution?.destroy?.();
    } else if (!draft.inverseApplied) {
      let resolvedBranch = null;
      try {
        resolvedBranch = draft.branch.forkAt(resolvedTip);
        const before = resolvedBranch.oplogVersion();
        const inverse = resolvedBranch.diff(resolvedTip, resolvedBase, false)
          .filter(([, diff]) => diff.type === "text");
        const peer = BigInt(`0x${crypto.randomUUID().replaceAll("-", "").slice(0, 16)}`) || 1n;
        resolvedBranch.setPeerId(peer);
        resolvedBranch.applyDiff(inverse);
        resolvedBranch.commit();
        const reverts = resolvedBranch.export({ mode: "update", from: before });
        if (reverts.byteLength) draft.branch.import(reverts);
        draft.inverseApplied = true;
      } catch {
        draft.error = "the rejected proposal could not be removed from its local branch";
        changed();
        return;
      } finally {
        resolvedBranch?.destroy?.();
      }
    }

    // Import the room's current state after applying the private inverse. This
    // also brings in edits that happened after the resolution frontier.
    const missing = session.doc.export({ mode: "update", from: draft.branch.oplogVersion() });
    if (missing.byteLength) {
      try { draft.branch.import(missing); } catch { return; }
    }
    draft.branch.commit();
    draft.base = session.doc.frontiers();
    draft.baseVersion = session.doc.oplogVersion();
    draft.baseMoved = false;
    draft.opened = false;
    draft.openPending = false;
    draft.openBase = null;
    draft.inflight = null;
    draft.ackOwn = ownOps(draft, draft.baseVersion);
    draft.ackTipBytes = encodeBase64(encodeFrontiers(draft.base));
    draft.resolutionPending = null;
    draft.inverseApplied = false;
    draft.decisions.clear();
    draft.error = "";

    // Resolution imports the proposal graph and inverse operations. Keeping
    // this branch preserves a local keystroke inside text that was rejected.
    const remainingChanges = hasTextChanges(draft.base, draft.branch);
    if (remainingChanges === null) {
      draft.error = "the resolved draft base is no longer available";
      changed();
      return;
    }
    if (!remainingChanges) {
      destroy(draft);
      return;
    }
    retained.delete(draft.id);
    draft.id = crypto.randomUUID();
    draft.openBase = null;
    if (proposal === draft) subscribe(draft);
    else retained.set(draft.id, draft);
    flushDraft(draft);
  };

  const absorb = (draft) => {
    draft.branch.commit();
    const update = session.doc.export({ mode: "update", from: draft.branch.oplogVersion() });
    if (update.byteLength) {
      try {
        draft.branch.import(update);
        draft.base = session.doc.frontiers();
        // The replacement update is based at this moved frontier. Operations
        // absent from the room, including this author's proposal, remain in
        // the branch export because their peer counters exceed this version.
        draft.baseVersion = session.doc.oplogVersion();
        draft.baseMoved = true;
      } catch {
        draft.error = "could not merge a room update into this draft";
      }
    }
    if (draft.resolutionPending) finishResolution(draft);
    else if (draft.retryable) retryDraft(draft);
    changed();
  };

  const retryDraft = (draft) => {
    if (!draft.retryable || draft.retryTimer) return;
    draft.retryTimer = setTimeout(() => {
      draft.retryTimer = null;
      if (!allDrafts().includes(draft) || !draft.retryable) return;
      draft.retryable = false;
      flushDraft(draft);
    }, 1500);
  };

  function subscribe(draft) {
    draft.unsubscribe?.();
    draft.unsubscribe = session.doc.subscribe((event) => {
      if (event.by === "import") absorb(draft);
    });
  }

  const api = {
    start() {
      if (proposal) return proposal;
      if (!canEdit) return null;
      const doc = session.doc;
      doc.commit();
      const base = doc.frontiers();
      const branch = doc.fork();
      proposal = {
        id: crypto.randomUUID(),
        openBase: null,
        base,
        baseVersion: branch.oplogVersion(),
        branch,
        opened: false,
        openPending: false,
        baseMoved: false,
        ackOwn: ownOps({ branch }),
        ackTipBytes: encodeBase64(encodeFrontiers(base)),
        decisions: new Map(),
        inflight: null,
        error: "",
        resolutionPending: null,
        appliedVersion: null,
        retryTimer: null,
      };
      subscribe(proposal);
      changed();
      return proposal;
    },
    drafting() { return Boolean(proposal); },
    id() { return proposal?.id || ""; },
    draftMarks() {
      if (!proposal) return [];
      proposal.branch.commit();
      return draftMarksOf(session.doc, proposal.base, proposal.branch);
    },
    doc() { return proposal?.branch || null; },
    text(fileId) {
      if (!proposal) return null;
      const text = proposal.branch.getMap("files").get(fileId);
      return text?.kind?.() === "Text" ? text : null;
    },
    flush() { if (proposal) flushDraft(proposal); },
    reconnect() {
      for (const draft of allDrafts()) {
        clearTimeout(draft.retryTimer);
        draft.retryTimer = null;
        draft.retryable = false;
        draft.openPending = false;
        draft.inflight = null;
        if (draft.opened) {
          // Query the row on reconnect even when our local branch looks clean:
          // we may have missed its final decision while disconnected. The
          // server treats this stable create key as an idempotent status query.
          const result = sendMessage({
            type: "proposal-open",
            request_id: draft.id,
            base: encodeBase64(encodeFrontiers(draft.openBase ?? draft.base)),
          });
          if (result?.ok !== false) draft.openPending = true;
          else draft.error = result.error?.message || "offline";
          continue;
        }
        // A create key is stable; an update is a complete branch snapshot.
        flushDraft(draft);
      }
      changed();
    },
    detach() {
      // The manager belongs to the shared session and survives Editor
      // recreation, so detaching a view must not detach its pending branch.
    },
    attach(nextSend, nextMayEdit) {
      sendMessage = nextSend;
      canEdit = nextMayEdit;
      for (const draft of allDrafts()) if (!draft.unsubscribe) subscribe(draft);
    },
    stop() {
      if (!proposal) return;
      const draft = proposal;
      draft.unsubscribe?.();
      draft.unsubscribe = null;
      flushDraft(draft);
      proposal = null;
      const changedText = hasTextChanges(draft.base, draft.branch);
      if (changedText === false && (draft.opened || draft.openPending)) {
        draft.discarding = true;
        draft.discardCause = "empty";
      }
      if (draft.opened && draft.discarding && !draft.discardRequestId) {
        const requestId = crypto.randomUUID();
        const result = sendMessage({ type: "proposal-discard", proposal_id: draft.id, request_id: requestId });
        if (result?.ok !== false) {
          draft.discarding = true;
          draft.discardRequestId = requestId;
        }
      }
      const needsSync = changedText === null || changedText && dirty(draft);
      if (draft.openPending || !draft.opened && (needsSync || draft.error) || draft.opened && (needsSync || draft.inflight || draft.error || draft.discarding)) {
        retained.set(draft.id, draft);
        subscribe(draft);
      } else destroy(draft);
      changed();
    },
    discard() {
      const draft = proposal;
      if (!draft) return { ok: false, error: new Error("no active draft") };
      if (!draft.opened) {
        if (draft.openPending) {
          draft.discarding = true;
          draft.discardCause = "explicit";
          return { ok: true, pending: true };
        }
        destroy(draft);
        return { ok: true };
      }
      const requestId = crypto.randomUUID();
      const result = sendMessage({ type: "proposal-discard", proposal_id: draft.id, request_id: requestId });
      if (result?.ok === false) return result;
      draft.discarding = true;
      draft.discardCause = "explicit";
      draft.discardRequestId = requestId;
      changed();
      return { ok: true, pending: true };
    },
    status() {
      const draft = proposal || [...retained.values()].at(-1);
      const pending = allDrafts().filter((item) => item.openPending || item.inflight || item.error ||
        hasTextChanges(item.base, item.branch) === null || hasTextChanges(item.base, item.branch) && dirty(item)).length;
      if (!draft) return { state: "idle", error: "", pending };
      return {
        state: draft.error ? "error" : draft.inflight || draft.openPending ? "pending" : draft.opened ? "saved" : "drafting",
        error: draft.error,
        id: draft.id,
        pending,
      };
    },
    onchange(callback) {
      if (typeof callback !== "function") return () => {};
      changeListeners.add(callback);
      return () => { changeListeners.delete(callback); };
    },
    apply(message) {
      if (!message) return;
      if (message.type === "proposal-opened") {
        const id = String(message.proposal_id || message.request_id || "");
        const draft = allDrafts().find((item) => item.id === id);
        if (!draft) return;
        draft.opened = true;
        draft.openPending = false;
        draft.error = "";
        clearTimeout(draft.retryTimer);
        draft.retryTimer = null;
        draft.retryable = false;
        if (Number.isSafeInteger(Number(message.applied_version))) {
          draft.appliedVersion = Number(message.applied_version);
        }
        draft.ackTipBytes = message.tip || encodeBase64(encodeFrontiers(draft.base));
        // An idempotent open may report a row whose update ack was lost.
        // Only treat our current branch as stored when its exact tip matches.
        const currentTip = encodeBase64(encodeFrontiers(draft.branch.frontiers()));
        if (draft.ackTipBytes === currentTip) draft.ackOwn = ownOps(draft);
        changed();
        if (draft.discarding) {
          const requestId = draft.discardRequestId || crypto.randomUUID();
          const result = sendMessage({ type: "proposal-discard", proposal_id: draft.id, request_id: requestId });
          if (result?.ok !== false) draft.discardRequestId = requestId;
        } else flushDraft(draft);
        return;
      }
      if (message.type === "proposal-updated") {
        const draft = retained.get(String(message.proposal_id)) ||
          (proposal?.id === String(message.proposal_id) ? proposal : null);
        if (!draft || !draft.inflight ||
            (message.request_id && message.request_id !== draft.inflight.requestId)) return;
        draft.ackOwn = draft.inflight.own;
        draft.ackTipBytes = draft.inflight.tipBytes;
        if (Number.isSafeInteger(Number(message.applied_version))) {
          draft.appliedVersion = Number(message.applied_version);
        }
        clearTimeout(draft.retryTimer);
        draft.retryTimer = null;
        draft.retryable = false;
        if (encodeBase64(encodeFrontiers(draft.base)) === encodeBase64(encodeFrontiers(draft.inflight.base))) {
          draft.baseMoved = false;
        }
        draft.inflight = null;
        draft.error = "";
        if (draft.resolutionPending) finishResolution(draft);
        else if (draft === proposal && ownOps(draft) !== draft.ackOwn) flushDraft(draft);
        else if (retained.has(draft.id)) {
          if (hasTextChanges(draft.base, draft.branch) && dirty(draft)) flushDraft(draft);
          else destroy(draft);
        }
        changed();
        return;
      }
      if (message.type === "proposal-discarded" || message.type === "proposal-deleted") {
        const draft = retained.get(String(message.proposal_id)) ||
          (proposal?.id === String(message.proposal_id) ? proposal : null);
        if (draft?.discarding && draft.discardCause === "explicit") destroy(draft);
        else if (draft) {
          draft.resolutionPending = { ...message, discarded: true };
          finishResolution(draft);
        }
        return;
      }
      if (message.type === "error") {
        const requestId = String(message.request_id || "");
        const draft = allDrafts().find((item) =>
          item.id === requestId || item.inflight?.requestId === requestId ||
          item.discardRequestId === requestId);
        if (!draft) return;
        const versionConflict = message.version_conflict === true;
        if (versionConflict && Number.isSafeInteger(Number(message.applied_version))) {
          // The row advanced while this client was disconnected. Refresh only
          // the guard; keep our ackOwn watermark so the full local snapshot is
          // retried instead of being mistaken for already stored work.
          draft.appliedVersion = Number(message.applied_version);
        }
        draft.error = versionConflict ? "proposal changed elsewhere; retrying" :
          message.message || "proposal update was refused";
        draft.retryable = versionConflict || Boolean(message.retry);
        if (draft.openPending) draft.openPending = false;
        if (draft.inflight?.requestId === requestId) draft.inflight = null;
        if (draft.retryable) retryDraft(draft);
        changed();
        return;
      }
      if (message.type === "proposal-decided" && !message.resolved && message.proposal_id) {
        const draft = retained.get(String(message.proposal_id)) ||
          (proposal?.id === String(message.proposal_id) ? proposal : null);
        if (draft) {
          if (Array.isArray(message.decisions)) {
            draft.decisions = new Map(message.decisions.map((d) => [Number(d.hunk), Boolean(d.accepted)]));
          } else draft.decisions.set(Number(message.hunk), Boolean(message.accepted));
        }
        changed();
        return;
      }
      if (message.type === "proposal-decided" &&
          (message.resolved || message.resolved_tip && message.resolved_base)) {
        const draft = retained.get(String(message.proposal_id)) ||
          (proposal?.id === String(message.proposal_id) ? proposal : null);
        if (!draft) return;
        if (Array.isArray(message.decisions)) {
          draft.decisions = new Map(message.decisions.map((d) => [Number(d.hunk), Boolean(d.accepted)]));
        } else if (Number.isInteger(message.hunk)) {
          draft.decisions.set(Number(message.hunk), Boolean(message.accepted));
        }
        draft.resolutionPending = message;
        finishResolution(draft);
      }
    },
    hunksOf(proposalData) { return hunksOfProposal(session.doc, proposalData); },
  };
  proposalManagers.set(session, api);
  return api;
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
/// The test is: same file, different proposals, conflicting extents. A row's
/// extent is `[position, position + before.length)`, which is the same basis
/// `proposal-marks.js` draws in, so two rows contend here exactly when they
/// would be drawn over each other there.
///
/// Touching ends do not overlap. An insertion strictly inside a replacement
/// conflicts with it, but an insertion at either boundary is adjacent. Two
/// insertions at the same point compete for the same gap.
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
      const aLength = a.before.length;
      const bLength = b.before.length;
      const overlaps = aLength === 0 && bLength === 0
        ? a.position === b.position
        : aLength === 0
          ? b.position < a.position && a.position < bEnd
          : bLength === 0
            ? a.position < b.position && b.position < aEnd
            : a.position < bEnd && b.position < aEnd;
      if (!overlaps) continue;
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
