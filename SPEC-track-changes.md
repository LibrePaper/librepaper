# Track changes

Status: Implemented

## Purpose

Make tracked editing a normal way to write in LibrePaper. A persistent button enables tracking, ordinary edits become reviewable revisions, and the Changes side pane provides a fast way to inspect, accept, and reject them.

The primary workflow must not require selecting a passage, opening a comment dialog, and entering a replacement. Comments support discussion; revisions represent proposed document edits.

## Product decision

Use live tracked edits: proposed edits appear immediately in the working document, with revision metadata identifying what remains pending. Accepting a revision keeps the text and clears its pending status. Rejecting it restores the relevant prior content without undoing unrelated work.

This specification chooses that experience over a separate proposal editor or a diff computed only against a session checkpoint. Checkpoint comparison remains a History feature and is not the source of pending revisions.

## Scope

The first release covers text insertions, deletions, and replacements in editable source files; inline source markup; a dedicated Changes pane; persistence; and collaborative review. Changes made through typing, paste, cut, selection replacement, and dictation use the same tracking path.

File creation, deletion, renaming, binary assets, move detection, and semantic tracking of formatting are outside the first release. Editing formatting syntax in a source file is still a text revision. Direct editing of a rendered preview is not required.

## Enabling tracking

- Place a visibly labeled toggle at the top of the Changes sidebar: `Track changes: Off` or `Track changes: On`. This is the primary tracking control; it belongs in the same pane used to review revisions.
- Tracking applies to subsequent local edits by this user in the current document, across its editable text files. It does not enable tracking for collaborators.
- Remember the setting per user and document, including across reloads. Default to off when no preference exists.
- Keep the toggle visible in the sidebar header even when there are no revisions or the revision list is scrolled. The user can close the pane and continue tracking.
- Disabling tracking does not accept, reject, or hide pending revisions.
- When the sidebar is closed, show an active-tracking indicator on the Changes sidebar opener. Opening it reveals the toggle. Provide an accessible pressed state for the toggle and retain the same placement in the compact-screen pane.
- Tracking does not grant edit or review permissions. Read-only users cannot enable editing through this control.

Tracking and markup visibility are independent settings. Hiding markup must never disable tracking.

## Editing behavior

The normal editable document contains the proposed text. Deleted text remains available in revision records and can be displayed as noneditable markup. Markup is presentation, not literal source syntax.

| Action | Result |
| --- | --- |
| Insert text | One pending insertion |
| Delete existing text | One pending deletion retaining the deleted content |
| Replace a selection | One pending replacement with old and new content |
| Continue typing in an adjacent insertion | Extend that insertion when grouping rules allow |
| Correct your own pending insertion | Update its proposed content |
| Delete your own pending insertion completely | Cancel the insertion; do not create a deletion of previously accepted text |
| Paste a large passage | One revision initially |
| Undo a tracked edit | Undo both its text effect and corresponding revision metadata |
| Redo that edit | Restore both its text effect and revision metadata |

An ordinary undo after another user's edit must preserve the other user's work. Review decisions have a separate undo action described below.

### Grouping

Review units should correspond to meaningful editing actions rather than keystrokes.

- Merge contiguous typing by the same author in the same file and revision session, provided no intervening edit or decision makes the merge ambiguous.
- Merge consecutive adjacent deletions under the same conditions.
- Treat a selection replacement as one decision, even though it contains both a deletion and an insertion.
- Begin a new group when the user moves to a separate passage, changes files, switches tracking off and on, or performs a distinct paste or structural insertion.
- Never group revisions from different authors automatically.
- Grouping must be deterministic and must not depend solely on a timing threshold. An idle timeout may refine grouping but cannot define identity or attribution.
- Preserve enough operation detail to support later splitting. Manual splitting is a follow-up feature, not a first-release requirement.

A revision session starts when tracking is enabled and ends when it is disabled. Reloading while tracking remains enabled resumes the session. Sessions provide filtering and attribution context; they are not all-or-nothing review units.

### Editing pending revisions

Editing your own pending proposal updates that proposal when it remains an independent review unit. Editing another author's pending proposal creates an attributed dependent revision; it must not silently rewrite the original author's proposal.

Turning tracking off does not bypass review for an existing pending span. Edits that alter pending proposed content still update or create revisions according to these rules. Untracked edits outside pending spans apply normally.

When an edit crosses pending and ordinary text, preserve the relationship between its affected spans. If independent acceptance or rejection cannot be performed safely, expose a dependency or conflict for review.

## Changes pane

Replace the suggestion-filtered Comments view with a dedicated revision review interface.

### Layout

- A pinned header contains the Changes title and tracking toggle at the top, followed by the pending count and filters for author, file, and revision session. The tracking control remains separate from accept/reject actions so enabling capture cannot be confused with deciding a revision.
- Default to pending revisions in document order, using stable file order followed by position within each file.
- Display compact rows with a short inline diff, author, and enough location context to identify the passage.
- Expand one active row to show the complete change and surrounding context. Long content may scroll or expand without widening the pane.
- Keep `Accept and next`, `Reject and next`, previous, and next controls in a stable location.
- Allow accept/reject directly from compact rows, with accessible labels.
- Keep optional discussion collapsed under each revision.
- Show accepted and rejected revisions through a separate history/status filter. They should not crowd the default queue.

Selecting a revision opens its file, scrolls to its passage, and highlights it. Selecting revision markup in the editor activates the corresponding row. Deleted passages must remain navigable through their anchored location.

### Fast review

- After a successful decision, activate the next pending revision in the filtered order. If none follows, activate the preceding pending revision; if none remains, show a completed state.
- Keep keyboard focus in the review interface after a decision.
- Provide discoverable shortcuts for previous, next, accept, and reject. Activate review shortcuts only in the review context; do not intercept typing in the editor or discussion fields.
- Provide checkboxes for explicit batch selection. Bulk controls state their scope, for example `Accept 12 selected` or `Reject all 8 filtered`.
- Bulk decisions operate on a captured set of revision IDs, not a moving query that could include newly arriving changes.
- For the first release, revisions with unresolved dependencies or conflicts are excluded from bulk decisions and reported explicitly.
- Report partial success accurately and leave failed revisions pending. Do not stop review of unrelated revisions because one revision needs attention.
- Individual accept/reject actions do not require a confirmation dialog. Offer undo for successful review decisions.

### Decision feedback and undo

While a decision is pending, disable duplicate actions for that revision and retain its visible state. Mark it accepted or rejected only after server confirmation. On failure, retain the pending revision and show an actionable error.

Offer `Undo acceptance` or `Undo rejection` for the last completed review action, including a batch action when supported. Undo acceptance reopens the revision. Undo rejection reapplies its proposal and reopens it. These are new audited operations, not deletion of decision history.

If intervening edits prevent safe undo, preserve the document and explain which passage requires review. Never restore a whole-file snapshot to undo a revision decision.

## Markup and preview

- Source markup distinguishes insertions, deletions, and replacements using more than color alone.
- Deleted text is noneditable decoration; it must not accidentally enter clipboard text or subsequent source edits.
- Provide `Show markup` and `Clean proposed text` display modes. Both show the same underlying proposed document.
- Render and compile the proposed source normally. Pending deletion metadata must not enter compiler input.
- Rendered-preview redlines may be shown when source-to-preview mapping is reliable. For unmappable source edits or unsupported output formats, navigation opens the source and the pane retains the exact diff.
- Preview mapping failures must not block reviewing source revisions or place markup approximately on an unrelated passage.
- Ordinary exports contain the current proposed text. If pending revisions exist, the export interface identifies that they are included. Exporting an accepted-only version is outside the first release.

## Collaboration and persistence

Text and revision metadata must be committed as one logical operation. No collaborator should receive tracked text without the metadata needed to review it, or a pending revision whose text operation was lost.

Each revision needs a stable ID, document and file identity, author identity, session identity, creation/update ordering, operation kind, retained old content, proposed content or operation references, durable anchors, dependency information, and decision history. Exact schema and CRDT representation require an implementation design before coding.

Required invariants:

1. Anchors survive unrelated preceding edits and distinguish repeated passages. Raw character offsets and quotation matching alone are insufficient.
2. Rejected deletions restore content at its surviving anchored location.
3. Accepting a live revision does not apply its insertion or deletion a second time.
4. Rejection and review undo preserve unrelated concurrent edits.
5. Requests are idempotent. Retrying a decision cannot duplicate text or decisions.
6. Concurrent decisions on the same revision have one authoritative outcome, broadcast to all clients.
7. Server-side permissions govern editing, acceptance, rejection, and review undo.
8. Revision records and retained deleted content survive reloads, reconnects, and storage maintenance for as long as review requires them.
9. Untracked remote edits do not become revisions merely because another client has tracking enabled.
10. Reconnecting clients reconcile decisions and dependent edits without silently dropping either.

If a collaborator deletes a file with pending revisions, retain those revisions as requiring attention and identify the missing file. Do not redirect them to a similarly named file or recreate the file automatically.

### Dependencies and conflicts

Two edits touching the same passage are not automatically a conflict. A conflict exists when a requested decision cannot preserve the remaining edits with a well-defined result.

Display affected revisions together with author attribution and a concrete explanation. Do not silently accept, reject, or discard dependent work. The first release may block an unsafe individual decision and provide a source-based resolution flow that records the resolution and affected revision IDs. Unrelated revisions remain reviewable.

## Existing suggestions and History

The current Changes component delegates to Comments with a suggestions filter. Existing suggestions hold proposed replacements that are applied only on acceptance. Live tracked edits have different acceptance semantics and must be explicitly distinguished internally.

- Remove `Suggest a change` from the primary commenting toolbar and replace its primary editing role with the tracking button.
- Preserve existing pending suggestions and their discussions. Present them in the new Changes pane with an explicit legacy proposal type until reviewed.
- Accepting a legacy suggestion applies its replacement once; accepting a live tracked revision only settles its status.
- Preserve existing accepted/rejected suggestion records and attribution.
- Route newly generated external or assistant proposals through an explicit proposal ingestion path; do not relabel unapplied proposals as live revisions.
- Keep checkpoint comparisons in History. Differences since a checkpoint do not automatically become pending revisions.

Commit `6060be0` hardened annotation writes and suggestion acceptance. Reuse applicable authorization, persistence, idempotency, and failure-handling patterns. Its existing suggestion model does not itself implement live tracking.

## Delivery plan

1. Design the operation and anchor representation, atomic synchronization, dependency rules, and recovery behavior. Prove insertion, deletion, replacement, rejection, and concurrent editing behavior before building the full UI.
2. Add the tracking toggle to the pinned Changes sidebar header, an active indicator on its opener, and capture ordinary source edits, including paste, dictation, undo, and redo. Persist tracking preference and revision sessions.
3. Build the dedicated Changes pane, inline source markup, synchronized navigation, and individual review decisions with undo.
4. Add filters, batch decisions, keyboard review, legacy suggestion presentation, and supported preview markup.
5. Verify reconnect, concurrent decisions, storage failure, permissions, and large review queues before release.

## Acceptance criteria

- A user can enable tracking, rewrite a paragraph normally, and review a small set of meaningful revisions without opening a suggestion dialog.
- Replacing a phrase produces one replacement; continued typing and corrections do not create one card per keystroke.
- Deleting a pending insertion cancels it. Rejecting a deletion restores the correct occurrence of repeated text after preceding edits.
- Turning tracking off preserves pending revisions and does not permit silent alteration of another author's proposal.
- Two collaborators can edit separate passages and review revisions without losing either person's work.
- Editing another author's pending insertion retains both authors' attribution and exposes dependencies when relevant.
- Accept/reject retries, simultaneous decisions, and reconnects do not double-apply edits or produce contradictory final status.
- Review proceeds by keyboard through the filtered queue with stable focus and automatic advancement.
- Batch actions affect only their captured selection and report successful, failed, and excluded revisions accurately.
- Review undo preserves intervening unrelated edits or reports why it cannot safely proceed.
- Pending revisions survive reload and storage maintenance, including retained deleted text.
- The proposed source compiles without injected tracking markers. Unmappable preview changes remain reviewable in source.
- Existing suggestions remain reviewable and use the correct apply-on-accept behavior.
- A large queue remains usable: row rendering, filtering, and navigation must avoid mounting full discussions and complete diffs for every revision at once.
