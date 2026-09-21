//! Where a comment's passage is now.
//!
//! [`super::locate`] answered "what is this comment about" once, and that
//! answer never changes again. This answers a different question, asked afresh
//! every time the document is served: given the passage a comment was made
//! about, where is that passage in the document as it stands, and is it still
//! the passage it was?
//!
//! The answer is a [`DerivedAttachment`]: a cache, not a record. It is keyed
//! by the projection digest it was computed against and is thrown away and
//! recomputed whenever the source moves. Nothing it contains is ever written
//! back into the original anchor -- not the offsets it finds, and not the
//! replacement cursors Loro hands back, which belong to the cache for the
//! same reason. Under SPEC-server-is-a-log §8.3 `annotation_live_state` is
//! gone: the cursor pair that used to live there now lives only in the
//! room's bounded attachment cache (`Room::attach`, below), and a
//! process that starts cold recomputes it from scratch rather than reading it
//! back from anywhere.
//!
//! Loro's own history is the first and best answer. A cursor captured when the
//! comment was made follows the characters it was put between, through every
//! insertion and deletion since, and comes back knowing where they went. A
//! cursor that resolves is not by itself proof that anything survived --
//! Loro deliberately relocates a cursor whose content was deleted to the
//! boundary where it used to be -- so what it says is checked against the text
//! that was quoted. Only when there are no cursors, or they no longer name the
//! file, does this fall back to looking for the words; and a fallback that
//! finds two equally good candidates says `Ambiguous` rather than choosing.

use std::collections::HashMap;

use loro::cursor::{Cursor, Side};
use loro::Frontiers;
use serde_json::json;

use super::annotation::{
    AnchorSide, AnchorStatus, CommentTarget, DerivedAttachment, LiveSourceRange, OriginalAnchor,
    ResolutionDiagnostic, SourceTextTarget,
};
use super::text::slice16;
use super::{Comment, Room};
use crate::document::session::{self, CursorResolutionError};
use crate::log::sequencer::SequencerError;

/// The encoding of the cursor bytes stored beside a comment. Written with
/// every pair, so a future change of Loro's cursor format is a recognisable
/// mismatch rather than a decode that quietly returns nonsense.
pub(crate) const CURSOR_FORMAT: &str = "loro-1.16-postcard";

/// Captures the pair of cursors that will follow a passage through later
/// edits. Called once, when the comment is made, against the checkpoint the
/// range was verified in.
///
/// A missing pair is not an error: a document served without its CRDT state
/// still gets comments, they simply resolve by their words instead.
pub(crate) fn capture(doc: &loro::LoroDoc, target: &SourceTextTarget) -> Option<LiveSourceRange> {
    let start = session::cursor_at_file_id(
        doc,
        &target.file_id.0,
        target.start_utf16,
        side_of(target.start_side),
    )?;
    let end = session::cursor_at_file_id(
        doc,
        &target.file_id.0,
        target.end_utf16,
        side_of(target.end_side),
    )?;
    Some(LiveSourceRange {
        start_cursor: session::encode_cursor(&start),
        end_cursor: session::encode_cursor(&end),
        cursor_format: CURSOR_FORMAT.to_string(),
    })
}

fn side_of(side: AnchorSide) -> Side {
    match side {
        AnchorSide::Left => Side::Left,
        AnchorSide::Right => Side::Right,
    }
}

fn diagnostic_of(error: CursorResolutionError) -> ResolutionDiagnostic {
    match error {
        CursorResolutionError::MissingFile => ResolutionDiagnostic::RemovedFile,
        CursorResolutionError::MissingContainer => ResolutionDiagnostic::DeletedContainer,
        CursorResolutionError::MalformedCursor => ResolutionDiagnostic::MalformedCursor,
        CursorResolutionError::ForeignContainer => ResolutionDiagnostic::ForeignContainer,
        CursorResolutionError::InvalidPosition => ResolutionDiagnostic::InvalidRange,
    }
}

/// The document's files, read once for a whole pass.
///
/// Every comment on a document is resolved together, whenever that document
/// changes, and reading the files out of the CRDT is the expensive part of
/// doing so. Reading them once per pass rather than once per comment is the
/// difference between a room with five hundred comments being usable and not.
///
/// The files are held as UTF-16 units rather than as `String`s, because that
/// is the only form anything here reads them in: offsets are counted the way
/// a browser counts them. Converting once per file per pass rather than once
/// per comment is the same economy as reading them once, and for the same
/// reason -- otherwise a five-hundred-comment room converts a whole file five
/// hundred times on every keystroke, to slice eight characters out of it.
pub(crate) struct Sources {
    paths: std::collections::HashMap<String, String>,
    units: std::collections::BTreeMap<String, Vec<u16>>,
}

impl Sources {
    pub(crate) fn of(doc: &loro::LoroDoc) -> Sources {
        Sources {
            paths: session::paths_of(doc),
            units: session::texts_of(doc)
                .into_iter()
                .map(|(path, text)| (path, text.encode_utf16().collect()))
                .collect(),
        }
    }

    fn units_of(&self, file_id: &str) -> Option<&[u16]> {
        let path = self.paths.get(file_id)?;
        self.units.get(path).map(Vec::as_slice)
    }
}

/// Works out where a comment's passage is in the document `doc` stands for.
///
/// `live` is whatever the last resolution stored: the cursors it was given, or
/// the replacements Loro handed back. The attachment returned carries the
/// cursors to store for next time, which may be the same ones.
pub(crate) fn resolve(
    doc: &loro::LoroDoc,
    sources: &Sources,
    tree_digest: &str,
    anchor: &OriginalAnchor,
    live: Option<&LiveSourceRange>,
) -> DerivedAttachment {
    let target = match &anchor.target {
        // A comment on the document as a whole is about the document as a
        // whole, which is still here.
        CommentTarget::Document => {
            return attached(tree_digest, AnchorStatus::Exact, None, None, None)
        }
        CommentTarget::SourceText(target) => target,
    };
    let Some(units) = sources.units_of(&target.file_id.0) else {
        // The file it names is gone, or is no longer a text. Either way this
        // cannot say where the passage went, which is not the same as saying
        // it was deleted.
        return attached(
            tree_digest,
            AnchorStatus::Unresolved,
            None,
            None,
            Some(ResolutionDiagnostic::RemovedFile),
        );
    };
    if let Some(live) = live {
        match through_cursors(doc, target, live, units) {
            Ok(attachment) => return attachment.with_digest(tree_digest),
            Err(diagnostic) => {
                // The cursors are no help, but the words may still be. A
                // diagnostic is kept either way: "the file was renamed out
                // from under this" and "we had to go looking" are different
                // things to show.
                let mut found = anchored_by_words(doc, target, units, tree_digest);
                found.diagnostic = Some(diagnostic);
                return found;
            }
        }
    }
    anchored_by_words(doc, target, units, tree_digest)
}

/// The fallback, with cursors taken at whatever it found.
///
/// Looking for the words means reading the whole file, and a document is
/// resolved on every edit: a comment that has to be searched for once would
/// otherwise be searched for again on every keystroke after it. Cursors at
/// the range just found make the next resolution a lookup instead -- and they
/// are cache, like the range beside them, so taking them changes nothing about
/// what the comment is about.
fn anchored_by_words(
    doc: &loro::LoroDoc,
    target: &SourceTextTarget,
    units: &[u16],
    tree_digest: &str,
) -> DerivedAttachment {
    let mut found = by_words(target, units, tree_digest);
    if let (true, Some((start, end))) = (found.status.is_placed(), found.resolved_range_utf16) {
        found.live_source_range = capture(
            doc,
            &SourceTextTarget {
                start_utf16: start,
                end_utf16: end,
                ..target.clone()
            },
        );
    }
    found
}

/// Resolution through Loro's own history, which is the answer whenever it is
/// available.
fn through_cursors(
    doc: &loro::LoroDoc,
    target: &SourceTextTarget,
    live: &LiveSourceRange,
    units: &[u16],
) -> Result<DerivedAttachment, ResolutionDiagnostic> {
    if live.cursor_format != CURSOR_FORMAT {
        return Err(ResolutionDiagnostic::MalformedCursor);
    }
    let (Some(start), Some(end)) = (
        session::decode_cursor(&live.start_cursor),
        session::decode_cursor(&live.end_cursor),
    ) else {
        return Err(ResolutionDiagnostic::MalformedCursor);
    };
    let range = session::offsets_of_cursors_in_file(doc, &target.file_id.0, &start, &end)
        .map_err(diagnostic_of)?;
    let (from, to) = (range.start_utf16, range.end_utf16);
    // Loro relocates a cursor whose content was deleted to the boundary the
    // content used to occupy, so a pair that resolves may well have collapsed
    // onto that boundary. Crossed endpoints mean the same thing.
    if to <= from {
        return Ok(attached(
            "",
            AnchorStatus::Deleted,
            Some(replaced(
                live,
                &range.start_replacement,
                &range.end_replacement,
            )),
            Some((from, from)),
            None,
        ));
    }
    let current = slice16(units, from as usize, to as usize);
    let status = if current == target.exact {
        AnchorStatus::Exact
    } else {
        AnchorStatus::Modified
    };
    Ok(attached(
        "",
        status,
        Some(replaced(
            live,
            &range.start_replacement,
            &range.end_replacement,
        )),
        Some((from, to)),
        None,
    ))
}

/// What Loro says to store for next time: its replacements where it offered
/// them, and what was already there where it did not.
fn replaced(
    live: &LiveSourceRange,
    start: &Option<Cursor>,
    end: &Option<Cursor>,
) -> LiveSourceRange {
    LiveSourceRange {
        start_cursor: start
            .as_ref()
            .map(session::encode_cursor)
            .unwrap_or_else(|| live.start_cursor.clone()),
        end_cursor: end
            .as_ref()
            .map(session::encode_cursor)
            .unwrap_or_else(|| live.end_cursor.clone()),
        cursor_format: CURSOR_FORMAT.to_string(),
    }
}

/// The fallback: the quoted words, in the file they were quoted from.
///
/// This is evidence, not identity. It can say where a passage probably is now;
/// it cannot say what the comment is about, and it never writes an answer back
/// into the anchor. Two equally good candidates are reported as such.
fn by_words(target: &SourceTextTarget, units: &[u16], tree_digest: &str) -> DerivedAttachment {
    // Every source range has words: `locate` refuses a selection that
    // flattens to nothing, so a target with no `exact` is a record from
    // before that was true and there is nothing to look for.
    if target.exact.is_empty() {
        return attached(tree_digest, AnchorStatus::Unresolved, None, None, None);
    }
    let wanted: Vec<u16> = target.exact.encode_utf16().collect();
    let prefix: Vec<u16> = target.prefix.encode_utf16().collect();
    let suffix: Vec<u16> = target.suffix.encode_utf16().collect();
    let mut best: Option<(usize, usize)> = None;
    let mut ties = 0;
    if wanted.len() <= units.len() {
        for at in 0..=(units.len() - wanted.len()) {
            if units[at..at + wanted.len()] != wanted[..] {
                continue;
            }
            let before = &units[..at];
            let after = &units[at + wanted.len()..];
            let score = common_suffix(&prefix, before) + common_prefix(&suffix, after);
            match best {
                Some((_, previous)) if score < previous => {}
                Some((_, previous)) if score == previous => ties += 1,
                _ => {
                    best = Some((at, score));
                    ties = 1;
                }
            }
        }
    }
    let Some((at, _)) = best else {
        // The file is here and the words are not: what this comment was about
        // is gone. That is a thing to show, not an error to swallow.
        return attached(tree_digest, AnchorStatus::Deleted, None, None, None);
    };
    let status = if ties > 1 {
        AnchorStatus::Ambiguous
    } else if at as u32 == target.start_utf16 {
        AnchorStatus::Exact
    } else {
        AnchorStatus::Modified
    };
    attached(
        tree_digest,
        status,
        None,
        Some((at as u32, (at + wanted.len()) as u32)),
        None,
    )
}

fn attached(
    tree_digest: &str,
    status: AnchorStatus,
    live_source_range: Option<LiveSourceRange>,
    resolved_range_utf16: Option<(u32, u32)>,
    diagnostic: Option<ResolutionDiagnostic>,
) -> DerivedAttachment {
    DerivedAttachment {
        tree_digest: tree_digest.to_string(),
        status,
        live_source_range,
        resolved_range_utf16,
        diagnostic,
    }
}

impl DerivedAttachment {
    fn with_digest(mut self, tree_digest: &str) -> DerivedAttachment {
        self.tree_digest = tree_digest.to_string();
        self
    }
}

fn common_prefix(a: &[u16], b: &[u16]) -> usize {
    a.iter().zip(b.iter()).take_while(|(x, y)| x == y).count()
}

fn common_suffix(a: &[u16], b: &[u16]) -> usize {
    a.iter()
        .rev()
        .zip(b.iter().rev())
        .take_while(|(x, y)| x == y)
        .count()
}

impl Room {
    /// Fills in where each of these comments' passages is now, and
    /// remembers the answer.
    ///
    /// This replaces the pass that re-anchored the room's resident copy of
    /// every comment on the document. It does the same work over the same
    /// evidence, but over one page at a time, and what it keeps is bounded:
    /// [`super::AttachmentCache`] holds the cursor pairs and the last
    /// answer for at most `ATTACHMENT_CACHE_MAX` comments and
    /// `ATTACHMENT_CACHE_BYTES` of quoted context.
    ///
    /// A comment with no remembered cursor pair is given one by forking the
    /// document at the state the comment names (`anchor.frontier`, via
    /// `with_fork_at`, §4.3) and capturing there, because the anchor's own
    /// offsets are only valid at that state. A cursor captured on a fork
    /// still resolves against head afterwards: Loro cursors name an
    /// operation, not an offset, and the fork shares head's history. Once
    /// cached, later passes resolve against head directly. An eviction
    /// costs that fork again, which is the same cost a cold process
    /// already paid.
    ///
    /// Failure is not an error here: a sequencer that cannot build a
    /// projection leaves these comments with no attachment, which every
    /// caller already reads as "not yet known".
    pub(crate) async fn attach(&self, comments: &mut [Comment]) {
        if comments.is_empty() {
            return;
        }
        // What is already known, and what has to be captured first.
        let mut needing: Vec<(String, Frontiers, SourceTextTarget)> = Vec::new();
        {
            let mut cache = self.attachments.lock().await;
            for comment in comments.iter() {
                let Some(anchor) = comment.original_anchor.as_ref() else {
                    continue;
                };
                cache.remember(&comment.id, anchor);
                let known = cache
                    .attachment(&comment.id)
                    .and_then(|found| found.live_source_range.as_ref())
                    .is_some();
                if known {
                    continue;
                }
                let Some(source) = anchor.target.source() else {
                    continue;
                };
                let Ok(frontier) = Frontiers::decode(&anchor.frontier) else {
                    continue;
                };
                needing.push((comment.id.clone(), frontier, source.clone()));
            }
        }
        let mut captured: HashMap<String, LiveSourceRange> = HashMap::new();
        for (id, frontier, source) in needing {
            if let Ok(Some(live)) = self
                .log()
                .with_fork_at(&frontier, move |doc| capture(doc, &source))
                .await
            {
                captured.insert(id, live);
            }
        }
        let known: HashMap<String, Option<LiveSourceRange>> = {
            let cache = self.attachments.lock().await;
            comments
                .iter()
                .map(|comment| {
                    let held = cache
                        .attachment(&comment.id)
                        .and_then(|found| found.live_source_range.clone())
                        .or_else(|| captured.get(&comment.id).cloned());
                    (comment.id.clone(), held)
                })
                .collect()
        };
        let resolved = self
            .log()
            .with_head(|doc| {
                let sources = Sources::of(doc);
                let tree_digest = librepaper_document_core::project(doc, &self.config().paths())
                    .projection
                    .digest();
                let mut found = Vec::with_capacity(comments.len());
                for comment in comments.iter() {
                    let Some(anchor) = comment.original_anchor.as_ref() else {
                        continue;
                    };
                    let live = known.get(&comment.id).cloned().flatten();
                    found.push((
                        comment.id.clone(),
                        resolve(doc, &sources, &tree_digest, anchor, live.as_ref()),
                    ));
                }
                found
            })
            .await;
        let Ok(resolved) = resolved else { return };
        let mut cache = self.attachments.lock().await;
        let mut by_id: HashMap<String, DerivedAttachment> = HashMap::new();
        for (id, found) in resolved {
            cache.set_attachment(&id, found.clone());
            by_id.insert(id, found);
        }
        drop(cache);
        for comment in comments.iter_mut() {
            if let Some(found) = by_id.remove(&comment.id) {
                comment.attachment = Some(found);
            }
        }
    }

    /// Re-anchors every comment this room still remembers an anchor for,
    /// and says which ones moved.
    ///
    /// Bounded by the attachment cache rather than by the document's
    /// comment count: a comment nobody has read on this process is not
    /// remembered, so it is not re-resolved and nothing is broadcast about
    /// it. Whoever pages it in next resolves it then.
    pub(crate) async fn reattach_comments(
        &self,
    ) -> Result<Vec<(String, DerivedAttachment)>, SequencerError> {
        let held: Vec<(
            String,
            OriginalAnchor,
            Option<LiveSourceRange>,
            Option<DerivedAttachment>,
        )> = {
            let cache = self.attachments.lock().await;
            cache
                .ids()
                .into_iter()
                .filter_map(|id| {
                    let anchor = cache.anchor(&id)?.clone();
                    let found = cache.attachment(&id).cloned();
                    let live = found
                        .as_ref()
                        .and_then(|found| found.live_source_range.clone());
                    Some((id, anchor, live, found))
                })
                .collect()
        };
        if held.is_empty() {
            return Ok(Vec::new());
        }
        let moved = self
            .log()
            .with_head(|doc| {
                let sources = Sources::of(doc);
                let tree_digest = librepaper_document_core::project(doc, &self.config().paths())
                    .projection
                    .digest();
                let mut moved = Vec::new();
                for (id, anchor, live, before) in &held {
                    let found = resolve(doc, &sources, &tree_digest, anchor, live.as_ref());
                    if before.as_ref() != Some(&found) {
                        moved.push((id.clone(), found));
                    }
                }
                moved
            })
            .await?;
        let mut cache = self.attachments.lock().await;
        for (id, found) in &moved {
            cache.set_attachment(id, found.clone());
        }
        Ok(moved)
    }

    /// §4.5's cadence, applied to comments instead of to a reader's digest:
    /// re-anchors and broadcasts at most once per housekeeping pass, and
    /// only when there is something to do.
    ///
    /// Before the cutover this ran at the end of every accepted update, on
    /// the write path. That is exactly the per-keystroke cost §1 and §5
    /// exist to remove: `ingest` is a header decode, a gap check, an append
    /// and a relay, full stop. So this is driven from `Rooms::housekeep`
    /// instead, coalesced by construction (it only ever acts once per
    /// digest, however many edits produced that digest) and free when
    /// nothing is watching: a room whose attachment cache is empty is left
    /// alone, and a document cache that is not already warm is not built
    /// for this, the same restraint `Sequencer::projection_if_warm` gives a
    /// reader's `source-changed`.
    pub(crate) async fn reattach_comments_if_moved(&self) {
        if self.attachments.lock().await.ids().is_empty() {
            return;
        }
        let Some(projection) = self.log().projection_if_warm().await else {
            return;
        };
        let digest = projection.projection.digest();
        {
            let mut last = self.reattached_digest.lock().await;
            if last.as_deref() == Some(digest.as_str()) {
                return;
            }
            *last = Some(digest);
        }
        let Ok(moved) = self.reattach_comments().await else {
            return;
        };
        // Only editors: where a passage is in the source is source, and a
        // reader of a published render is never told about it. Chunked, so
        // one pass over a full attachment cache is still a bounded frame.
        for chunk in moved.chunks(ATTACHMENT_FRAME_MAX) {
            let payload = json!({
                "type": "attachments",
                "attachments": chunk.iter().map(|(id, attachment)| {
                    json!({"comment_id": id, "attachment": attachment})
                }).collect::<Vec<_>>(),
            });
            self.broadcast_editors_except(None, &payload).await;
        }
    }
}

/// How many moved passages one `attachments` frame may name.
pub(crate) const ATTACHMENT_FRAME_MAX: usize = 200;

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::document::session;
    use crate::room::annotation::FileId;

    /// A document with one text file, the way a publish builds one.
    pub(super) fn document(path: &str, body: &str) -> (loro::LoroDoc, String) {
        let doc = session::new_doc();
        let id = session::put_text(&doc, path, body);
        (doc, id)
    }

    pub(super) fn target(file_id: &str, start: u32, end: u32, exact: &str) -> SourceTextTarget {
        SourceTextTarget {
            file_id: FileId(file_id.to_string()),
            start_utf16: start,
            end_utf16: end,
            start_side: AnchorSide::Left,
            end_side: AnchorSide::Right,
            exact: exact.to_string(),
            prefix: String::new(),
            suffix: String::new(),
        }
    }

    pub(super) fn anchor(target: SourceTextTarget) -> OriginalAnchor {
        OriginalAnchor {
            source_sequence: 1,
            frontier: Vec::new(),
            target: CommentTarget::SourceText(target),
        }
    }

    #[test]
    fn an_untouched_passage_resolves_exactly_where_it_was() {
        let (doc, id) = document("paper.md", "The interval covers the mean.");
        let wanted = target(&id, 4, 12, "interval");
        let live = capture(&doc, &wanted).expect("cursors");
        let found = resolve(&doc, &Sources::of(&doc), "b", &anchor(wanted), Some(&live));
        assert_eq!(found.status, AnchorStatus::Exact);
        assert_eq!(found.resolved_range_utf16, Some((4, 12)));
    }

    #[test]
    fn text_inserted_before_a_passage_moves_it_without_changing_it() {
        let (doc, id) = document("paper.md", "The interval covers the mean.");
        let wanted = target(&id, 4, 12, "interval");
        let live = capture(&doc, &wanted).expect("cursors");
        session::put_text(&doc, "paper.md", "Well. The interval covers the mean.");
        let found = resolve(&doc, &Sources::of(&doc), "b", &anchor(wanted), Some(&live));
        assert_eq!(found.status, AnchorStatus::Exact);
        assert_eq!(found.resolved_range_utf16, Some((10, 18)));
    }

    #[test]
    fn a_passage_edited_in_place_is_modified_not_lost() {
        let (doc, id) = document("paper.md", "The interval covers the mean.");
        let wanted = target(&id, 4, 12, "interval");
        let live = capture(&doc, &wanted).expect("cursors");
        session::put_text(&doc, "paper.md", "The intervals cover the mean.");
        let found = resolve(&doc, &Sources::of(&doc), "b", &anchor(wanted), Some(&live));
        assert_eq!(found.status, AnchorStatus::Modified);
    }

    #[test]
    fn a_passage_that_was_deleted_says_so_rather_than_moving_elsewhere() {
        let (doc, id) = document("paper.md", "The interval covers the mean.");
        let wanted = target(&id, 4, 12, "interval");
        let live = capture(&doc, &wanted).expect("cursors");
        session::put_text(&doc, "paper.md", "The  covers the mean.");
        let found = resolve(&doc, &Sources::of(&doc), "b", &anchor(wanted), Some(&live));
        assert_eq!(found.status, AnchorStatus::Deleted);
    }

    #[test]
    fn a_comment_on_the_whole_document_is_always_attached() {
        let (doc, _) = document("paper.md", "Anything at all.");
        let whole = OriginalAnchor {
            source_sequence: 1,
            frontier: Vec::new(),
            target: CommentTarget::Document,
        };
        assert_eq!(
            resolve(&doc, &Sources::of(&doc), "b", &whole, None).status,
            AnchorStatus::Exact
        );
    }

    #[test]
    fn without_cursors_the_words_are_looked_for() {
        let (doc, id) = document("paper.md", "Well. The interval covers the mean.");
        let wanted = target(&id, 4, 12, "interval");
        let found = resolve(&doc, &Sources::of(&doc), "b", &anchor(wanted), None);
        assert_eq!(found.status, AnchorStatus::Modified);
        assert_eq!(found.resolved_range_utf16, Some((10, 18)));
    }

    #[test]
    fn two_equally_good_candidates_are_ambiguous_rather_than_the_first_one() {
        let (doc, id) = document("paper.md", "interval\ninterval\n");
        let wanted = target(&id, 0, 8, "interval");
        let found = resolve(&doc, &Sources::of(&doc), "b", &anchor(wanted), None);
        assert_eq!(found.status, AnchorStatus::Ambiguous);
    }

    #[test]
    fn a_file_that_is_gone_is_unresolved_and_says_why() {
        let (doc, _) = document("paper.md", "The interval covers the mean.");
        let wanted = target("file-missing", 4, 12, "interval");
        let found = resolve(&doc, &Sources::of(&doc), "b", &anchor(wanted), None);
        assert_eq!(found.status, AnchorStatus::Unresolved);
        assert_eq!(found.diagnostic, Some(ResolutionDiagnostic::RemovedFile));
    }

    #[test]
    fn cursors_from_another_file_are_refused_and_the_words_are_used_instead() {
        let (doc, id) = document("paper.md", "The interval covers the mean.");
        let other = session::put_text(&doc, "notes.md", "The interval covers the mean.");
        let wanted = target(&id, 4, 12, "interval");
        let borrowed = capture(&doc, &target(&other, 4, 12, "interval")).expect("cursors");
        let found = resolve(
            &doc,
            &Sources::of(&doc),
            "b",
            &anchor(wanted),
            Some(&borrowed),
        );
        assert_eq!(
            found.diagnostic,
            Some(ResolutionDiagnostic::ForeignContainer)
        );
        assert_eq!(found.status, AnchorStatus::Exact);
    }
}

#[cfg(test)]
mod cursor_caching_tests {
    use super::tests::*;
    use super::*;

    /// A comment found by its words gets cursors at what was found, so the
    /// next edit resolves it without reading the file again.
    #[test]
    fn a_passage_found_by_its_words_is_given_cursors_for_next_time() {
        let (doc, id) = document("paper.md", "Well. The interval covers the mean.");
        let wanted = target(&id, 4, 12, "interval");
        let found = resolve(&doc, &Sources::of(&doc), "b", &anchor(wanted.clone()), None);
        assert_eq!(found.status, AnchorStatus::Modified);
        let live = found.live_source_range.expect("cursors for next time");

        // And those cursors follow the passage on the next edit, without the
        // words being looked for again.
        session::put_text(
            &doc,
            "paper.md",
            "Well, then. The interval covers the mean.",
        );
        let again = resolve(&doc, &Sources::of(&doc), "c", &anchor(wanted), Some(&live));
        assert_eq!(again.status, AnchorStatus::Exact);
        assert_eq!(again.resolved_range_utf16, Some((16, 24)));
    }
}
