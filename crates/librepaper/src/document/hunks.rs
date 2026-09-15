//! Hunks: the unit a reviewer accepts or declines.
//!
//! A proposal is a branch (SPEC-loro.md §3.3), and review is the text diff
//! between the branch's base and its tip. Loro hands that back as a flat run
//! of retain/insert/delete deltas with no notion of which deltas belong
//! together. A reviewer does not decide about deltas, though: when a branch
//! replaces "cat" with "tabby", the delete and the insert are one decision.
//!
//! A **hunk** is therefore a maximal run of non-retain deltas. That grouping is
//! ours; Loro has no hunk concept.
//!
//! ## Declining is a revert, not a filtered apply
//!
//! `LoroDoc::apply_diff` re-authors everything it applies as the applying peer,
//! so applying a diff filtered to the accepted hunks would produce the right
//! text over the wrong authorship -- the reviewer would appear to have written
//! the author's prose. §3.4 takes the other route: merge the whole branch, then
//! revert only the declined hunks as the reviewer. The author wrote all of it;
//! the reviewer removed part. That is both correct and what actually happened.
//!
//! [`keep_declined`] is the filter step of that sequence. It takes the inverse
//! diff `diff(tip, base)` -- which would undo the entire branch -- and keeps
//! only the hunks the reviewer declined. An accepted hunk collapses to a retain
//! of its tip-side length, because that text stays.
//!
//! ## Offsets here are code points, not UTF-16
//!
//! Every other text path in this crate counts UTF-16 code units, because that
//! is what a browser counts in (§3.2). The deltas here are the one exception,
//! and not by choice: `LoroDoc::diff` indexes by the `wasm` feature of the
//! crate that computed it -- code points off, UTF-16 on -- and offers no
//! `*_utf16` variant to opt out of. A delta computed here and a delta computed
//! in the browser therefore disagree past the first astral character.
//!
//! Nothing in this module converts, because a delta run is self-consistent: the
//! retains and deletes are in the same basis as each other, and applying them
//! back through `apply_diff` on the same side is exact. What must not happen is
//! a delta run crossing the wire and being applied on the other side, or a
//! delta offset being compared against a UTF-16 offset from anywhere else.

// Landed with Phase 0 (SPEC-loro.md §11.1) ahead of its only consumer, the
// accept/decline command path Phase 2 builds. Until that lands, the tests below
// are what this module is for: they hold the attribution property the whole
// proposal model rests on.
#![allow(dead_code)]

use loro::TextDelta;

/// One reviewable decision: a maximal run of non-retain deltas.
#[derive(Debug, Clone, PartialEq)]
pub struct Hunk {
    /// Index of this hunk within its diff, counting from zero. This is the
    /// identity a review decision names, and it is stable only against the
    /// diff it came from -- recompute the diff and the numbering is fresh.
    pub index: usize,
    /// Offset into the *old* side of the diff where this hunk starts, in the
    /// diff's own basis (see the module note: code points on the server).
    /// Useful for presenting a hunk in order; not comparable with a UTF-16
    /// offset taken from anywhere else.
    pub start: usize,
    /// Text this hunk removes from the old side.
    pub deleted: usize,
    /// Text this hunk adds.
    pub inserted: String,
}

/// Split a text delta run into hunks.
pub fn hunks(deltas: &[TextDelta]) -> Vec<Hunk> {
    let mut out = Vec::new();
    let mut cursor = 0usize;
    let mut i = 0usize;
    while i < deltas.len() {
        if let TextDelta::Retain { retain, .. } = &deltas[i] {
            cursor += retain;
            i += 1;
            continue;
        }
        let start = cursor;
        let mut deleted = 0usize;
        let mut inserted = String::new();
        while i < deltas.len() {
            match &deltas[i] {
                TextDelta::Retain { .. } => break,
                TextDelta::Delete { delete } => deleted += delete,
                TextDelta::Insert { insert, .. } => inserted.push_str(insert),
            }
            i += 1;
        }
        cursor += deleted;
        out.push(Hunk {
            index: out.len(),
            start,
            deleted,
            inserted,
        });
    }
    out
}

/// Keep only the declined hunks of an inverse diff, so that applying the result
/// reverts those and leaves the accepted ones standing.
///
/// `deltas` is `diff(tip, base)`: the run that would undo the whole branch.
/// `declined` is asked about each hunk by its index, in order.
pub fn keep_declined(deltas: &[TextDelta], declined: impl Fn(usize) -> bool) -> Vec<TextDelta> {
    let mut out: Vec<TextDelta> = Vec::new();
    let mut i = 0usize;
    let mut hunk = 0usize;
    while i < deltas.len() {
        if let TextDelta::Retain { retain, attributes } = &deltas[i] {
            out.push(TextDelta::Retain {
                retain: *retain,
                attributes: attributes.clone(),
            });
            i += 1;
            continue;
        }
        let start = i;
        while i < deltas.len() && !matches!(deltas[i], TextDelta::Retain { .. }) {
            i += 1;
        }
        let run = &deltas[start..i];
        if declined(hunk) {
            out.extend(run.iter().cloned());
        } else {
            // The hunk is accepted, so the branch's text stays. Retain exactly
            // what this hunk would have removed -- its tip-side length, which
            // on the inverse diff is the sum of its deletes.
            let keep: usize = run
                .iter()
                .map(|d| {
                    if let TextDelta::Delete { delete } = d {
                        *delete
                    } else {
                        0
                    }
                })
                .sum();
            if keep > 0 {
                out.push(TextDelta::Retain {
                    retain: keep,
                    attributes: None,
                });
            }
        }
        hunk += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use loro::{event::Diff, event::DiffBatch, ExportMode, LoroDoc, TextDelta};
    use std::borrow::Cow;

    const AUTHOR: u64 = 2;
    const REVIEWER: u64 = 3;
    const OWNER: u64 = 1;

    fn text_deltas(batch: &DiffBatch) -> Vec<TextDelta> {
        for (_, diff) in batch.iter() {
            if let Diff::Text(deltas) = diff {
                return deltas.clone();
            }
        }
        Vec::new()
    }

    /// A room document with one file, and a branch off it that makes two edits.
    fn room_with_branch() -> (LoroDoc, LoroDoc, loro::Frontiers, loro::Frontiers) {
        let room = LoroDoc::new();
        room.set_peer_id(OWNER).unwrap();
        room.get_text("f")
            .insert_utf16(0, "The cat sat. The dog ran.")
            .unwrap();
        room.commit();
        let base = room.state_frontiers();

        let branch = room.fork();
        branch.set_peer_id(AUTHOR).unwrap();
        {
            let t = branch.get_text("f");
            // Later edit first, so the earlier one's offsets still hold.
            t.delete_utf16(21, 3).unwrap();
            t.insert_utf16(21, "sprinted").unwrap();
            t.delete_utf16(4, 3).unwrap();
            t.insert_utf16(4, "tabby").unwrap();
        }
        branch.commit();
        let tip = branch.state_frontiers();
        (room, branch, base, tip)
    }

    #[test]
    fn a_branch_diff_groups_into_one_hunk_per_decision() {
        let (_room, branch, base, tip) = room_with_branch();
        let found = hunks(&text_deltas(&branch.diff(&base, &tip).unwrap()));

        // Two replacements, so two decisions -- not four.
        assert_eq!(
            found,
            vec![
                Hunk {
                    index: 0,
                    start: 4,
                    deleted: 3,
                    inserted: "tabby".into()
                },
                Hunk {
                    index: 1,
                    start: 21,
                    deleted: 3,
                    inserted: "sprinted".into()
                },
            ],
            "a delete and the insert that replaces it are one hunk"
        );
    }

    #[test]
    fn accepting_one_hunk_keeps_the_other_authors_text_and_attribution() {
        let (room, branch, base, tip) = room_with_branch();

        // Step 1: import the whole branch. Every one of the author's operations,
        // and their authorship, enters the graph.
        let update = branch
            .export(ExportMode::Updates {
                from: Cow::Owned(room.oplog_vv()),
            })
            .unwrap();
        room.import(&update).unwrap();
        assert_eq!(
            room.get_text("f").to_string(),
            "The tabby sat. The dog sprinted."
        );

        // Steps 2 and 3: the inverse diff, filtered to the declined hunk alone.
        let inverse = room.diff(&tip, &base).unwrap();
        let mut filtered = DiffBatch::default();
        for (cid, diff) in inverse.iter() {
            if let Diff::Text(deltas) = diff {
                // Accept hunk 0 ("tabby"), decline hunk 1 ("sprinted").
                let kept = keep_declined(deltas, |h| h == 1);
                filtered.push(cid.clone(), Diff::Text(kept)).unwrap();
            }
        }

        // Step 4: apply it as the reviewer, not the author.
        room.set_peer_id(REVIEWER).unwrap();
        room.apply_diff(filtered).unwrap();
        room.commit();

        assert_eq!(
            room.get_text("f").to_string(),
            "The tabby sat. The dog ran.",
            "the accepted hunk stands and the declined one is reverted"
        );

        // This is contract 4. The author's operations are still in the graph, so
        // the accepted prose is still theirs; the reviewer appears as the peer that
        // removed the rest, rather than as the author of what remains.
        let vv = room.oplog_vv();
        assert!(
            vv.get(&AUTHOR).is_some(),
            "the branch author survives the partial accept"
        );
        assert!(
            vv.get(&REVIEWER).is_some(),
            "the reviewer is recorded as having removed something"
        );
        assert!(
            vv.get(&OWNER).is_some(),
            "the original author of the base text survives"
        );
    }

    #[test]
    fn declining_every_hunk_restores_the_base_exactly() {
        let (room, branch, base, tip) = room_with_branch();
        let update = branch
            .export(ExportMode::Updates {
                from: Cow::Owned(room.oplog_vv()),
            })
            .unwrap();
        room.import(&update).unwrap();

        let inverse = room.diff(&tip, &base).unwrap();
        let mut filtered = DiffBatch::default();
        for (cid, diff) in inverse.iter() {
            if let Diff::Text(deltas) = diff {
                filtered
                    .push(cid.clone(), Diff::Text(keep_declined(deltas, |_| true)))
                    .unwrap();
            }
        }
        room.set_peer_id(REVIEWER).unwrap();
        room.apply_diff(filtered).unwrap();
        room.commit();

        assert_eq!(room.get_text("f").to_string(), "The cat sat. The dog ran.");
    }

    #[test]
    fn accepting_every_hunk_is_the_merge_alone() {
        let (room, branch, base, tip) = room_with_branch();
        let update = branch
            .export(ExportMode::Updates {
                from: Cow::Owned(room.oplog_vv()),
            })
            .unwrap();
        room.import(&update).unwrap();

        let inverse = room.diff(&tip, &base).unwrap();
        for (_, diff) in inverse.iter() {
            if let Diff::Text(deltas) = diff {
                let kept = keep_declined(deltas, |_| false);
                // Accepting everything reverts nothing: what is left is retains.
                assert!(
                    kept.iter().all(|d| matches!(d, TextDelta::Retain { .. })),
                    "accepting every hunk must leave nothing to revert, got {kept:?}"
                );
            }
        }
        assert_eq!(
            room.get_text("f").to_string(),
            "The tabby sat. The dog sprinted."
        );
    }

    #[test]
    fn a_branch_whose_base_has_moved_on_still_merges() {
        let (room, branch, base, tip) = room_with_branch();

        // Main moves while the branch is open, in a part of the text the branch
        // does not touch.
        room.set_peer_id(OWNER).unwrap();
        room.get_text("f").insert_utf16(25, " Again.").unwrap();
        room.commit();

        let update = branch
            .export(ExportMode::Updates {
                from: Cow::Owned(room.oplog_vv()),
            })
            .unwrap();
        room.import(&update).unwrap();
        assert_eq!(
            room.get_text("f").to_string(),
            "The tabby sat. The dog sprinted. Again."
        );

        // The stale base is still a valid left-hand side for the inverse diff.
        let inverse = room.diff(&tip, &base).unwrap();
        let mut filtered = DiffBatch::default();
        for (cid, diff) in inverse.iter() {
            if let Diff::Text(deltas) = diff {
                filtered
                    .push(cid.clone(), Diff::Text(keep_declined(deltas, |h| h == 1)))
                    .unwrap();
            }
        }
        room.set_peer_id(REVIEWER).unwrap();
        room.apply_diff(filtered).unwrap();
        room.commit();

        assert_eq!(
            room.get_text("f").to_string(),
            "The tabby sat. The dog ran. Again.",
            "main's own edit survives a partial accept against a stale base"
        );
    }

    /// §3.2 says every offset counts UTF-16 code units, with no exceptions. `diff`
    /// is the exception it cannot have: the basis is a compile-time feature of
    /// whichever crate computed the delta, so the server says code points and the
    /// browser says UTF-16 for the same edit.
    ///
    /// This test pins the server's answer. If it ever starts failing, Loro has
    /// changed the basis and `document::hunks`' offsets mean something new.
    #[test]
    fn diff_offsets_are_code_points_on_this_side_of_the_wire() {
        let doc = LoroDoc::new();
        doc.get_text("f").insert_utf16(0, "\u{1F600}Z").unwrap();
        doc.commit();
        let base = doc.state_frontiers();

        let branch = doc.fork();
        // Append after the "Z": UTF-16 index 3, code point index 2.
        branch.get_text("f").insert_utf16(3, "!").unwrap();
        branch.commit();

        let deltas = text_deltas(&branch.diff(&base, &branch.state_frontiers()).unwrap());
        let Some(TextDelta::Retain { retain, .. }) = deltas.first() else {
            panic!("expected a leading retain, got {deltas:?}");
        };
        assert_eq!(
            *retain, 2,
            "server-side diff offsets are code points; a browser computes 3 for this same edit, \
			 so a delta run must never cross the wire (see document::hunks)"
        );

        // And the astral character really is the reason the two disagree.
        let t = branch.get_text("f");
        assert_eq!(t.len_utf16(), 4);
        assert_eq!(t.len_unicode(), 3);
    }
}
