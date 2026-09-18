//! Where a comment's passage is now.
//!
//! [`super::locate`] answered "what is this comment about" once, and that
//! answer never changes again. This answers a different question, asked afresh
//! every time the document is served: given the passage a comment was made
//! about, where is that passage in the document as it stands, and is it still
//! the passage it was?
//!
//! The answer is a [`DerivedAttachment`]: a cache, not a record. It is keyed
//! by the checkpoint it was computed against and is thrown away and recomputed
//! whenever the source moves. Nothing it contains is ever written back into
//! the original anchor -- not the offsets it finds, and not the replacement
//! cursors Loro hands back, which belong to the cache for the same reason.
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

use loro::cursor::{Cursor, Side};

use super::annotation::{
    AnchorSide, AnchorStatus, CommentTarget, DerivedAttachment, LiveSourceRange, OriginalAnchor,
    ResolutionDiagnostic, SourceTextTarget,
};
use crate::document::session::{self, CursorResolutionError};

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

/// Works out where a comment's passage is in the document as it stands.
///
/// `live` is whatever the last resolution stored: the cursors it was given, or
/// the replacements Loro handed back. The attachment returned carries the
/// cursors to store for next time, which may be the same ones.
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

pub(crate) fn resolve(
    doc: &loro::LoroDoc,
    sources: &Sources,
    checkpoint: &str,
    anchor: &OriginalAnchor,
    live: Option<&LiveSourceRange>,
) -> DerivedAttachment {
    let target = match &anchor.target {
        // A comment on the document as a whole is about the document as a
        // whole, which is still here.
        CommentTarget::Document => {
            return attached(checkpoint, AnchorStatus::Exact, None, None, None)
        }
        CommentTarget::SourceText(target) => target,
    };
    let Some(units) = sources.units_of(&target.file_id.0) else {
        // The file it names is gone, or is no longer a text. Either way this
        // cannot say where the passage went, which is not the same as saying
        // it was deleted.
        return attached(
            checkpoint,
            AnchorStatus::Unresolved,
            None,
            None,
            Some(ResolutionDiagnostic::RemovedFile),
        );
    };
    if let Some(live) = live {
        match through_cursors(doc, target, live, units) {
            Ok(attachment) => return attachment.with_checkpoint(checkpoint),
            Err(diagnostic) => {
                // The cursors are no help, but the words may still be. A
                // diagnostic is kept either way: "the file was renamed out
                // from under this" and "we had to go looking" are different
                // things to show.
                let mut found = anchored_by_words(doc, target, units, checkpoint);
                found.diagnostic = Some(diagnostic);
                return found;
            }
        }
    }
    anchored_by_words(doc, target, units, checkpoint)
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
    checkpoint: &str,
) -> DerivedAttachment {
    let mut found = by_words(target, units, checkpoint);
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
    // A point is an empty range by construction -- a place between two
    // characters rather than a passage -- so its two cursors resolving onto
    // one another is that place, not a deletion. It is still here; whether it
    // is still between the same words is what its context says.
    if target.start_utf16 == target.end_utf16 {
        let status = if point_context_holds(units, from, target) {
            AnchorStatus::Exact
        } else {
            AnchorStatus::Modified
        };
        return Ok(attached(
            "",
            status,
            Some(replaced(
                live,
                &range.start_replacement,
                &range.end_replacement,
            )),
            Some((from, from)),
            None,
        ));
    }
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

/// Whether the source still reads the same on either side of a point.
///
/// This is to a point what comparing the quoted words is to a passage. A
/// point *is* the place between two words, so those two words are what it was
/// made about and all that is compared -- not the whole stored context, which
/// runs to `CONTEXT` units and would call a point modified because somebody
/// edited a sentence nearby. A passage is not modified by an edit beside it
/// either.
///
/// A point resting against whitespace has no word on that side, and nothing
/// is what it agrees with.
fn point_context_holds(units: &[u16], at: u32, target: &SourceTextTarget) -> bool {
    let at = (at as usize).min(units.len());
    let prefix: Vec<u16> = target.prefix.encode_utf16().collect();
    let suffix: Vec<u16> = target.suffix.encode_utf16().collect();
    let before = last_word(&prefix);
    let after = first_word(&suffix);
    before.len() <= at
        && after.len() <= units.len() - at
        && units[at - before.len()..at] == before[..]
        && units[at..at + after.len()] == after[..]
}

/// UTF-16 whitespace, by the only characters a source file collapses on.
fn is_space16(unit: u16) -> bool {
    matches!(unit, 0x20 | 0x09 | 0x0a | 0x0d | 0x0b | 0x0c)
}

fn last_word(units: &[u16]) -> &[u16] {
    let at = units
        .iter()
        .rposition(|unit| is_space16(*unit))
        .map_or(0, |at| at + 1);
    &units[at..]
}

fn first_word(units: &[u16]) -> &[u16] {
    let end = units
        .iter()
        .position(|unit| is_space16(*unit))
        .unwrap_or(units.len());
    &units[..end]
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
fn by_words(target: &SourceTextTarget, units: &[u16], checkpoint: &str) -> DerivedAttachment {
    if target.exact.is_empty() {
        return point_by_context(target, units, checkpoint);
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
        return attached(checkpoint, AnchorStatus::Deleted, None, None, None);
    };
    let status = if ties > 1 {
        AnchorStatus::Ambiguous
    } else if at as u32 == target.start_utf16 {
        AnchorStatus::Exact
    } else {
        AnchorStatus::Modified
    };
    attached(
        checkpoint,
        status,
        None,
        Some((at as u32, (at + wanted.len()) as u32)),
        None,
    )
}

/// A point has no selected words. When its original cursors are unavailable
/// (as with a note on a published checkpoint), locate the boundary using the
/// source on either side. Equal best matches remain unresolved: choosing an
/// arbitrary occurrence would turn a display guess into a source position.
fn point_by_context(
    target: &SourceTextTarget,
    units: &[u16],
    checkpoint: &str,
) -> DerivedAttachment {
    let prefix: Vec<u16> = target.prefix.encode_utf16().collect();
    let suffix: Vec<u16> = target.suffix.encode_utf16().collect();
    if prefix.is_empty() && suffix.is_empty() {
        return attached(checkpoint, AnchorStatus::Unresolved, None, None, None);
    }
    let mut best: Option<(usize, usize)> = None;
    let mut ties = 0;
    for at in 0..=units.len() {
        let score = common_suffix(&prefix, &units[..at]) + common_prefix(&suffix, &units[at..]);
        match best {
            Some((_, previous)) if score < previous => {}
            Some((_, previous)) if score == previous => ties += 1,
            _ => {
                best = Some((at, score));
                ties = 1;
            }
        }
    }
    let Some((at, score)) = best else {
        return attached(checkpoint, AnchorStatus::Unresolved, None, None, None);
    };
    let exact_context = score == prefix.len() + suffix.len();
    if ties > 1 || (!exact_context && score < 8) {
        return attached(checkpoint, AnchorStatus::Ambiguous, None, None, None);
    }
    let status = if exact_context && at as u32 == target.start_utf16 {
        AnchorStatus::Exact
    } else {
        AnchorStatus::Modified
    };
    attached(checkpoint, status, None, Some((at as u32, at as u32)), None)
}

fn attached(
    checkpoint: &str,
    status: AnchorStatus,
    live_source_range: Option<LiveSourceRange>,
    resolved_range_utf16: Option<(u32, u32)>,
    diagnostic: Option<ResolutionDiagnostic>,
) -> DerivedAttachment {
    DerivedAttachment {
        checkpoint_id: super::annotation::CheckpointId(checkpoint.to_string()),
        status,
        live_source_range,
        resolved_range_utf16,
        diagnostic,
    }
}

impl DerivedAttachment {
    fn with_checkpoint(mut self, checkpoint: &str) -> DerivedAttachment {
        self.checkpoint_id = super::annotation::CheckpointId(checkpoint.to_string());
        self
    }
}

fn slice16(units: &[u16], start: usize, end: usize) -> String {
    if start >= end || start >= units.len() {
        return String::new();
    }
    String::from_utf16_lossy(&units[start..end.min(units.len())])
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

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::document::session;
    use crate::room::annotation::{CheckpointId, FileId};

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
            checkpoint_id: CheckpointId("a".repeat(64)),
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
            checkpoint_id: CheckpointId("a".repeat(64)),
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

    /// A point note is an empty range, and an empty range is exactly what a
    /// deleted passage collapses to. Telling the two apart is the difference
    /// between a point note that works and one that reports itself removed
    /// the moment anybody types.
    #[test]
    fn a_point_is_a_place_and_not_a_deleted_passage() {
        let (doc, id) = document("paper.md", "Before after.");
        let mut wanted = target(&id, 7, 7, "");
        wanted.prefix = "Before ".into();
        wanted.suffix = "after.".into();
        let live = capture(&doc, &wanted).expect("cursors");
        let found = resolve(
            &doc,
            &Sources::of(&doc),
            "b",
            &anchor(wanted.clone()),
            Some(&live),
        );
        assert_eq!(found.status, AnchorStatus::Exact);
        assert_eq!(found.resolved_range_utf16, Some((7, 7)));
    }

    #[test]
    fn a_point_follows_its_cursors_through_an_edit_before_it() {
        let (doc, id) = document("paper.md", "Before after.");
        let mut wanted = target(&id, 7, 7, "");
        wanted.prefix = "Before ".into();
        wanted.suffix = "after.".into();
        let live = capture(&doc, &wanted).expect("cursors");
        session::put_text(&doc, "paper.md", "Added. Before after.");
        let found = resolve(&doc, &Sources::of(&doc), "b", &anchor(wanted), Some(&live));
        assert_eq!(found.status, AnchorStatus::Exact);
        assert_eq!(found.resolved_range_utf16, Some((14, 14)));
    }

    #[test]
    fn a_point_whose_surroundings_were_rewritten_is_modified_not_lost() {
        let (doc, id) = document("paper.md", "Before after.");
        let mut wanted = target(&id, 7, 7, "");
        wanted.prefix = "Before ".into();
        wanted.suffix = "after.".into();
        let live = capture(&doc, &wanted).expect("cursors");
        session::put_text(&doc, "paper.md", "Before then.");
        let found = resolve(&doc, &Sources::of(&doc), "b", &anchor(wanted), Some(&live));
        assert_eq!(found.status, AnchorStatus::Modified);
        assert_eq!(found.resolved_range_utf16, Some((7, 7)));
    }

    #[test]
    fn published_point_without_cursors_finds_its_source_boundary() {
        let (doc, id) = document("paper.md", "Before after.");
        let mut wanted = target(&id, 7, 7, "");
        wanted.prefix = "Before ".into();
        wanted.suffix = "after.".into();
        let found = resolve(&doc, &Sources::of(&doc), "b", &anchor(wanted), None);
        assert_eq!(found.status, AnchorStatus::Exact);
        assert_eq!(found.resolved_range_utf16, Some((7, 7)));
        assert!(found.live_source_range.is_some());
    }

    #[test]
    fn published_point_moves_with_its_context_without_cursors() {
        let (doc, id) = document("paper.md", "Added. Before after.");
        let mut wanted = target(&id, 7, 7, "");
        wanted.prefix = "Before ".into();
        wanted.suffix = "after.".into();
        let found = resolve(&doc, &Sources::of(&doc), "b", &anchor(wanted), None);
        assert_eq!(found.status, AnchorStatus::Modified);
        assert_eq!(found.resolved_range_utf16, Some((14, 14)));
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
