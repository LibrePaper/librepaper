//! Tracked changes: Word's model on the shared Loro document.
//!
//! A tracked edit goes into the text at once, like any edit, and the `changes`
//! root records it as a pending change. A change is one author's edit of one
//! region: the text it inserted (its span, two cursors) and the original text
//! it deleted. Deleted text lives in gaps: a gap is one point of the text, and
//! it holds the deleted texts that sat there as fragments, in document order,
//! each owned by a change. Accepting drops a change's fragments and its record.
//! Rejecting puts its text back where it was, then drops the record. See
//! `SPEC-track-changes.md`.
//!
//! [`plan_edits`] is pure and has a twin in `web/src/lib/track-changes.js`;
//! both run `web/tests/fixtures/track-changes.json`. Everything here counts in
//! Unicode code points, which is what Loro cursors use. The fixtures count in
//! UTF-16 and the test runner converts.
//!
//! None of the writers here commit. The caller owns the transaction, the same
//! as with `apply_edits_at`.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use loro::{
    cursor::{Cursor, PosType, Side},
    Container, ContainerTrait, LoroDoc, LoroMap, LoroText, LoroValue, ValueOrContainer,
};

use crate::document::session::{apply_text_edits, Edit, CHANGES, FILES};

/// Who is making an edit: the stable key and the name comments show.
pub struct Author<'a> {
    pub who: &'a str,
    pub author: &'a str,
}

/// A pending change as the planner sees it. `from == to` for a change with no
/// span; it then sits at the gap of its first fragment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChangeRecord {
    pub id: String,
    pub who: String,
    pub author: String,
    pub run: Option<String>,
    pub at: i64,
    /// Code point offsets of the inserted text, `from <= to`.
    pub from: usize,
    pub to: usize,
    /// A span this edit emptied, the author's own included. It stays in the
    /// plan with `from == to` so that it is kept rather than treated as gone;
    /// it is never written.
    pub emptied: bool,
    /// Its stored cursors resolve to a replaced character. Set by the reader.
    pub stale: bool,
    /// A fragment read as a change of its own, because its change record is
    /// gone. Set by the reader. It has no record of its own to write.
    pub orphan: bool,
}

/// Original text deleted by one change, as it sits in a gap. Each fragment is
/// its own record once stored.
#[derive(Clone, Debug, PartialEq)]
pub struct Fragment {
    /// The record's id. `None` for a fragment the planner made and not yet stored.
    pub id: Option<String>,
    pub change: String,
    pub text: String,
    /// Order among the fragments at one point. `None` until stored.
    pub rank: Option<f64>,
    /// Its cursor resolved to a replaced character. Set by the reader.
    pub stale: bool,
}

/// The deleted texts at one point of the text, in document order.
#[derive(Clone, Debug, PartialEq)]
pub struct Gap {
    pub id: String,
    pub at: usize,
    pub fragments: Vec<Fragment>,
    /// Some fragment's cursor resolved to a replaced character. Set by the reader.
    pub stale: bool,
}

/// The live records of one file, resolved against its text now.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FileState {
    /// Sorted by `(from, to, id)`.
    pub changes: Vec<ChangeRecord>,
    /// Sorted by `at`.
    pub gaps: Vec<Gap>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlanState {
    /// Unchanged apart from shifting; its cursors still hold.
    Kept,
    /// New or changed; its entry must be written.
    Set,
    /// Gone; its entry must be deleted.
    Removed,
}

/// The planned records of one file, in the coordinates of the text after the
/// batch.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Plan {
    pub changes: Vec<(ChangeRecord, PlanState)>,
    pub gaps: Vec<(Gap, PlanState)>,
}

/// A pending change with its span resolved to offsets in the text now, and
/// its deleted text joined from its fragments in document order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedChange {
    pub id: String,
    pub file: String,
    pub who: String,
    pub author: String,
    pub run: Option<String>,
    pub at: i64,
    /// Code point offsets of the inserted text, `from <= to`.
    pub from: usize,
    pub to: usize,
    /// The original text this change removed.
    pub deleted: String,
}

type Row = (ChangeRecord, PlanState);
type GapRow = (Gap, PlanState);

/// Plans a batch of edits against the pending records of one file.
///
/// `edits` are `(from, to, insert)` in the coordinates of `before`, ascending
/// and non-overlapping. They are applied in order, each one adjusted by the
/// edits before it. The result holds every record that was or is now live, in
/// the coordinates of the text after the whole batch, and the records that
/// went away, marked `Removed`.
///
/// With `untracked`, the edits record nothing and change nobody's records
/// except to move, shrink, split and merge them.
///
/// A stored fragment is never rewritten or dropped here, whoever stored it. Its
/// author's other session may be extending it, or a reviewer may be deciding
/// its change, at this very moment, and either write would be lost. So adjacent
/// pieces join into one fragment only while neither is stored, and a span that
/// an edit empties stays as an empty record for prune to settle.
#[allow(clippy::too_many_arguments)]
pub fn plan_edits(
    before: &str,
    state: &FileState,
    edits: &[(usize, usize, String)],
    me: &Author,
    run: Option<&str>,
    now_ms: i64,
    untracked: bool,
    mint: &mut dyn FnMut() -> String,
) -> Plan {
    let mut cur: Vec<char> = before.chars().collect();
    let mut changes: Vec<Row> = state
        .changes
        .iter()
        .map(|c| (c.clone(), PlanState::Kept))
        .collect();
    let mut gaps: Vec<GapRow> = state
        .gaps
        .iter()
        .map(|g| (g.clone(), PlanState::Kept))
        .collect();
    let original_changes: HashSet<String> = state.changes.iter().map(|c| c.id.clone()).collect();
    let original_gaps: HashSet<String> = state.gaps.iter().map(|g| g.id.clone()).collect();
    let mut removed_changes: BTreeMap<String, Row> = BTreeMap::new();
    let mut removed_gaps: BTreeMap<String, GapRow> = BTreeMap::new();
    let mut adj: isize = 0;

    for (from, to, insert) in edits {
        let ef = (*from as isize + adj) as usize;
        let et = (*to as isize + adj) as usize;
        let ins: Vec<char> = insert.chars().collect();
        if ef == et && ins.is_empty() {
            continue;
        }
        let (next_changes, next_gaps) = plan_one(
            &cur,
            changes.clone(),
            gaps.clone(),
            ef,
            et,
            &ins,
            me,
            run,
            now_ms,
            untracked,
            mint,
        );
        for (c, _) in &changes {
            if original_changes.contains(&c.id) && !next_changes.iter().any(|(n, _)| n.id == c.id) {
                removed_changes.insert(c.id.clone(), (c.clone(), PlanState::Removed));
            }
        }
        for (g, _) in &gaps {
            if original_gaps.contains(&g.id) && !next_gaps.iter().any(|(n, _)| n.id == g.id) {
                removed_gaps.insert(g.id.clone(), (g.clone(), PlanState::Removed));
            }
        }
        changes = next_changes;
        gaps = next_gaps;
        cur.splice(ef..et, ins.iter().copied());
        adj += ins.len() as isize - (et - ef) as isize;
    }

    for (c, st) in changes.iter_mut() {
        if !original_changes.contains(&c.id) && *st == PlanState::Kept {
            *st = PlanState::Set;
        }
    }
    for (g, st) in gaps.iter_mut() {
        if !original_gaps.contains(&g.id) && *st == PlanState::Kept {
            *st = PlanState::Set;
        }
    }
    let mut out_changes = changes;
    out_changes.extend(removed_changes.into_values());
    let mut out_gaps = gaps;
    out_gaps.extend(removed_gaps.into_values());
    Plan {
        changes: out_changes,
        gaps: out_gaps,
    }
}

fn has_span(c: &ChangeRecord) -> bool {
    c.to > c.from
}

/// The end of the span that holds `p`, if any change inserted the character
/// at `p`.
fn span_end_at(changes: &[Row], p: usize) -> Option<usize> {
    changes
        .iter()
        .find(|(c, _)| has_span(c) && c.from <= p && p < c.to)
        .map(|(c, _)| c.to)
}

/// Appends `text` owned by `change`, joining it to the last fragment only when
/// neither has a stored id. A stored fragment keeps its record and its rank.
fn push_fragment(
    list: &mut Vec<Fragment>,
    change: &str,
    text: &str,
    id: Option<&str>,
    rank: Option<f64>,
) {
    match list.last_mut() {
        Some(last) if last.change == change && last.id.is_none() && id.is_none() => {
            last.text.push_str(text);
        }
        _ => list.push(Fragment {
            id: id.map(str::to_string),
            change: change.to_string(),
            text: text.to_string(),
            rank,
            stale: false,
        }),
    }
}

fn push_unique(ids: &mut Vec<String>, id: &str) {
    if !ids.iter().any(|x| x == id) {
        ids.push(id.to_string());
    }
}

/// The next gap id: a swallowed gap's id is reused first, then a fresh one.
fn take_id(ids: &mut Vec<String>, mint: &mut dyn FnMut() -> String) -> String {
    if ids.is_empty() {
        mint()
    } else {
        ids.remove(0)
    }
}

/// Plans one edit `[ef, et)` with `insert`, in the coordinates of `cur`.
#[allow(clippy::too_many_arguments)]
fn plan_one(
    cur: &[char],
    changes: Vec<Row>,
    gaps: Vec<GapRow>,
    ef: usize,
    et: usize,
    insert: &[char],
    me: &Author,
    run: Option<&str>,
    now: i64,
    untracked: bool,
    mint: &mut dyn FnMut() -> String,
) -> (Vec<Row>, Vec<GapRow>) {
    let len = insert.len();
    let delta = len as isize - (et - ef) as isize;
    let shift = |p: usize| (p as isize + delta) as usize;

    let swallowed: Vec<Gap> = gaps
        .iter()
        .filter(|(g, _)| g.at >= ef && g.at <= et)
        .map(|(g, _)| g.clone())
        .collect();

    // Own changes the edit touches. A span of mine joins when it touches
    // [ef, et]. A change of mine with no span joins when one of its fragments
    // sits in a swallowed gap. A span of mine elsewhere stays its own change:
    // one change holds one span, and a span never widens over text that is
    // not its own.
    // Spans never merge: a change holds one span, and a record is never
    // emptied for another record's sake. Only the first own span touching the
    // edit takes it; another own span there is handled like any other span, so
    // it shrinks, splits or moves and keeps its record and its text.
    let mut owned: Vec<String> = Vec::new();
    let first_span = changes
        .iter()
        .filter(|(c, _)| c.who == me.who && has_span(c) && c.from <= et && c.to >= ef)
        .map(|(c, _)| c)
        .min_by(|a, b| a.from.cmp(&b.from).then_with(|| a.id.cmp(&b.id)));
    if let Some(c) = first_span {
        push_unique(&mut owned, &c.id);
    }
    for g in &swallowed {
        for f in &g.fragments {
            if let Some((c, _)) = changes.iter().find(|(c, _)| c.id == f.change) {
                // A fragment read as a change of its own is not joined: it
                // has no record to merge into, and it never leaves its gap.
                if c.who == me.who && !has_span(c) && !c.orphan {
                    push_unique(&mut owned, &c.id);
                }
            }
        }
    }

    // Original text inside [ef, et) that the edit removes makes a record; so
    // does inserted text. A deletion inside insertions only shrinks them.
    // An untracked edit records nothing and makes no change of mine: spans
    // still shrink, move and split, and swallowed gaps still merge at `ef`.
    let original = (ef..et).any(|p| span_end_at(&changes, p).is_none());
    let records = !untracked && (original || len > 0);
    if untracked {
        owned.clear();
    }

    // The change this edit writes: the first owned one by position, or a new
    // one. Its span is widened over the owned spans below.
    let mut mine: Option<ChangeRecord> = None;
    if records {
        let first = owned
            .iter()
            .filter_map(|id| changes.iter().find(|(c, _)| &c.id == id).map(|(c, _)| c))
            .min_by_key(|c| c.from);
        mine = Some(match first {
            Some(c) => ChangeRecord {
                run: run.map(str::to_string).or_else(|| c.run.clone()),
                at: now,
                emptied: false,
                orphan: false,
                ..c.clone()
            },
            None => ChangeRecord {
                id: mint(),
                who: me.who.to_string(),
                author: me.author.to_string(),
                run: run.map(str::to_string),
                at: now,
                from: ef,
                to: ef,
                emptied: false,
                stale: false,
                orphan: false,
            },
        });
    }
    if let Some(m) = mine.as_mut() {
        let mut from = ef;
        let mut to = ef + len;
        for id in &owned {
            let Some((c, _)) = changes.iter().find(|(c, _)| &c.id == id) else {
                continue;
            };
            if !has_span(c) {
                continue;
            }
            from = from.min(c.from);
            if c.to > et {
                to = to.max(shift(c.to));
            }
        }
        m.from = from;
        m.to = to;
    }

    // Merged fragments: walking [ef, et], the gap at p comes first, then the
    // original character at p. Text that sat at et goes after the new span
    // when text is inserted and no own change claims that point.
    let own_at_end = changes
        .iter()
        .any(|(c, _)| c.who == me.who && has_span(c) && c.from == et && owned.contains(&c.id))
        || swallowed
            .iter()
            .any(|g| g.at == et && g.fragments.iter().any(|f| owned.contains(&f.change)));
    let split_at_end = len > 0 && !own_at_end;
    let mut merged: Vec<Fragment> = Vec::new();
    let mut after: Vec<Fragment> = Vec::new();
    for p in ef..=et {
        // Untracked edits can leave two gaps on one point; every one is taken.
        for g in swallowed.iter().filter(|g| g.at == p) {
            for f in &g.fragments {
                let change: &str = match &mine {
                    Some(m) if owned.contains(&f.change) => m.id.as_str(),
                    _ => f.change.as_str(),
                };
                let target = if split_at_end && p == et {
                    &mut after
                } else {
                    &mut merged
                };
                push_fragment(target, change, &f.text, f.id.as_deref(), f.rank);
            }
        }
        if let (Some(m), Some(ch)) = (&mine, cur.get(p)) {
            if p < et && span_end_at(&changes, p).is_none() {
                push_fragment(&mut merged, &m.id, &ch.to_string(), None, None);
            }
        }
    }

    // Spans: foreign spans shrink, move or split around the edit.
    let mut out: Vec<Row> = Vec::new();
    let mut new_spans: Vec<Row> = Vec::new();
    for (c, st) in &changes {
        if let Some(m) = &mine {
            if owned.contains(&c.id) {
                // Mine, or an own deletion whose fragments now carry the merged
                // id. The latter's record stays, untouched and empty: another
                // session of the author may be adding to it at this moment, and
                // prune drops it once it has rested empty.
                if c.id != m.id {
                    out.push((
                        ChangeRecord {
                            from: ef + len,
                            to: ef + len,
                            emptied: true,
                            ..c.clone()
                        },
                        PlanState::Kept,
                    ));
                }
                continue;
            }
        }
        if !has_span(c) {
            out.push((c.clone(), *st));
            continue;
        }
        // A span whose boundary sits exactly at an insertion point is written
        // again: a cursor left on a deleted character there could drift across
        // the new text, and the write re-pins it only if it did.
        if c.to <= ef {
            let state = if len > 0 && c.to == ef {
                PlanState::Set
            } else {
                *st
            };
            out.push((c.clone(), state));
            continue;
        }
        if c.from >= et {
            let state = if len > 0 && c.from == et {
                PlanState::Set
            } else {
                *st
            };
            out.push((
                ChangeRecord {
                    from: shift(c.from),
                    to: shift(c.to),
                    ..c.clone()
                },
                state,
            ));
            continue;
        }
        if c.from < ef && c.to > et && len > 0 {
            out.push((
                ChangeRecord {
                    to: ef,
                    ..c.clone()
                },
                PlanState::Set,
            ));
            let id = mint();
            new_spans.push((
                ChangeRecord {
                    id,
                    from: ef + len,
                    to: shift(c.to),
                    ..c.clone()
                },
                PlanState::Set,
            ));
            continue;
        }
        let from = if c.from < ef { c.from } else { ef + len };
        let to = if c.to > et {
            shift(c.to)
        } else if c.from < ef {
            ef
        } else {
            ef + len
        };
        if to > from {
            out.push((
                ChangeRecord {
                    from,
                    to,
                    ..c.clone()
                },
                PlanState::Set,
            ));
        } else {
            // The whole span went from this view. The record stays, untouched:
            // its author may be extending it at this moment, and a stale cursor
            // resolves to the right place anyway. It is kept, never written. Prune
            // marks it, and drops it once it has rested empty for a while.
            out.push((
                ChangeRecord {
                    from: ef + len,
                    to: ef + len,
                    emptied: true,
                    ..c.clone()
                },
                PlanState::Kept,
            ));
        }
    }
    out.extend(new_spans);
    if let Some(m) = &mine {
        out.push((m.clone(), PlanState::Set));
    }

    // Gaps: those before the edit stay, those after shift; the swallowed ones
    // merge into one gap at the merged span's start (or at ef without a span).
    let mut out_gaps: Vec<GapRow> = Vec::new();
    for (g, st) in &gaps {
        if g.at < ef {
            out_gaps.push((g.clone(), *st));
        } else if g.at > et {
            out_gaps.push((
                Gap {
                    at: shift(g.at),
                    ..g.clone()
                },
                *st,
            ));
        }
    }
    let mut ids: Vec<String> = swallowed.iter().map(|g| g.id.clone()).collect();
    let pos = mine.as_ref().map_or(ef, |m| m.from);
    if !merged.is_empty() {
        // One gap per position: every gap already at the span's start, which
        // held text replaced by its earlier part, takes the new fragments
        // after its own, in order.
        let there: Vec<GapRow> = out_gaps
            .iter()
            .filter(|(g, _)| g.at == pos)
            .cloned()
            .collect();
        if let Some((first, _)) = there.first() {
            let mut list: Vec<Fragment> = Vec::new();
            for (g, _) in &there {
                for f in &g.fragments {
                    push_fragment(&mut list, &f.change, &f.text, f.id.as_deref(), f.rank);
                }
            }
            for f in &merged {
                push_fragment(&mut list, &f.change, &f.text, f.id.as_deref(), f.rank);
            }
            out_gaps.retain(|(g, _)| g.at != pos);
            out_gaps.push((
                Gap {
                    fragments: list,
                    ..first.clone()
                },
                PlanState::Set,
            ));
        } else {
            let id = take_id(&mut ids, mint);
            out_gaps.push((
                Gap {
                    id,
                    at: pos,
                    fragments: merged,
                    stale: false,
                },
                PlanState::Set,
            ));
        }
    }
    if !after.is_empty() {
        let id = take_id(&mut ids, mint);
        out_gaps.push((
            Gap {
                id,
                at: ef + len,
                fragments: after,
                stale: false,
            },
            PlanState::Set,
        ));
    }

    // The changes that merged into mine are gone; their fragments anywhere
    // else now belong to mine.
    if let Some(m) = &mine {
        for (g, st) in out_gaps.iter_mut() {
            if !g
                .fragments
                .iter()
                .any(|f| owned.contains(&f.change) && f.change != m.id)
            {
                continue;
            }
            let mut list: Vec<Fragment> = Vec::new();
            for f in &g.fragments {
                let change = if owned.contains(&f.change) {
                    m.id.as_str()
                } else {
                    f.change.as_str()
                };
                push_fragment(&mut list, change, &f.text, f.id.as_deref(), f.rank);
            }
            g.fragments = list;
            *st = PlanState::Set;
        }
    }

    // A change with no span and no fragment is gone. A pure deletion sits at
    // the gap of its first fragment.
    let referenced: HashSet<String> = out_gaps
        .iter()
        .flat_map(|(g, _)| g.fragments.iter().map(|f| f.change.clone()))
        .collect();
    // A change whose span went but whose deletions remain is a deletion now,
    // not an empty record.
    for (c, _) in out.iter_mut() {
        if c.emptied && referenced.contains(&c.id) {
            c.emptied = false;
        }
    }
    let mut live: Vec<Row> = out
        .into_iter()
        .filter(|(c, _)| has_span(c) || referenced.contains(&c.id) || c.emptied)
        .collect();
    live.sort_by_key(|(c, _)| (c.from, c.to));
    out_gaps.sort_by_key(|(g, _)| g.at);
    for (c, _) in live.iter_mut() {
        if has_span(c) {
            continue;
        }
        if let Some((g, _)) = out_gaps
            .iter()
            .find(|(g, _)| g.fragments.iter().any(|f| f.change == c.id))
        {
            if c.from != g.at {
                c.from = g.at;
                c.to = g.at;
            }
        }
    }
    (live, out_gaps)
}

/// The record `id` of the `changes` root, when it is a map container.
fn record_map(root: &LoroMap, id: &str) -> Option<LoroMap> {
    match root.get(id)? {
        ValueOrContainer::Container(Container::Map(m)) => Some(m),
        _ => None,
    }
}

/// Every map record of the `changes` root. Plain values are left out.
fn records(doc: &LoroDoc) -> Vec<(String, LoroMap)> {
    let root = doc.get_map(CHANGES);
    let ids: Vec<String> = root.keys().map(|k| k.to_string()).collect();
    ids.into_iter()
        .filter_map(|id| record_map(&root, &id).map(|m| (id, m)))
        .collect()
}

fn str_of(m: &LoroMap, key: &str) -> Option<String> {
    match m.get(key)? {
        ValueOrContainer::Value(LoroValue::String(s)) => Some(s.to_string()),
        _ => None,
    }
}

fn bytes_of(m: &LoroMap, key: &str) -> Option<Vec<u8>> {
    match m.get(key)? {
        ValueOrContainer::Value(LoroValue::Binary(b)) => Some(b.to_vec()),
        _ => None,
    }
}

/// A number field. The browser writes JS numbers, which Loro stores as doubles.
fn num_of(m: &LoroMap, key: &str) -> Option<f64> {
    match m.get(key)? {
        ValueOrContainer::Value(LoroValue::Double(v)) => Some(v),
        ValueOrContainer::Value(LoroValue::I64(v)) => Some(v as f64),
        _ => None,
    }
}

/// Sets `key` to `value` unless it already holds it.
fn put(m: &LoroMap, key: &str, value: LoroValue) {
    let same = match m.get(key) {
        Some(ValueOrContainer::Value(v)) => v == value,
        _ => false,
    };
    if !same {
        m.insert(key, value).ok();
    }
}

fn run_value(run: Option<&str>) -> LoroValue {
    run.map_or(LoroValue::Null, LoroValue::from)
}

/// A fragment's rank. A fragment the planner made has none yet and reads as 0.
fn rank_of(f: &Fragment) -> f64 {
    f.rank.unwrap_or(0.0)
}

/// A cursor resolved to its offset in `text`. `stale` when Loro answered with
/// a replacement because its character was deleted.
struct Pos {
    offset: usize,
    stale: bool,
}

fn cursor_pos(doc: &LoroDoc, text: &LoroText, bytes: &[u8]) -> Option<Pos> {
    let cursor = Cursor::decode(bytes).ok()?;
    if cursor.container != text.id() {
        return None;
    }
    let found = doc.get_cursor_pos(&cursor).ok()?;
    Some(Pos {
        offset: found.current.pos,
        stale: found.update.is_some(),
    })
}

/// Whether two encoded cursors are attached to the same character (or the same
/// end) on the same side. Compared by what they are attached to, because one
/// position can encode in more than one way.
fn same_cursor(a: &[u8], b: &[u8]) -> bool {
    match (Cursor::decode(a), Cursor::decode(b)) {
        (Ok(x), Ok(y)) => x.id == y.id && x.side == y.side && x.container == y.container,
        _ => false,
    }
}

/// The span a change record's stored cursors resolve to now, or `None` when the
/// record holds no span.
fn resolved_span(doc: &LoroDoc, text: &LoroText, m: &LoroMap) -> Option<(usize, usize)> {
    let s = cursor_pos(doc, text, &bytes_of(m, "start")?)?;
    let e = cursor_pos(doc, text, &bytes_of(m, "end")?)?;
    // A live end sits on the last inserted character; a replaced one is the
    // exclusive end already.
    let to = if e.stale { e.offset } else { e.offset + 1 };
    Some((s.offset, to.max(s.offset)))
}

/// The author fields a fragment record carries, so that `resolve_file` can read
/// a fragment whose change record is gone as a change of its own.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Authorship {
    id: String,
    who: Option<String>,
    author: Option<String>,
    run: Option<String>,
    at: Option<i64>,
}

/// A fragment record as read, before its gap is known.
struct Stored {
    point: usize,
    fragment: Fragment,
    authorship: Authorship,
}

/// What `resolve_file` reads: the live state.
#[derive(Default)]
struct Resolved {
    state: FileState,
}

/// Reads a fragment record. Left out when its cursor, text or change is missing.
fn read_fragment(doc: &LoroDoc, text: &LoroText, id: &str, m: &LoroMap) -> Option<Stored> {
    let start = bytes_of(m, "start")?;
    let change = str_of(m, "change")?;
    let body = str_of(m, "text")?;
    if body.is_empty() {
        return None;
    }
    let pos = cursor_pos(doc, text, &start)?;
    Some(Stored {
        point: pos.offset,
        fragment: Fragment {
            id: Some(id.to_string()),
            change,
            text: body,
            rank: Some(num_of(m, "rank").unwrap_or(0.0)),
            stale: pos.stale,
        },
        authorship: Authorship {
            id: id.to_string(),
            who: str_of(m, "who"),
            author: str_of(m, "author"),
            run: str_of(m, "run"),
            at: num_of(m, "at").map(|v| v as i64),
        },
    })
}

/// Reads a change record. Left out when its author fields or cursors are missing.
fn read_change(doc: &LoroDoc, text: &LoroText, id: &str, m: &LoroMap) -> Option<ChangeRecord> {
    let who = str_of(m, "who")?;
    let author = str_of(m, "author")?;
    let mut from = 0;
    let mut to = 0;
    let mut stale = false;
    if let Some(start) = bytes_of(m, "start") {
        let s = cursor_pos(doc, text, &start)?;
        from = s.offset;
        to = from;
        stale = s.stale;
        if let Some(end) = bytes_of(m, "end") {
            let e = cursor_pos(doc, text, &end)?;
            if e.stale {
                stale = true;
            }
            // A live end sits on the last inserted character; a replaced one
            // is the exclusive end already.
            to = if e.stale { e.offset } else { e.offset + 1 };
            to = to.max(from);
        }
    }
    Some(ChangeRecord {
        id: id.to_string(),
        who,
        author,
        run: str_of(m, "run"),
        at: num_of(m, "at").map_or(0, |v| v as i64),
        from,
        to,
        emptied: false,
        stale,
        orphan: false,
    })
}

/// Resolves the records of one file against its text now. Gaps are derived:
/// the fragments that resolve to one offset, in rank order. A change with no
/// span takes its place from its first fragment, and one with neither span nor
/// fragment is left out. A fragment whose change record is gone is read as a
/// change of its own, under its own id, when it has author fields; without
/// them it is left out. Records without a known `kind` are ignored.
fn resolve_file(doc: &LoroDoc, file_id: &str) -> Resolved {
    let Some(text) = text_of(doc, file_id) else {
        return Resolved::default();
    };
    let mut changes: Vec<ChangeRecord> = Vec::new();
    let mut stored: Vec<Stored> = Vec::new();
    // Records of the earlier model are read in place, as the changes and
    // fragments they stand for. Nothing is written until one of them is.
    for (c, fragments) in expand_legacy(&legacy_records(doc, file_id)) {
        for (fragment, anchor) in fragments {
            let Some(pos) = cursor_pos(doc, &text, &anchor) else {
                continue;
            };
            let id = fragment.id.clone().unwrap_or_default();
            stored.push(Stored {
                point: pos.offset,
                fragment: Fragment {
                    stale: pos.stale,
                    ..fragment
                },
                authorship: Authorship {
                    id,
                    who: None,
                    author: None,
                    run: None,
                    at: None,
                },
            });
        }
        let (mut from, mut to, mut stale) = (0, 0, false);
        if let Some(end) = &c.end {
            let (Some(s), Some(e)) = (
                cursor_pos(doc, &text, &c.start),
                cursor_pos(doc, &text, end),
            ) else {
                continue;
            };
            from = s.offset;
            stale = s.stale || e.stale;
            let exclusive = if e.stale { e.offset } else { e.offset + 1 };
            to = exclusive.max(from);
        }
        changes.push(ChangeRecord {
            id: c.id,
            who: c.who,
            author: c.author,
            run: c.run,
            at: c.at,
            from,
            to,
            emptied: false,
            stale,
            orphan: false,
        });
    }
    for (id, m) in records(doc) {
        if str_of(&m, "file").as_deref() != Some(file_id) {
            continue;
        }
        match str_of(&m, "kind").as_deref() {
            Some("fragment") => stored.extend(read_fragment(doc, &text, &id, &m)),
            Some("change") => changes.extend(read_change(doc, &text, &id, &m)),
            _ => {}
        }
    }
    // A fragment whose change record is gone was written while someone accepted
    // that change. The deletion it records is still pending, so it is read as a
    // change of its own, under the fragment's id, and decided like any other.
    // Nothing is written for it: every reader derives the same row.
    let mut known: HashSet<String> = changes.iter().map(|c| c.id.clone()).collect();
    for s in stored.iter_mut() {
        let Some(who) = s.authorship.who.clone() else {
            continue;
        };
        if known.contains(&s.fragment.change) {
            continue;
        }
        let id = s.authorship.id.clone();
        s.fragment.change = id.clone();
        known.insert(id.clone());
        changes.push(ChangeRecord {
            id,
            author: s.authorship.author.clone().unwrap_or_else(|| who.clone()),
            who,
            run: s.authorship.run.clone(),
            at: s.authorship.at.unwrap_or(0),
            from: s.point,
            to: s.point,
            emptied: false,
            stale: s.fragment.stale,
            orphan: true,
        });
    }
    let mut live: Vec<Stored> = stored
        .into_iter()
        .filter(|s| known.contains(&s.fragment.change))
        .collect();
    // A fragment never sits inside a span: one anchored strictly inside a span
    // is placed at the span's start, since its text preceded the span.
    for s in live.iter_mut() {
        if let Some(c) = changes.iter().find(|c| c.from < s.point && s.point < c.to) {
            s.point = c.from;
        }
    }
    live.sort_by(|a, b| {
        a.point
            .cmp(&b.point)
            .then_with(|| rank_of(&a.fragment).total_cmp(&rank_of(&b.fragment)))
            .then_with(|| a.fragment.id.cmp(&b.fragment.id))
    });
    let mut gaps: Vec<Gap> = Vec::new();
    for Stored {
        point, fragment, ..
    } in live
    {
        let stale = fragment.stale;
        match gaps.last_mut() {
            Some(g) if g.at == point => {
                g.stale |= stale;
                g.fragments.push(fragment);
            }
            _ => gaps.push(Gap {
                id: format!("gap@{point}"),
                at: point,
                fragments: vec![fragment],
                stale,
            }),
        }
    }
    let mut out: Vec<ChangeRecord> = Vec::new();
    for mut c in changes {
        if c.to == c.from {
            let Some(at) = gaps
                .iter()
                .find(|g| g.fragments.iter().any(|f| f.change == c.id))
                .map(|g| g.at)
            else {
                continue;
            };
            c.from = at;
            c.to = at;
        }
        out.push(c);
    }
    out.sort_by(|a, b| (a.from, a.to, &a.id).cmp(&(b.from, b.to, &b.id)));
    Resolved {
        state: FileState { changes: out, gaps },
    }
}

/// A record of the earlier model, read in place. It is a plain value, not a
/// container. It stands for a change with the deleted text it owns, and for the
/// pure deletions it swallowed, which it names as points.
#[derive(Clone, Debug, PartialEq)]
struct LegacyRecord {
    deleted: String,
    start: Vec<u8>,
    end: Option<Vec<u8>>,
    who: String,
    author: String,
    run: Option<String>,
    at: i64,
    /// The record ids of the pure deletions swallowed here, each with its code
    /// point offset in `deleted`.
    points: Vec<(String, usize)>,
}

/// A legacy record as the change it stands for. Its fragments are listed apart,
/// each with the start cursor it is anchored on.
#[derive(Clone, Debug, PartialEq)]
struct LegacyChange {
    id: String,
    who: String,
    author: String,
    run: Option<String>,
    at: i64,
    start: Vec<u8>,
    end: Option<Vec<u8>>,
}

fn plain_str(v: &LoroValue) -> Option<String> {
    match v {
        LoroValue::String(s) => Some(s.to_string()),
        _ => None,
    }
}

fn plain_bytes(v: &LoroValue) -> Option<Vec<u8>> {
    match v {
        LoroValue::Binary(b) => Some(b.to_vec()),
        _ => None,
    }
}

fn plain_num(v: &LoroValue) -> Option<f64> {
    match v {
        LoroValue::Double(x) => Some(*x),
        LoroValue::I64(x) => Some(*x as f64),
        _ => None,
    }
}

/// The code point index of a UTF-16 offset in `s`. An offset inside a surrogate
/// pair names the character after it.
fn utf16_to_code_point(s: &str, units: usize) -> usize {
    let mut seen = 0;
    for (i, ch) in s.chars().enumerate() {
        if seen >= units {
            return i;
        }
        seen += ch.len_utf16();
    }
    s.chars().count()
}

/// The plain-value records of the earlier model that belong to `file_id`, by id.
/// Their point offsets are converted from UTF-16 to code points.
fn legacy_records(doc: &LoroDoc, file_id: &str) -> BTreeMap<String, LegacyRecord> {
    let root = doc.get_map(CHANGES);
    let ids: Vec<String> = root.keys().map(|k| k.to_string()).collect();
    let mut out = BTreeMap::new();
    for id in ids {
        let Some(ValueOrContainer::Value(LoroValue::Map(m))) = root.get(&id) else {
            continue;
        };
        if m.get("file").and_then(plain_str).as_deref() != Some(file_id) {
            continue;
        }
        let Some(deleted) = m.get("deleted").and_then(plain_str) else {
            continue;
        };
        let Some(start) = m.get("start").and_then(plain_bytes) else {
            continue;
        };
        let points: Vec<(String, usize)> = match m.get("points") {
            Some(LoroValue::List(list)) => list
                .iter()
                .filter_map(|p| match p {
                    LoroValue::Map(p) => {
                        let units = p.get("at").and_then(plain_num)?;
                        let at = utf16_to_code_point(&deleted, units.max(0.0) as usize);
                        Some((p.get("id").and_then(plain_str)?, at))
                    }
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        };
        out.insert(
            id,
            LegacyRecord {
                end: m.get("end").and_then(plain_bytes),
                who: m.get("who").and_then(plain_str).unwrap_or_default(),
                author: m.get("author").and_then(plain_str).unwrap_or_default(),
                run: m.get("run").and_then(plain_str),
                at: m.get("at").and_then(plain_num).map_or(0, |v| v as i64),
                deleted,
                start,
                points,
            },
        );
    }
    out
}

/// The deleted text of legacy record `id` as pieces, each with the id of the
/// change it belongs to. The text is split at each point, and the pointed
/// record's pieces go in between. A point that another point of the same owner
/// also lists is expanded inside that inner owner only.
fn legacy_pieces(
    records: &BTreeMap<String, LegacyRecord>,
    id: &str,
    visited: &mut HashSet<String>,
) -> Vec<(String, String)> {
    let Some(v) = records.get(id) else {
        return Vec::new();
    };
    if !visited.insert(id.to_string()) {
        return Vec::new();
    }
    let known: Vec<&(String, usize)> = v
        .points
        .iter()
        .filter(|(p, _)| records.contains_key(p.as_str()))
        .collect();
    let mut inner: HashSet<&str> = HashSet::new();
    for (p, _) in &known {
        if let Some(owner) = records.get(p.as_str()) {
            inner.extend(owner.points.iter().map(|(q, _)| q.as_str()));
        }
    }
    let mut kept: Vec<&(String, usize)> = known
        .into_iter()
        .filter(|(p, _)| !inner.contains(p.as_str()))
        .collect();
    kept.sort_by_key(|point| point.1);
    let chars: Vec<char> = v.deleted.chars().collect();
    let mut pieces: Vec<(String, String)> = Vec::new();
    let mut cursor = 0;
    for (p, at) in kept {
        if *at > cursor {
            pieces.push((
                id.to_string(),
                chars[cursor..*at].iter().collect::<String>(),
            ));
        }
        pieces.extend(legacy_pieces(records, p, visited));
        cursor = cursor.max(*at);
    }
    if cursor < chars.len() {
        pieces.push((id.to_string(), chars[cursor..].iter().collect::<String>()));
    }
    pieces
}

/// A legacy record expanded: its change, and its fragments each paired with
/// the anchor bytes they inherit from it.
type LegacyExpansion = (LegacyChange, Vec<(Fragment, Vec<u8>)>);

/// Each legacy record as the change it stands for, with the fragments it owns.
/// Fragment ids are `<record id>:<index>`, the rank is the index, and each
/// fragment is anchored on the start cursor of the record that holds it. A
/// record that another one points at has no fragments of its own.
fn expand_legacy(records: &BTreeMap<String, LegacyRecord>) -> Vec<LegacyExpansion> {
    let pointed: HashSet<&str> = records
        .values()
        .flat_map(|v| v.points.iter().map(|(p, _)| p.as_str()))
        .filter(|p| records.contains_key(*p))
        .collect();
    let mut visited: HashSet<String> = HashSet::new();
    let mut out: Vec<LegacyExpansion> = Vec::new();
    for (id, v) in records {
        let change = LegacyChange {
            id: id.clone(),
            who: v.who.clone(),
            author: v.author.clone(),
            run: v.run.clone(),
            at: v.at,
            start: v.start.clone(),
            end: v.end.clone(),
        };
        if pointed.contains(id.as_str()) {
            out.push((change, Vec::new()));
            continue;
        }
        let fragments: Vec<(Fragment, Vec<u8>)> = legacy_pieces(records, id, &mut visited)
            .into_iter()
            .enumerate()
            .map(|(i, (owner, text))| {
                let fragment = Fragment {
                    id: Some(format!("{id}:{i}")),
                    change: owner,
                    text,
                    rank: Some(i as f64),
                    stale: false,
                };
                (fragment, v.start.clone())
            })
            .collect();
        out.push((change, fragments));
    }
    out
}

/// Rewrites legacy record `id` as containers. The group is the outer record
/// that holds its fragments and the records those fragments name: each change
/// record goes under its own id, and the outer record's fragments under their
/// derived ids. The plain records are deleted. Returns false when `id` is not a
/// legacy record of `file_id`.
fn convert_legacy(doc: &LoroDoc, file_id: &str, id: &str) -> bool {
    let records = legacy_records(doc, file_id);
    if !records.contains_key(id) {
        return false;
    }
    let expanded = expand_legacy(&records);
    // The outer record holding this id's fragments, which may be another one.
    let Some(outer) = expanded
        .iter()
        .position(|(c, fs)| c.id == id && !fs.is_empty())
        .or_else(|| {
            expanded
                .iter()
                .position(|(_, fs)| fs.iter().any(|(f, _)| f.change == id))
        })
        .or_else(|| expanded.iter().position(|(c, _)| c.id == id))
    else {
        return false;
    };
    let outer_fragments = expanded[outer].1.clone();
    let mut group = vec![outer];
    group.extend((0..expanded.len()).filter(|&i| {
        i != outer
            && outer_fragments
                .iter()
                .any(|(f, _)| f.change == expanded[i].0.id)
    }));
    let owners: HashMap<&str, &LegacyChange> = group
        .iter()
        .map(|&i| (expanded[i].0.id.as_str(), &expanded[i].0))
        .collect();
    let root = doc.get_map(CHANGES);
    for &i in &group {
        let c = &expanded[i].0;
        root.delete(&c.id).ok();
        let Ok(m) = root.insert_container(&c.id, LoroMap::new()) else {
            continue;
        };
        m.insert("kind", LoroValue::from("change")).ok();
        m.insert("file", LoroValue::from(file_id)).ok();
        m.insert("who", LoroValue::from(c.who.as_str())).ok();
        m.insert("author", LoroValue::from(c.author.as_str())).ok();
        m.insert("run", run_value(c.run.as_deref())).ok();
        m.insert("at", LoroValue::Double(c.at as f64)).ok();
        // A span keeps its start only together with its end.
        let (start, end) = match &c.end {
            Some(end) => (
                LoroValue::from(c.start.clone()),
                LoroValue::from(end.clone()),
            ),
            None => (LoroValue::Null, LoroValue::Null),
        };
        m.insert("start", start).ok();
        m.insert("end", end).ok();
    }
    for (f, anchor) in &outer_fragments {
        let Some(fid) = &f.id else {
            continue;
        };
        root.delete(fid).ok();
        let Ok(m) = root.insert_container(fid, LoroMap::new()) else {
            continue;
        };
        let owner = owners.get(f.change.as_str()).copied();
        m.insert("kind", LoroValue::from("fragment")).ok();
        m.insert("file", LoroValue::from(file_id)).ok();
        m.insert("who", LoroValue::from(owner.map_or("", |o| o.who.as_str())))
            .ok();
        m.insert(
            "author",
            LoroValue::from(owner.map_or("", |o| o.author.as_str())),
        )
        .ok();
        m.insert("run", run_value(owner.and_then(|o| o.run.as_deref())))
            .ok();
        m.insert("at", LoroValue::Double(owner.map_or(0, |o| o.at) as f64))
            .ok();
        m.insert("change", LoroValue::from(f.change.as_str())).ok();
        m.insert("text", LoroValue::from(f.text.as_str())).ok();
        m.insert("start", LoroValue::from(anchor.clone())).ok();
        m.insert("rank", LoroValue::Double(rank_of(f))).ok();
    }
    true
}

/// The legacy record a fragment id `<record id>:<index>` names, when the id has
/// that shape.
fn legacy_prefix(id: &str) -> Option<&str> {
    let (legacy, _) = id.rsplit_once(':')?;
    (!legacy.is_empty()).then_some(legacy)
}

/// Makes record `id` a container before it is written: converts the legacy
/// record it is, or the one its fragment id names.
fn materialize(doc: &LoroDoc, file_id: &str, id: &str) {
    let root = doc.get_map(CHANGES);
    if record_map(&root, id).is_some() {
        return;
    }
    let legacy_id = if root.get(id).is_some() {
        id
    } else {
        legacy_prefix(id).unwrap_or(id)
    };
    convert_legacy(doc, file_id, legacy_id);
}

/// Converts the legacy record `id`, when it is one, before a decision reads it.
fn convert_plain(doc: &LoroDoc, id: &str) {
    let root = doc.get_map(CHANGES);
    if let Some(ValueOrContainer::Value(LoroValue::Map(m))) = root.get(id) {
        if let Some(file) = m.get("file").and_then(plain_str) {
            convert_legacy(doc, &file, id);
        }
    }
}

/// The deleted text of change `id`: its fragments in gap order.
fn deleted_of(gaps: &[Gap], id: &str) -> String {
    gaps.iter()
        .flat_map(|g| g.fragments.iter())
        .filter(|f| f.change == id)
        .map(|f| f.text.as_str())
        .collect()
}

/// The pending changes of one file, resolved against its text now and sorted
/// by `(from, to, id)`. Empty changes are left out.
pub fn resolve_changes(doc: &LoroDoc, file_id: &str) -> Vec<ResolvedChange> {
    let state = resolve_file(doc, file_id).state;
    state
        .changes
        .iter()
        .map(|c| ResolvedChange {
            id: c.id.clone(),
            file: file_id.to_string(),
            who: c.who.clone(),
            author: c.author.clone(),
            run: c.run.clone(),
            at: c.at,
            from: c.from,
            to: c.to,
            deleted: deleted_of(&state.gaps, &c.id),
        })
        .collect()
}

/// Writes a change record: a new container, or only the keys that changed. A
/// cursor is rewritten only when it would resolve to another offset than the
/// stored one does, so a span that shrank needs no write, and two peers moving
/// its two ends at once both keep their write. Nothing is written over a cursor
/// with null. With `heal`, a stale cursor is rewritten even where it resolves
/// to the right offset. A legacy record is converted first. Returns false when
/// no cursor can be made.
fn write_change(doc: &LoroDoc, text: &LoroText, file: &str, c: &ChangeRecord, heal: bool) -> bool {
    let cursors = if c.to > c.from {
        match (
            text.get_cursor(c.from, Side::Left),
            text.get_cursor(c.to - 1, Side::Right),
        ) {
            (Some(start), Some(end)) => Some((start.encode(), end.encode())),
            _ => return false,
        }
    } else {
        None
    };
    materialize(doc, file, &c.id);
    let root = doc.get_map(CHANGES);
    let m = match record_map(&root, &c.id) {
        Some(m) => m,
        None => {
            let Ok(m) = root.insert_container(&c.id, LoroMap::new()) else {
                return false;
            };
            m.insert("kind", LoroValue::from("change")).ok();
            m.insert("file", LoroValue::from(file)).ok();
            m
        }
    };
    put(&m, "who", LoroValue::from(c.who.as_str()));
    put(&m, "author", LoroValue::from(c.author.as_str()));
    put(&m, "run", run_value(c.run.as_deref()));
    put(&m, "at", LoroValue::Double(c.at as f64));
    match cursors {
        Some((start, end)) => {
            let have = resolved_span(doc, text, &m);
            let stale_start = heal
                && bytes_of(&m, "start")
                    .and_then(|b| cursor_pos(doc, text, &b))
                    .is_some_and(|p| p.stale);
            let stale_end = heal
                && bytes_of(&m, "end")
                    .and_then(|b| cursor_pos(doc, text, &b))
                    .is_some_and(|p| p.stale);
            if have.map(|h| h.0) != Some(c.from) || stale_start {
                m.insert("start", LoroValue::from(start)).ok();
            }
            if have.map(|h| h.1) != Some(c.to) || stale_end {
                m.insert("end", LoroValue::from(end)).ok();
            }
        }
        None => {
            if m.get("start").is_none() {
                m.insert("start", LoroValue::Null).ok();
                m.insert("end", LoroValue::Null).ok();
            }
        }
    }
    true
}

/// Sets the cursor under `key` to `bytes`, unless the stored one is the same
/// cursor or already resolves to `offset`.
fn move_cursor(
    doc: &LoroDoc,
    text: &LoroText,
    m: &LoroMap,
    key: &str,
    bytes: &[u8],
    offset: usize,
) {
    if let Some(stored) = bytes_of(m, key) {
        if same_cursor(&stored, bytes) {
            return;
        }
        if cursor_pos(doc, text, &stored).is_some_and(|p| p.offset == offset) {
            return;
        }
    }
    m.insert(key, LoroValue::from(bytes.to_vec())).ok();
}

/// Ranks for the fragments of one gap, in document order. The longest run of
/// existing ranks that rises in that order keeps its values; the others take
/// values between their neighbours, one past the last, or one before the first,
/// so that a peer adding a fragment beside one of ours does not move ours.
fn rank_gap(fragments: &[Fragment]) -> Vec<f64> {
    let have: Vec<Option<f64>> = fragments.iter().map(|f| f.rank).collect();
    let n = have.len();
    let mut best = vec![1usize; n];
    let mut prev: Vec<Option<usize>> = vec![None; n];
    for (i, ri) in have.iter().enumerate() {
        let Some(ri) = *ri else {
            best[i] = 0;
            continue;
        };
        for (j, rj) in have.iter().enumerate().take(i) {
            if let Some(rj) = *rj {
                if rj < ri && best[j] + 1 > best[i] {
                    best[i] = best[j] + 1;
                    prev[i] = Some(j);
                }
            }
        }
    }
    let mut end: Option<usize> = None;
    for (i, b) in best.iter().enumerate() {
        if *b == 0 {
            continue;
        }
        let better = match end {
            None => true,
            Some(e) => *b > best[e],
        };
        if better {
            end = Some(i);
        }
    }
    let mut keep = vec![false; n];
    let mut cur = end;
    while let Some(i) = cur {
        keep[i] = true;
        cur = prev[i];
    }
    let mut out: Vec<Option<f64>> = have
        .iter()
        .zip(&keep)
        .map(|(r, k)| if *k { *r } else { None })
        .collect();
    for i in 0..n {
        if keep[i] {
            continue;
        }
        let lo = out[..i]
            .iter()
            .rev()
            .find_map(|v| *v)
            .unwrap_or(f64::NEG_INFINITY);
        let hi = (i + 1..n)
            .find(|&j| keep[j])
            .and_then(|j| out[j])
            .unwrap_or(f64::INFINITY);
        let mut v = if lo == f64::NEG_INFINITY && hi == f64::INFINITY {
            0.0
        } else if lo == f64::NEG_INFINITY {
            hi - 1.0
        } else if hi == f64::INFINITY {
            lo + 1.0
        } else {
            (lo + hi) / 2.0
        };
        // The midpoint collapsed onto a neighbour: take one past the lower
        // one, and let the kept values after it go.
        if v <= lo || v >= hi {
            v = lo + 1.0;
            for k in keep.iter_mut().skip(i + 1) {
                *k = false;
            }
        }
        out[i] = Some(v);
    }
    out.into_iter().map(|v| v.unwrap_or(0.0)).collect()
}

/// Writes one fragment record: a new container, with its owner's author fields
/// when it has an owner, or only the keys that changed. The start cursor moves
/// only when it would resolve elsewhere, or when `heal` finds it stale. A
/// fragment of a legacy record converts that record, with its whole group, only
/// when something about the fragment changes.
#[allow(clippy::too_many_arguments)]
fn write_fragment(
    doc: &LoroDoc,
    text: &LoroText,
    file: &str,
    id: &str,
    f: &Fragment,
    start: &[u8],
    at: usize,
    rank: f64,
    owner: Option<&ChangeRecord>,
    heal: bool,
) {
    let root = doc.get_map(CHANGES);
    if record_map(&root, id).is_none() {
        if let Some(legacy_id) = legacy_prefix(id) {
            let records = legacy_records(doc, file);
            if let Some(legacy) = records.get(legacy_id) {
                let index = id.rsplit(':').next().and_then(|i| i.parse::<usize>().ok());
                let was = index.and_then(|i| {
                    expand_legacy(&records)
                        .into_iter()
                        .find(|(c, _)| c.id == legacy_id)
                        .and_then(|(_, fragments)| fragments.into_iter().nth(i))
                });
                let same = !heal
                    && was.is_some_and(|(w, _)| {
                        w.change == f.change && w.text == f.text && w.rank == Some(rank)
                    })
                    && (same_cursor(&legacy.start, start)
                        || cursor_pos(doc, text, &legacy.start).is_some_and(|p| p.offset == at));
                if same {
                    return;
                }
                convert_legacy(doc, file, legacy_id);
            }
        }
    }
    let m = match record_map(&root, id) {
        Some(m) => m,
        None => {
            let Ok(m) = root.insert_container(id, LoroMap::new()) else {
                return;
            };
            m.insert("kind", LoroValue::from("fragment")).ok();
            m.insert("file", LoroValue::from(file)).ok();
            if let Some(o) = owner {
                m.insert("who", LoroValue::from(o.who.as_str())).ok();
                m.insert("author", LoroValue::from(o.author.as_str())).ok();
                m.insert("run", run_value(o.run.as_deref())).ok();
                m.insert("at", LoroValue::Double(o.at as f64)).ok();
            }
            m
        }
    };
    // A fragment read as its own change keeps the change it had: its stored
    // pointer stays at the gone change.
    if f.change != id {
        put(&m, "change", LoroValue::from(f.change.as_str()));
    }
    put(&m, "text", LoroValue::from(f.text.as_str()));
    let stale_start = heal
        && bytes_of(&m, "start")
            .and_then(|b| cursor_pos(doc, text, &b))
            .is_some_and(|p| p.stale);
    if stale_start {
        m.insert("start", LoroValue::from(start.to_vec())).ok();
    } else {
        move_cursor(doc, text, &m, "start", start, at);
    }
    put(&m, "rank", LoroValue::Double(rank));
}

/// Writes the fragments of one gap at `at`, each its own record, with ranks from
/// `rank_gap`. A fragment without an id gets a fresh one. Writes nothing when no
/// cursor can be made at `at`, and returns true otherwise. `heal` is passed on
/// to `write_fragment`.
fn write_gap(
    doc: &LoroDoc,
    text: &LoroText,
    file: &str,
    at: usize,
    fragments: &[Fragment],
    owners: &HashMap<String, ChangeRecord>,
    heal: bool,
) -> bool {
    let Some(start) = text.get_cursor(at, Side::Left) else {
        return false;
    };
    let bytes = start.encode();
    let ranks = rank_gap(fragments);
    for (f, rank) in fragments.iter().zip(ranks) {
        let id = f.id.clone().unwrap_or_else(mint_uuid);
        write_fragment(
            doc,
            text,
            file,
            &id,
            f,
            &bytes,
            at,
            rank,
            owners.get(f.change.as_str()),
            heal,
        );
    }
    true
}

/// Writes a plan against the file's current text. Does not commit. Returns the
/// ids of the change records it set.
pub fn write_planned(doc: &LoroDoc, file_id: &str, plan: &Plan) -> Vec<String> {
    let Some(text) = text_of(doc, file_id) else {
        return Vec::new();
    };
    let root = doc.get_map(CHANGES);
    let live: HashSet<String> = plan
        .gaps
        .iter()
        .filter(|(_, st)| *st != PlanState::Removed)
        .flat_map(|(g, _)| g.fragments.iter().filter_map(|f| f.id.clone()))
        .collect();
    let owners: HashMap<String, ChangeRecord> = plan
        .changes
        .iter()
        .filter(|(_, st)| *st != PlanState::Removed)
        .map(|(c, _)| (c.id.clone(), c.clone()))
        .collect();
    for (g, st) in &plan.gaps {
        match st {
            PlanState::Removed => {
                for id in g.fragments.iter().filter_map(|f| f.id.as_ref()) {
                    if !live.contains(id) {
                        root.delete(id).ok();
                    }
                }
            }
            PlanState::Set => {
                write_gap(doc, &text, file_id, g.at, &g.fragments, &owners, false);
            }
            PlanState::Kept => {}
        }
    }
    let mut written = Vec::new();
    for (c, st) in &plan.changes {
        match st {
            PlanState::Removed => {
                root.delete(&c.id).ok();
            }
            PlanState::Set => {
                if !c.orphan && write_change(doc, &text, file_id, c, false) {
                    written.push(c.id.clone());
                }
            }
            PlanState::Kept => {}
        }
    }
    written
}

fn text_of(doc: &LoroDoc, file_id: &str) -> Option<LoroText> {
    match doc.get_map(FILES).get(file_id)? {
        ValueOrContainer::Container(Container::Text(t)) => Some(t),
        _ => None,
    }
}

/// A random version 4 UUID in its hyphenated form.
fn mint_uuid() -> String {
    let mut b = librepaper_base::util::random_bytes(16);
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h = hex::encode(b);
    format!(
        "{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    )
}

/// Applies `edits` to the file's text and plans their records. `None` when
/// `untracked` and the file has no records to keep in step. Errors, leaving
/// the document untouched, when the file is missing or the edits do not fit
/// its text. Does not commit.
#[allow(clippy::too_many_arguments)]
fn apply_planned(
    doc: &LoroDoc,
    file_id: &str,
    edits: &[Edit],
    by: &Author,
    run: Option<&str>,
    now_ms: i64,
    untracked: bool,
) -> Result<Option<Plan>, String> {
    let text = text_of(doc, file_id).ok_or_else(|| "the file does not exist".to_string())?;
    let before = text.to_string();
    // Resolved before the text moves: the cursors are read where they stand.
    let state = resolve_file(doc, file_id).state;

    let to_cp = |utf16: usize| {
        text.convert_pos(utf16, PosType::Utf16, PosType::Unicode)
            .ok_or_else(|| "an edit does not fit the file".to_string())
    };
    let mut planned: Vec<(usize, usize, String)> = Vec::with_capacity(edits.len());
    for e in edits {
        let from = to_cp(e.at)?;
        let to = to_cp(e.at.saturating_add(e.delete))?;
        planned.push((from, to, e.insert.clone()));
    }
    if !apply_text_edits(&text, edits) {
        return Err("an edit does not fit the file".to_string());
    }
    if untracked && state.changes.is_empty() && state.gaps.is_empty() {
        return Ok(None);
    }

    let mut mint = mint_uuid;
    let plan = plan_edits(
        &before, &state, &planned, by, run, now_ms, untracked, &mut mint,
    );
    Ok(Some(plan))
}

/// Applies `edits` (UTF-16, measured against the file as it is now, ascending
/// and non-overlapping, as `apply_edits_at` takes them) to the file's text and
/// records them as tracked changes by `by`. Returns the ids of the change
/// records it set. Errors, leaving the document untouched, when the file is
/// missing or the edits do not fit its text. Does not commit.
pub fn apply_tracked_edits(
    doc: &LoroDoc,
    file_id: &str,
    edits: &[Edit],
    by: &Author,
    run: Option<&str>,
    now_ms: i64,
) -> Result<Vec<String>, String> {
    let plan = apply_planned(doc, file_id, edits, by, run, now_ms, false)?;
    Ok(plan.map_or_else(Vec::new, |plan| write_planned(doc, file_id, &plan)))
}

/// Applies `edits` to the file's text without tracking, keeping the records in
/// step: pending spans shrink, move or split around them, and gaps the edits
/// collapse merge in order. Records nothing and makes no change of anyone's.
/// Errors, leaving the document untouched, when the file is missing or the
/// edits do not fit its text. Does not commit.
pub fn untracked_edits(doc: &LoroDoc, file_id: &str, edits: &[Edit]) -> Result<(), String> {
    // Nobody and no time: an untracked edit makes no change of its own.
    let nobody = Author {
        who: "",
        author: "",
    };
    if let Some(plan) = apply_planned(doc, file_id, edits, &nobody, None, 0, true)? {
        write_planned(doc, file_id, &plan);
    }
    Ok(())
}

/// Puts the fragments of `pending` back as a run anchored on the character at
/// `anchor`, and empties the run. Each fragment keeps its rank.
fn flush(doc: &LoroDoc, text: &LoroText, file: &str, anchor: usize, pending: &mut Vec<Fragment>) {
    if pending.is_empty() {
        return;
    }
    if let Some(start) = text.get_cursor(anchor, Side::Left) {
        let bytes = start.encode();
        for f in pending.iter() {
            if let Some(id) = &f.id {
                write_fragment(
                    doc,
                    text,
                    file,
                    id,
                    f,
                    &bytes,
                    anchor,
                    rank_of(f),
                    None,
                    false,
                );
            }
        }
    }
    pending.clear();
}

/// Puts back the gaps of one point `at` for change `id`. Their fragments are
/// walked in order: the rejected change's text is inserted where the walk
/// stands, and each run of other changes' fragments is anchored on the
/// character that follows it. Where gaps collapsed onto one point, later gaps
/// are raised above earlier ones.
fn restore_group(
    doc: &LoroDoc,
    text: &LoroText,
    file: &str,
    id: &str,
    at: usize,
    mut list: Vec<Gap>,
) {
    list.sort_by_key(|g| g.at);
    let mut floor = f64::NEG_INFINITY;
    let mut fragments: Vec<Fragment> = Vec::new();
    for g in &list {
        let low = g
            .fragments
            .iter()
            .map(rank_of)
            .fold(f64::INFINITY, f64::min);
        let high = g
            .fragments
            .iter()
            .map(rank_of)
            .fold(f64::NEG_INFINITY, f64::max);
        let shift = if low <= floor { floor - low + 1.0 } else { 0.0 };
        for f in &g.fragments {
            fragments.push(Fragment {
                rank: Some(rank_of(f) + shift),
                ..f.clone()
            });
        }
        floor = high + shift;
    }
    let mut pos = at;
    let mut pending: Vec<Fragment> = Vec::new();
    for f in fragments {
        if f.change != id {
            pending.push(f);
            continue;
        }
        let n = f.text.chars().count();
        // Spans that end or start at the point keep their bounds across the
        // restored text; a cursor left on a deleted character might not.
        let edges: Vec<ChangeRecord> = resolve_file(doc, file)
            .state
            .changes
            .into_iter()
            .filter(|c| c.id != id && c.to > c.from && (c.to == pos || c.from == pos))
            .collect();
        text.insert(pos, &f.text).ok();
        for c in edges {
            if c.to == pos {
                write_change(doc, text, file, &c, false);
            } else {
                let shifted = ChangeRecord {
                    from: c.from + n,
                    to: c.to + n,
                    ..c
                };
                write_change(doc, text, file, &shifted, false);
            }
        }
        flush(doc, text, file, pos, &mut pending);
        pos += n;
        if let Some(fid) = &f.id {
            doc.get_map(CHANGES).delete(fid).ok();
        }
    }
    flush(doc, text, file, pos, &mut pending);
}

/// Puts back the text of change `id` and the gaps it sat in. The gaps beyond its
/// span come back first, while its text still holds their offsets; then the
/// span goes, and the gaps at and before it come back.
fn restore_change(doc: &LoroDoc, text: &LoroText, file: &str, id: &str) {
    let state = resolve_file(doc, file).state;
    let span = state
        .changes
        .iter()
        .find(|c| c.id == id)
        .filter(|c| c.to > c.from)
        .map(|c| (c.from, c.to));
    // The gaps to put back: every gap holding this change's fragments and, with
    // a span, every gap from its start to the character after it. Those collapse
    // onto the span's start when it goes.
    let mut groups: BTreeMap<usize, Vec<Gap>> = BTreeMap::new();
    for g in &state.gaps {
        let attached = span.is_some_and(|(from, to)| g.at >= from && g.at <= to);
        if !attached && !g.fragments.iter().any(|f| f.change == id) {
            continue;
        }
        let at = match span {
            Some((from, _)) if attached => from,
            _ => g.at,
        };
        groups.entry(at).or_default().push(g.clone());
    }
    let positions: Vec<usize> = groups.keys().rev().copied().collect();
    for at in &positions {
        let beyond = match span {
            Some((_, to)) => *at > to,
            None => true,
        };
        if beyond {
            if let Some(list) = groups.remove(at) {
                restore_group(doc, text, file, id, *at, list);
            }
        }
    }
    if let Some((from, to)) = span {
        text.delete(from, to - from).ok();
    }
    for at in &positions {
        if let Some(list) = groups.remove(at) {
            restore_group(doc, text, file, id, *at, list);
        }
    }
}

/// Whether `id` names a record a decision applies to: a change record, or a
/// fragment record whose change is gone, which is read as a change of its own.
fn decidable(root: &LoroMap, id: &str) -> bool {
    let Some(m) = record_map(root, id) else {
        return false;
    };
    match str_of(&m, "kind").as_deref() {
        Some("change") => true,
        Some("fragment") => !str_of(&m, "change")
            .and_then(|parent| record_map(root, &parent))
            .is_some_and(|p| str_of(&p, "kind").as_deref() == Some("change")),
        _ => false,
    }
}

/// Accepts a change: its fragments leave every gap (a gap left empty goes), and
/// its record is dropped. The text stays. Returns false when `id` is neither a
/// change record nor a fragment read as one. Does not commit.
pub fn accept(doc: &LoroDoc, id: &str) -> bool {
    convert_plain(doc, id);
    let root = doc.get_map(CHANGES);
    if !decidable(&root, id) {
        return false;
    }
    for (fid, m) in records(doc) {
        if str_of(&m, "kind").as_deref() == Some("fragment")
            && str_of(&m, "change").as_deref() == Some(id)
        {
            root.delete(&fid).ok();
        }
    }
    root.delete(id).ok();
    true
}

/// Rejects a change: puts its text back where it was, then drops its fragments
/// and its record. A fragment read as a change of its own restores its text the
/// same way. Returns false when `id` is neither a change record nor a fragment
/// read as one. Does not commit.
pub fn reject(doc: &LoroDoc, id: &str) -> bool {
    convert_plain(doc, id);
    let root = doc.get_map(CHANGES);
    if !decidable(&root, id) {
        return false;
    }
    let file = record_map(&root, id).and_then(|m| str_of(&m, "file"));
    if let Some(file) = file {
        if let Some(text) = text_of(doc, &file) {
            restore_change(doc, &text, &file, id);
        }
    }
    for (fid, m) in records(doc) {
        if str_of(&m, "kind").as_deref() == Some("fragment")
            && str_of(&m, "change").as_deref() == Some(id)
        {
            root.delete(&fid).ok();
        }
    }
    root.delete(id).ok();
    true
}

/// Rejects each distinct id in turn and returns how many were rejected. Does
/// not commit.
pub fn reject_many<S: AsRef<str>>(doc: &LoroDoc, ids: &[S]) -> usize {
    let mut seen: HashSet<String> = HashSet::new();
    let mut count = 0;
    for id in ids {
        let id = id.as_ref();
        if seen.insert(id.to_string()) && reject(doc, id) {
            count += 1;
        }
    }
    count
}

/// How long an empty record rests, from the first prune that sees it empty,
/// before `prune` drops it, and how long a record rests before `prune` heals a
/// stale cursor on it: its author's other session needs time to sync.
const SETTLE_MS: i64 = 60_000;

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// Drops empty records and heals stale cursors. Records of a file with no text
/// are dropped. Returns the number of records deleted. Does not commit. See
/// `prune_at`.
pub fn prune(doc: &LoroDoc) -> usize {
    prune_at(doc, now_ms())
}

/// `prune`, with the clock given, so that a test can let time pass.
///
/// An empty record is dropped only once a prune has seen it empty for a while:
/// the first prune that finds it empty marks it `emptied`, and a record found
/// live again loses the mark. A stale cursor is healed the same way: the first
/// prune that finds it stale marks its record `stale`, and the heal waits until
/// that mark is old. A record whose cursor is live again loses the mark, and
/// the mark goes once the cursor is healed. Derived orphan changes are skipped
/// by the heal, since their fragments are healed as gap fragments. Records of a
/// file with no text are dropped. Returns the number of records deleted. Does
/// not commit.
pub fn prune_at(doc: &LoroDoc, now: i64) -> usize {
    let root = doc.get_map(CHANGES);
    let files: BTreeSet<String> = records(doc)
        .iter()
        .filter_map(|(_, m)| str_of(m, "file"))
        .collect();
    let mut removed = 0;
    for file in files {
        let Some(text) = text_of(doc, &file) else {
            for (id, m) in records(doc) {
                if str_of(&m, "file").as_deref() == Some(file.as_str()) {
                    root.delete(&id).ok();
                    removed += 1;
                }
            }
            continue;
        };
        let resolved = resolve_file(doc, &file);
        let live: HashSet<String> = resolved
            .state
            .changes
            .iter()
            .map(|c| c.id.clone())
            .chain(
                resolved
                    .state
                    .gaps
                    .iter()
                    .flat_map(|g| g.fragments.iter().filter_map(|f| f.id.clone())),
            )
            .collect();
        for (id, m) in records(doc) {
            if str_of(&m, "file").as_deref() != Some(file.as_str()) {
                continue;
            }
            if live.contains(&id) {
                if m.get("emptied").is_some() {
                    m.delete("emptied").ok();
                }
                continue;
            }
            match num_of(&m, "emptied") {
                None => {
                    m.insert("emptied", LoroValue::Double(now as f64)).ok();
                }
                Some(since) if (since as i64) < now - SETTLE_MS => {
                    root.delete(&id).ok();
                    removed += 1;
                }
                Some(_) => {}
            }
        }
        // A legacy record with nothing left to show is dropped too.
        for (id, rec) in legacy_records(doc, &file) {
            if !live.contains(&id) && rec.points.is_empty() {
                root.delete(&id).ok();
                removed += 1;
            }
        }
        // A stale cursor is healed only once a prune has seen it stale for a
        // while, counted from that first sight rather than from the record's
        // last edit: the author may be moving that very end of the span in a
        // session that has not synced yet, and the heal would race it.
        let stale_settled = |id: &str| -> bool {
            let Some(m) = record_map(&root, id) else {
                return true;
            };
            match num_of(&m, "stale") {
                None => {
                    m.insert("stale", LoroValue::Double(now as f64)).ok();
                    false
                }
                Some(since) => (since as i64) < now - SETTLE_MS,
            }
        };
        let unmark = |id: &str| {
            if let Some(m) = record_map(&root, id) {
                if m.get("stale").is_some() {
                    m.delete("stale").ok();
                }
            }
        };
        for g in &resolved.state.gaps {
            if !g.stale {
                for f in &g.fragments {
                    if let Some(id) = f.id.as_deref() {
                        unmark(id);
                    }
                }
                continue;
            }
            let all_settled: Vec<bool> = g
                .fragments
                .iter()
                .map(|f| f.id.as_deref().is_none_or(&stale_settled))
                .collect();
            if all_settled.iter().all(|settled| *settled)
                && write_gap(doc, &text, &file, g.at, &g.fragments, &HashMap::new(), true)
            {
                for f in &g.fragments {
                    if let Some(id) = f.id.as_deref() {
                        unmark(id);
                    }
                }
            }
        }
        for c in &resolved.state.changes {
            if c.orphan {
                continue;
            }
            if !c.stale {
                unmark(c.id.as_str());
                continue;
            }
            if c.to > c.from
                && stale_settled(c.id.as_str())
                && write_change(doc, &text, &file, c, true)
            {
                unmark(c.id.as_str());
            }
        }
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::session::{new_doc, put_text};
    use serde::Deserialize;

    const FIXTURES: &str = include_str!("../../../../web/tests/fixtures/track-changes.json");

    #[derive(Deserialize)]
    struct FxFragment {
        change: String,
        text: String,
    }

    #[derive(Deserialize)]
    struct FxChange {
        id: String,
        who: String,
        from: usize,
        to: usize,
    }

    #[derive(Deserialize)]
    struct FxGap {
        at: usize,
        fragments: Vec<FxFragment>,
    }

    #[derive(Deserialize)]
    struct FxState {
        changes: Vec<FxChange>,
        gaps: Vec<FxGap>,
    }

    #[derive(Deserialize)]
    struct FxMe {
        who: String,
    }

    #[derive(Deserialize)]
    struct FxEdit {
        from: usize,
        to: usize,
        insert: String,
    }

    #[derive(Deserialize)]
    struct Fixture {
        name: String,
        before: String,
        state: FxState,
        me: FxMe,
        edits: Vec<FxEdit>,
        #[serde(default)]
        untracked: bool,
        after: String,
        expect: FxState,
    }

    /// Converts a UTF-16 offset in `s` to a code point offset.
    fn cp(s: &str, utf16: usize) -> usize {
        let mut units = 0;
        for (i, ch) in s.chars().enumerate() {
            if units >= utf16 {
                return i;
            }
            units += ch.len_utf16();
        }
        s.chars().count()
    }

    fn frags(list: &[FxFragment]) -> Vec<Fragment> {
        list.iter()
            .map(|f| Fragment {
                id: None,
                change: f.change.clone(),
                text: f.text.clone(),
                rank: None,
                stale: false,
            })
            .collect()
    }

    /// A row of the fixture comparison: id, who, from, to.
    type ChangeRow = (String, String, usize, usize);
    /// A gap of the fixture comparison: at, then (change, text) fragments.
    type GapRow = (usize, Vec<(String, String)>);

    fn sort_changes(rows: &mut [ChangeRow]) {
        rows.sort_by(|a, b| (a.2, a.3, &a.0).cmp(&(b.2, b.3, &b.0)));
    }

    fn fragment_pairs(gap: &Gap) -> Vec<(String, String)> {
        gap.fragments
            .iter()
            .map(|f| (f.change.clone(), f.text.clone()))
            .collect()
    }

    #[test]
    fn shared_fixtures() {
        let fixtures: Vec<Fixture> = serde_json::from_str(FIXTURES).unwrap();
        assert!(!fixtures.is_empty());
        for fx in fixtures {
            let state = FileState {
                changes: fx
                    .state
                    .changes
                    .iter()
                    .map(|c| ChangeRecord {
                        id: c.id.clone(),
                        who: c.who.clone(),
                        author: c.who.clone(),
                        run: None,
                        at: 1,
                        from: cp(&fx.before, c.from),
                        to: cp(&fx.before, c.to),
                        emptied: false,
                        stale: false,
                        orphan: false,
                    })
                    .collect(),
                gaps: fx
                    .state
                    .gaps
                    .iter()
                    .enumerate()
                    .map(|(i, g)| Gap {
                        id: format!("gap-{i}"),
                        at: cp(&fx.before, g.at),
                        fragments: frags(&g.fragments),
                        stale: false,
                    })
                    .collect(),
            };
            let edits: Vec<(usize, usize, String)> = fx
                .edits
                .iter()
                .map(|e| {
                    (
                        cp(&fx.before, e.from),
                        cp(&fx.before, e.to),
                        e.insert.clone(),
                    )
                })
                .collect();
            let me = Author {
                who: &fx.me.who,
                author: &fx.me.who,
            };
            let mut counter = 0;
            let mut mint = || {
                counter += 1;
                format!("new-{counter}")
            };
            let plan = plan_edits(
                &fx.before,
                &state,
                &edits,
                &me,
                None,
                2,
                fx.untracked,
                &mut mint,
            );

            let mut got_changes: Vec<ChangeRow> = plan
                .changes
                .iter()
                .filter(|(c, st)| *st != PlanState::Removed && !c.emptied)
                .map(|(c, _)| (c.id.clone(), c.who.clone(), c.from, c.to))
                .collect();
            let mut got_gaps: Vec<GapRow> = plan
                .gaps
                .iter()
                .filter(|(_, st)| *st != PlanState::Removed)
                .map(|(g, _)| (g.at, fragment_pairs(g)))
                .collect();
            let mut want_changes: Vec<ChangeRow> = fx
                .expect
                .changes
                .iter()
                .map(|c| {
                    (
                        c.id.clone(),
                        c.who.clone(),
                        cp(&fx.after, c.from),
                        cp(&fx.after, c.to),
                    )
                })
                .collect();
            let mut want_gaps: Vec<GapRow> = fx
                .expect
                .gaps
                .iter()
                .map(|g| {
                    (
                        cp(&fx.after, g.at),
                        g.fragments
                            .iter()
                            .map(|f| (f.change.clone(), f.text.clone()))
                            .collect(),
                    )
                })
                .collect();
            sort_changes(&mut got_changes);
            sort_changes(&mut want_changes);
            got_gaps.sort();
            want_gaps.sort();
            assert_eq!(got_changes, want_changes, "fixture: {}", fx.name);
            assert_eq!(got_gaps, want_gaps, "fixture gaps: {}", fx.name);
        }
    }

    const ME: Author<'static> = Author {
        who: "me",
        author: "Me",
    };
    const BOB: Author<'static> = Author {
        who: "bob",
        author: "Bob",
    };
    const CARA: Author<'static> = Author {
        who: "cara",
        author: "Cara",
    };
    const DAN: Author<'static> = Author {
        who: "dan",
        author: "Dan",
    };

    /// The author of the `i`th edit of a shape.
    fn author_for(i: usize) -> Author<'static> {
        match i % 4 {
            0 => ME,
            1 => BOB,
            2 => CARA,
            _ => DAN,
        }
    }

    fn who_of(w: &str) -> Author<'_> {
        Author { who: w, author: w }
    }

    const ALPHA: &str = "abcdefghijkl";
    const AUTHORS: [&str; 4] = ["p", "q", "r", "s"];

    /// The generator of the JS fuzzers. The product is a double, rounded
    /// before the low 31 bits are taken, as the JS arithmetic does it.
    struct Lcg {
        seed: u64,
    }

    impl Lcg {
        fn new(seed: u64) -> Self {
            Lcg { seed }
        }

        fn rnd(&mut self) -> f64 {
            let x = self.seed as f64 * 1_103_515_245.0 + 12_345.0;
            self.seed = (x % 4_294_967_296.0) as u64 & 0x7fff_ffff;
            self.seed as f64 / 2_147_483_647.0
        }

        fn pick(&mut self, n: usize) -> usize {
            (self.rnd() * n as f64).floor() as usize
        }

        fn shuffle<T>(&mut self, xs: &mut [T]) {
            for i in (1..xs.len()).rev() {
                let j = self.pick(i + 1);
                xs.swap(i, j);
            }
        }
    }

    fn permutations<T: Clone>(items: &[T]) -> Vec<Vec<T>> {
        if items.is_empty() {
            return vec![Vec::new()];
        }
        let mut out = Vec::new();
        for i in 0..items.len() {
            let mut rest = items.to_vec();
            let head = rest.remove(i);
            for mut tail in permutations(&rest) {
                tail.insert(0, head.clone());
                out.push(tail);
            }
        }
        out
    }

    fn setup(body: &str) -> (LoroDoc, String) {
        let doc = new_doc();
        let id = put_text(&doc, "main.md", body);
        doc.commit();
        (doc, id)
    }

    fn body(doc: &LoroDoc, id: &str) -> String {
        text_of(doc, id).unwrap().to_string()
    }

    fn edit(at: usize, delete: usize, insert: &str) -> Edit {
        Edit {
            at,
            delete,
            insert: insert.to_string(),
        }
    }

    /// Applies a shape's edits, each by its own author, committing after each.
    fn build_shape(start: &str, edits: &[(usize, usize, &str)]) -> (LoroDoc, String) {
        let (doc, id) = setup(start);
        for (i, &(from, len, insert)) in edits.iter().enumerate() {
            apply_tracked_edits(
                &doc,
                &id,
                &[edit(from, len, insert)],
                &author_for(i),
                None,
                1,
            )
            .unwrap();
            doc.commit();
        }
        (doc, id)
    }

    /// The id of each live change of `id`, by its author key. Each author in
    /// the shapes below has one change.
    fn who_ids(doc: &LoroDoc, id: &str) -> HashMap<String, String> {
        resolve_changes(doc, id)
            .into_iter()
            .map(|c| (c.who, c.id))
            .collect()
    }

    /// Bob's own change among the ids his edit wrote.
    fn bobs(ids: Vec<String>, alice: &str) -> String {
        ids.into_iter()
            .find(|id| id != alice)
            .expect("bob's change")
    }

    /// Alice deletes, then Bob's edit swallows her gap. Returns both ids.
    fn swallow(start: &str, alice: Edit, bob: Edit) -> (LoroDoc, String, String, String) {
        let (doc, id) = setup(start);
        let a = apply_tracked_edits(&doc, &id, &[alice], &ME, None, 1).unwrap()[0].clone();
        doc.commit();
        let b = bobs(
            apply_tracked_edits(&doc, &id, &[bob], &BOB, None, 2).unwrap(),
            &a,
        );
        doc.commit();
        (doc, id, a, b)
    }

    #[test]
    fn records_a_replacement() {
        let (doc, id) = setup("the quick fox");
        let set =
            apply_tracked_edits(&doc, &id, &[edit(4, 5, "slow")], &ME, Some("r1"), 9).unwrap();
        doc.commit();
        assert_eq!(set.len(), 1);
        assert_eq!(body(&doc, &id), "the slow fox");
        let changes = resolve_changes(&doc, &id);
        assert_eq!(changes.len(), 1);
        let c = &changes[0];
        assert_eq!((c.from, c.to), (4, 8));
        assert_eq!(c.deleted, "quick");
        assert_eq!((c.who.as_str(), c.author.as_str()), ("me", "Me"));
        assert_eq!((c.run.as_deref(), c.at), (Some("r1"), 9));
    }

    #[test]
    fn accept_keeps_the_text() {
        let (doc, id) = setup("the quick fox");
        let set = apply_tracked_edits(&doc, &id, &[edit(4, 5, "slow")], &ME, None, 1).unwrap();
        doc.commit();
        assert!(accept(&doc, &set[0]));
        doc.commit();
        assert_eq!(body(&doc, &id), "the slow fox");
        assert!(resolve_changes(&doc, &id).is_empty());
        assert!(!accept(&doc, &set[0]));
    }

    #[test]
    fn reject_restores_a_replacement() {
        let (doc, id) = setup("the quick fox");
        let set = apply_tracked_edits(&doc, &id, &[edit(4, 5, "slow")], &ME, None, 1).unwrap();
        doc.commit();
        assert!(reject(&doc, &set[0]));
        doc.commit();
        assert_eq!(body(&doc, &id), "the quick fox");
        assert!(resolve_changes(&doc, &id).is_empty());
    }

    #[test]
    fn reject_removes_a_pure_insertion() {
        let (doc, id) = setup("the fox");
        let set = apply_tracked_edits(&doc, &id, &[edit(4, 0, "quick ")], &ME, None, 1).unwrap();
        doc.commit();
        assert_eq!(body(&doc, &id), "the quick fox");
        assert!(reject(&doc, &set[0]));
        doc.commit();
        assert_eq!(body(&doc, &id), "the fox");
    }

    #[test]
    fn reject_restores_a_pure_deletion() {
        let (doc, id) = setup("the quick fox");
        let set = apply_tracked_edits(&doc, &id, &[edit(4, 6, "")], &ME, None, 1).unwrap();
        doc.commit();
        assert_eq!(body(&doc, &id), "the fox");
        let changes = resolve_changes(&doc, &id);
        assert_eq!((changes[0].from, changes[0].to), (4, 4));
        assert!(reject(&doc, &set[0]));
        doc.commit();
        assert_eq!(body(&doc, &id), "the quick fox");
    }

    #[test]
    fn deletion_at_the_end_of_the_text_round_trips() {
        let (doc, id) = setup("the fox");
        let set = apply_tracked_edits(&doc, &id, &[edit(3, 4, "")], &ME, None, 1).unwrap();
        doc.commit();
        assert_eq!(body(&doc, &id), "the");
        assert!(reject(&doc, &set[0]));
        doc.commit();
        assert_eq!(body(&doc, &id), "the fox");
    }

    #[test]
    fn changes_can_be_decided_in_either_order() {
        for first_is_a in [true, false] {
            let (doc, id) = setup("one two three four");
            let mut ids = apply_tracked_edits(&doc, &id, &[edit(0, 3, "1")], &ME, None, 1).unwrap();
            doc.commit();
            ids.extend(apply_tracked_edits(&doc, &id, &[edit(6, 5, "3")], &ME, None, 2).unwrap());
            doc.commit();
            assert_eq!(body(&doc, &id), "1 two 3 four");
            assert_eq!(ids.len(), 2);
            let (a, b) = if first_is_a { (0, 1) } else { (1, 0) };
            assert!(reject(&doc, &ids[a]));
            doc.commit();
            assert!(accept(&doc, &ids[b]));
            doc.commit();
            let want = if first_is_a {
                "one two 3 four"
            } else {
                "1 two three four"
            };
            assert_eq!(body(&doc, &id), want);
            assert!(resolve_changes(&doc, &id).is_empty());
        }
    }

    #[test]
    fn astral_text_is_counted_in_code_points() {
        let (doc, id) = setup("a\u{1F600}b");
        let set = apply_tracked_edits(&doc, &id, &[edit(3, 1, "c")], &ME, None, 1).unwrap();
        doc.commit();
        let changes = resolve_changes(&doc, &id);
        assert_eq!((changes[0].from, changes[0].to), (2, 3));
        assert!(reject(&doc, &set[0]));
        doc.commit();
        assert_eq!(body(&doc, &id), "a\u{1F600}b");
    }

    #[test]
    fn misfit_edits_leave_the_document_alone() {
        let (doc, id) = setup("short");
        assert!(apply_tracked_edits(&doc, &id, &[edit(3, 9, "x")], &ME, None, 1).is_err());
        assert!(apply_tracked_edits(&doc, "nope", &[edit(0, 0, "x")], &ME, None, 1).is_err());
        assert_eq!(body(&doc, &id), "short");
        assert!(doc.get_map(CHANGES).keys().next().is_none());
    }

    #[test]
    fn prune_drops_orphans_and_empty_entries() {
        let (doc, id) = setup("the quick fox");
        apply_tracked_edits(&doc, &id, &[edit(4, 5, "slow")], &ME, None, 1).unwrap();
        doc.commit();
        let (other, other_id) = setup("x");
        let orphan =
            apply_tracked_edits(&other, &other_id, &[edit(0, 0, "y")], &ME, None, 1).unwrap();
        doc.import(&other.export(loro::ExportMode::all_updates()).unwrap())
            .unwrap();
        doc.commit();
        assert_eq!(orphan.len(), 1);
        // The orphan's file is gone, so its record resolves to nothing.
        doc.get_map(FILES).delete(&other_id).unwrap();
        // An empty record: no span and no fragment, long settled.
        empty_record(&doc, "empty", &id, 1.0);
        // A record without a kind is not ours to touch.
        doc.get_map(CHANGES).insert("junk", 1).unwrap();
        doc.commit();
        // The orphan goes. The empty record is only marked: its settling starts.
        assert_eq!(prune(&doc), 1);
        doc.commit();
        let root = doc.get_map(CHANGES);
        assert!(record_map(&root, "empty").is_some());
        record_map(&root, "empty")
            .unwrap()
            .insert("emptied", LoroValue::Double(0.0))
            .unwrap();
        doc.commit();
        assert_eq!(prune(&doc), 1);
        assert!(doc.get_map(CHANGES).get("junk").is_some());
        assert_eq!(resolve_changes(&doc, &id).len(), 1);
        assert_eq!(prune(&doc), 0);
    }

    #[test]
    fn swallowed_deletions_reject_in_either_order() {
        for bob_first in [true, false] {
            let (doc, id) = setup("abcd");
            let a =
                apply_tracked_edits(&doc, &id, &[edit(1, 1, "")], &ME, None, 1).unwrap()[0].clone();
            doc.commit();
            let b = bobs(
                apply_tracked_edits(&doc, &id, &[edit(1, 1, "")], &BOB, None, 2).unwrap(),
                &a,
            );
            doc.commit();
            assert_eq!(body(&doc, &id), "ad");
            let order = if bob_first { [&b, &a] } else { [&a, &b] };
            for change in order {
                assert!(reject(&doc, change));
                doc.commit();
            }
            assert_eq!(body(&doc, &id), "abcd");
            assert!(resolve_changes(&doc, &id).is_empty());
        }
    }

    #[test]
    fn a_deletion_inside_a_deletion_restores_in_either_order() {
        for bob_first in [true, false] {
            let (doc, id) = setup("abcde");
            let a =
                apply_tracked_edits(&doc, &id, &[edit(2, 1, "")], &ME, None, 1).unwrap()[0].clone();
            doc.commit();
            assert_eq!(body(&doc, &id), "abde");
            let b = bobs(
                apply_tracked_edits(&doc, &id, &[edit(1, 2, "")], &BOB, None, 2).unwrap(),
                &a,
            );
            doc.commit();
            assert_eq!(body(&doc, &id), "ae");
            let order = if bob_first { [&b, &a] } else { [&a, &b] };
            for change in order {
                assert!(reject(&doc, change));
                doc.commit();
            }
            assert_eq!(body(&doc, &id), "abcde");
            assert!(resolve_changes(&doc, &id).is_empty());
        }
    }

    #[test]
    fn reject_many_restores_in_either_order() {
        for bob_first in [true, false] {
            let (doc, id, a, b) = swallow("abcde", edit(2, 1, ""), edit(1, 2, "Z"));
            assert_eq!(body(&doc, &id), "aZe");
            let ids = if bob_first {
                [b.clone(), a.clone()]
            } else {
                [a.clone(), b.clone()]
            };
            assert_eq!(reject_many(&doc, &ids), 2);
            doc.commit();
            assert_eq!(body(&doc, &id), "abcde");
            assert!(resolve_changes(&doc, &id).is_empty());
        }
    }

    /// A start text and its edits as (from, len, insert). Edit i is made by
    /// `author_for(i)`.
    type Shape = (&'static str, &'static [(usize, usize, &'static str)]);

    /// Nested deletions.
    const SHAPES: &[Shape] = &[
        ("abcde", &[(2, 1, ""), (1, 2, ""), (0, 1, "")]),
        ("abcde", &[(2, 1, "X"), (1, 3, ""), (0, 1, "")]),
        ("abcdefg", &[(3, 1, ""), (2, 3, ""), (1, 2, ""), (0, 1, "")]),
        (
            "abcdefgh",
            &[(3, 1, ""), (2, 2, ""), (1, 2, ""), (0, 2, "")],
        ),
        ("abcdefgh", &[(3, 1, ""), (2, 2, ""), (0, 5, "")]),
        ("abcdefg", &[(1, 1, ""), (4, 1, ""), (0, 5, "")]),
        (
            "abcdefgh",
            &[(1, 1, ""), (4, 1, ""), (0, 5, ""), (0, 1, "")],
        ),
    ];

    #[test]
    fn nested_deletions_restore_in_any_order() {
        for (start, edits) in SHAPES {
            let (probe, pid) = build_shape(start, edits);
            let whos: Vec<String> = resolve_changes(&probe, &pid)
                .into_iter()
                .map(|c| c.who)
                .collect();
            assert!(!whos.is_empty(), "{start} {edits:?}");
            for order in permutations(&whos) {
                // One at a time.
                let (doc, id) = build_shape(start, edits);
                let ids = who_ids(&doc, &id);
                for w in &order {
                    let cid = ids.get(w).unwrap().clone();
                    assert!(reject(&doc, &cid), "{start} {edits:?} {order:?}");
                    doc.commit();
                }
                assert_eq!(body(&doc, &id), *start, "{start} {edits:?} {order:?}");
                assert!(resolve_changes(&doc, &id).is_empty());

                // All at once.
                let (doc, id) = build_shape(start, edits);
                let ids = who_ids(&doc, &id);
                let list: Vec<String> = order.iter().map(|w| ids.get(w).unwrap().clone()).collect();
                assert_eq!(reject_many(&doc, &list), list.len());
                doc.commit();
                assert_eq!(body(&doc, &id), *start, "many: {start} {edits:?} {order:?}");
                assert!(resolve_changes(&doc, &id).is_empty());
            }
        }
    }

    #[test]
    fn accepting_the_middle_change_and_rejecting_the_others_gives_ace() {
        let (doc, id) = build_shape("abcde", SHAPES[0].1);
        let rows = resolve_changes(&doc, &id);
        for c in rows.iter().filter(|c| c.who == "bob") {
            assert!(accept(&doc, &c.id));
        }
        for c in rows.iter().filter(|c| c.who != "bob") {
            assert!(reject(&doc, &c.id));
        }
        doc.commit();
        assert_eq!(body(&doc, &id), "ace");
        assert!(resolve_changes(&doc, &id).is_empty());
    }

    /// Rejects every live change of the file that `keep` admits.
    fn reject_where(doc: &LoroDoc, id: &str, keep: impl Fn(&ResolvedChange) -> bool) {
        let ids: Vec<String> = resolve_changes(doc, id)
            .into_iter()
            .filter(|c| keep(c))
            .map(|c| c.id)
            .collect();
        for cid in &ids {
            assert!(reject(doc, cid), "{cid} is not a change");
        }
    }

    #[test]
    fn rejecting_a_restored_fragment_before_a_later_edit() {
        // Alice deletes c. Bob replaces b and d with Z, and Alice is rejected,
        // so c comes back in front of Z. Bob then inserts before c, and every
        // change of Bob is rejected.
        let (doc, id) = setup("abcde");
        apply_tracked_edits(&doc, &id, &[edit(2, 1, "")], &who_of("alice"), None, 1).unwrap();
        doc.commit();
        apply_tracked_edits(&doc, &id, &[edit(1, 2, "Z")], &who_of("bob"), None, 2).unwrap();
        doc.commit();
        assert_eq!(body(&doc, &id), "aZe");
        reject_where(&doc, &id, |c| c.who == "alice");
        doc.commit();
        assert_eq!(body(&doc, &id), "acZe");
        apply_tracked_edits(&doc, &id, &[edit(1, 0, "Q")], &who_of("bob"), None, 3).unwrap();
        doc.commit();
        assert_eq!(body(&doc, &id), "aQcZe");
        reject_where(&doc, &id, |c| c.who == "bob");
        doc.commit();
        assert_eq!(body(&doc, &id), "abcde");
    }

    #[test]
    fn rejecting_an_author_after_a_merge_across_a_restored_fragment() {
        let (doc, id) = setup("abcdef");
        apply_tracked_edits(&doc, &id, &[edit(0, 1, "")], &who_of("m"), None, 1).unwrap();
        doc.commit();
        apply_tracked_edits(&doc, &id, &[edit(2, 1, "")], &who_of("n"), None, 2).unwrap();
        doc.commit();
        apply_tracked_edits(&doc, &id, &[edit(1, 2, "")], &who_of("m"), None, 3).unwrap();
        doc.commit();
        assert_eq!(body(&doc, &id), "bf");
        reject_where(&doc, &id, |c| c.who == "n");
        doc.commit();
        assert_eq!(body(&doc, &id), "bdf");
        apply_tracked_edits(&doc, &id, &[edit(0, 1, "")], &who_of("m"), None, 4).unwrap();
        doc.commit();
        assert_eq!(body(&doc, &id), "df");
        reject_where(&doc, &id, |c| c.who == "m");
        doc.commit();
        assert_eq!(body(&doc, &id), "abcdef");
    }

    #[test]
    fn rejecting_everything_after_an_untracked_collapse_of_two_gaps() {
        let (doc, id) = setup("abcde");
        apply_tracked_edits(&doc, &id, &[edit(1, 1, "")], &who_of("p"), None, 1).unwrap();
        doc.commit();
        apply_tracked_edits(&doc, &id, &[edit(2, 1, "")], &who_of("q"), None, 2).unwrap();
        doc.commit();
        assert_eq!(body(&doc, &id), "ace");
        untracked_edits(&doc, &id, &[edit(1, 1, "")]).unwrap();
        prune(&doc);
        doc.commit();
        assert_eq!(body(&doc, &id), "ae");
        apply_tracked_edits(&doc, &id, &[edit(1, 0, "X")], &who_of("r"), None, 3).unwrap();
        doc.commit();
        assert_eq!(body(&doc, &id), "aXe");
        reject_where(&doc, &id, |_| true);
        doc.commit();
        assert_eq!(body(&doc, &id), "abde");
    }

    #[test]
    fn seeded_fuzz_pure_deletions() {
        // Random pure deletions by random authors. Oracle: in any decision
        // order, an original character survives iff its deleter was rejected,
        // and no record is left behind.
        for seed in [7, 11] {
            let mut rng = Lcg::new(seed);
            for iter in 0..150 {
                let start: String = ALPHA[..5 + rng.pick(8)].to_string();
                let (doc, id) = setup(&start);
                let mut cur: Vec<char> = start.chars().collect();
                let mut steps: Vec<(usize, usize, &str)> = Vec::new();
                let n = 2 + rng.pick(4);
                let mut i = 0;
                while i < n && !cur.is_empty() {
                    let from = rng.pick(cur.len());
                    let len = 1 + rng.pick((cur.len() - from).min(4));
                    let w = AUTHORS[rng.pick(4)];
                    steps.push((from, from + len, w));
                    apply_tracked_edits(&doc, &id, &[edit(from, len, "")], &who_of(w), None, 1)
                        .unwrap();
                    doc.commit();
                    cur.drain(from..from + len);
                    i += 1;
                }
                let rows = resolve_changes(&doc, &id);
                let mut decisions: Vec<(String, bool, String)> = Vec::new();
                for r in &rows {
                    let accept_it = rng.rnd() < 0.5;
                    decisions.push((r.id.clone(), accept_it, r.deleted.clone()));
                }
                let gone: String = decisions
                    .iter()
                    .filter(|d| d.1)
                    .map(|d| d.2.as_str())
                    .collect();
                let expected: String = start.chars().filter(|c| !gone.contains(*c)).collect();
                rng.shuffle(&mut decisions);
                let label = format!(
                    "seed={seed} iter={iter} start={start} steps={steps:?} decisions={decisions:?}"
                );
                for (cid, accept_it, _) in &decisions {
                    if doc.get_map(CHANGES).get(cid).is_none() {
                        continue;
                    }
                    if *accept_it {
                        assert!(accept(&doc, cid), "{label}");
                    } else {
                        assert!(reject(&doc, cid), "{label}");
                    }
                    doc.commit();
                }
                assert_eq!(body(&doc, &id), expected, "{label}");
                assert!(resolve_changes(&doc, &id).is_empty(), "{label}");
            }
        }
    }

    #[test]
    fn seeded_fuzz_with_insertions() {
        // Random deletions, insertions and replacements by random authors.
        // Oracle: an original character is gone iff its first deleter was
        // accepted, an inserted character is kept iff its author was accepted,
        // and the original characters keep their order.
        for seed in [7u64, 11] {
            let mut rng = Lcg::new(seed);
            let mut insert_counter: u32 = 0;
            for iter in 0..150 {
                let start: String = ALPHA[..5 + rng.pick(8)].to_string();
                let (doc, id) = setup(&start);
                // Each character with the index of the author who inserted it.
                let mut model: Vec<(char, Option<usize>)> =
                    start.chars().map(|c| (c, None)).collect();
                // Original character to the index of the author who deleted it first.
                let mut deleted_by: HashMap<char, usize> = HashMap::new();
                let mut steps: Vec<(usize, usize, String, &str)> = Vec::new();
                let n = 2 + rng.pick(5);
                for _ in 0..n {
                    let w = rng.pick(4);
                    let from = rng.pick(model.len() + 1);
                    let len = rng.pick((model.len() - from).min(4) + 1);
                    let mut ins = String::new();
                    if len == 0 || rng.rnd() < 0.4 {
                        let k = 1 + rng.pick(2);
                        for _ in 0..k {
                            ins.push(char::from_u32(0x391 + insert_counter % 24).unwrap());
                            insert_counter += 1;
                        }
                    }
                    if len == 0 && ins.is_empty() {
                        continue;
                    }
                    steps.push((from, from + len, ins.clone(), AUTHORS[w]));
                    apply_tracked_edits(
                        &doc,
                        &id,
                        &[edit(from, len, &ins)],
                        &who_of(AUTHORS[w]),
                        None,
                        1,
                    )
                    .unwrap();
                    doc.commit();
                    for (ch, by) in &model[from..from + len] {
                        if by.is_none() && !deleted_by.contains_key(ch) {
                            deleted_by.insert(*ch, w);
                        }
                    }
                    let inserted: Vec<(char, Option<usize>)> =
                        ins.chars().map(|c| (c, Some(w))).collect();
                    let tail = model.split_off(from + len);
                    model.truncate(from);
                    model.extend(inserted);
                    model.extend(tail);
                    let text: String = model.iter().map(|(c, _)| *c).collect();
                    assert_eq!(
                        body(&doc, &id),
                        text,
                        "seed={seed} iter={iter} start={start} steps={steps:?}"
                    );
                }

                let mut verdict = [false; 4];
                for v in verdict.iter_mut() {
                    *v = rng.rnd() < 0.5;
                }
                let label = format!(
                    "seed={seed} iter={iter} start={start} steps={steps:?} accept={verdict:?}"
                );
                let rows = resolve_changes(&doc, &id);
                let mut order = rows.clone();
                rng.shuffle(&mut order);
                for r in &order {
                    if doc.get_map(CHANGES).get(&r.id).is_none() {
                        continue;
                    }
                    let idx = AUTHORS.iter().position(|a| *a == r.who).unwrap();
                    if verdict[idx] {
                        assert!(accept(&doc, &r.id), "{label}");
                    } else {
                        assert!(reject(&doc, &r.id), "{label}");
                    }
                    doc.commit();
                }

                let expected_original: Vec<char> = start
                    .chars()
                    .filter(|ch| match deleted_by.get(ch) {
                        None => true,
                        Some(&w) => !verdict[w],
                    })
                    .collect();
                let mut expected_inserted: Vec<char> = model
                    .iter()
                    .filter_map(|(c, by)| match by {
                        Some(w) if verdict[*w] => Some(*c),
                        _ => None,
                    })
                    .collect();
                expected_inserted.sort();
                let final_chars: Vec<char> = body(&doc, &id).chars().collect();
                let final_original: Vec<char> = final_chars
                    .iter()
                    .copied()
                    .filter(|c| (*c as u32) < 0x300)
                    .collect();
                let mut final_inserted: Vec<char> = final_chars
                    .iter()
                    .copied()
                    .filter(|c| (*c as u32) >= 0x300)
                    .collect();
                final_inserted.sort();
                assert_eq!(final_original, expected_original, "{label}");
                assert_eq!(final_inserted, expected_inserted, "{label}");
                assert!(resolve_changes(&doc, &id).is_empty(), "{label}");
            }
        }
    }

    /// The oracle's state of one character in the interleaved fuzz.
    #[derive(Clone, Copy, PartialEq, Debug)]
    enum Mark {
        /// In the text, or back in it after a rejection.
        Live,
        /// Deleted by a tracked change of the author with this index.
        Deleted(usize),
        /// A tracked insertion nobody has decided yet.
        Pending,
        /// Gone for good.
        Gone,
    }

    struct CharState {
        /// Original or untracked text, with no pending change of its own.
        plain: bool,
        /// The author index of a tracked insertion.
        by: usize,
        state: Mark,
    }

    /// Accepts or rejects every live change of author `w` by its verdict, and
    /// moves the oracle's characters to match.
    fn decide_author(
        doc: &LoroDoc,
        id: &str,
        chars: &mut HashMap<char, CharState>,
        verdict: &[bool; 4],
        w: usize,
    ) {
        let ids: Vec<String> = resolve_changes(doc, id)
            .into_iter()
            .filter(|c| c.who == AUTHORS[w])
            .map(|c| c.id)
            .collect();
        for cid in &ids {
            if verdict[w] {
                assert!(accept(doc, cid), "{cid} is not a change");
            } else {
                assert!(reject(doc, cid), "{cid} is not a change");
            }
        }
        doc.commit();
        for c in chars.values_mut() {
            if c.plain && c.state == Mark::Deleted(w) {
                c.state = if verdict[w] { Mark::Gone } else { Mark::Live };
            } else if !c.plain && c.by == w && c.state == Mark::Pending {
                if verdict[w] {
                    c.plain = true;
                    c.state = Mark::Live;
                } else {
                    c.state = Mark::Gone;
                }
            }
        }
    }

    #[test]
    fn seeded_fuzz_interleaved() {
        // Tracked edits, untracked edits and per-author decisions, with authors
        // editing again after being decided. Every character is unique, so one
        // state per character is the oracle: a plain character is live, deleted
        // by a tracked change of its author, or gone; a tracked insertion is
        // pending until its author decides it, and vanishes if it is deleted
        // first. Original characters keep their order.
        let mut rng = Lcg::new(23);
        let mut insert_counter: u32 = 0;
        for iter in 0..120 {
            let start: String = ALPHA[..5 + rng.pick(8)].to_string();
            let (doc, id) = setup(&start);
            let mut chars: HashMap<char, CharState> = start
                .chars()
                .map(|c| {
                    (
                        c,
                        CharState {
                            plain: true,
                            by: 0,
                            state: Mark::Live,
                        },
                    )
                })
                .collect();
            let mut verdict = [false; 4];
            for v in verdict.iter_mut() {
                *v = rng.rnd() < 0.5;
            }
            let mut log: Vec<String> = Vec::new();
            let steps = 3 + rng.pick(6);
            for _ in 0..steps {
                let kind = rng.rnd();
                if kind < 0.2 {
                    let w = rng.pick(4);
                    decide_author(&doc, &id, &mut chars, &verdict, w);
                    let word = if verdict[w] { "accept" } else { "reject" };
                    log.push(format!("decide {}={word}", AUTHORS[w]));
                    continue;
                }
                let text: Vec<char> = body(&doc, &id).chars().collect();
                let from = rng.pick(text.len() + 1);
                let len = rng.pick((text.len() - from).min(4) + 1);
                let ins: Option<char> = if len == 0 || rng.rnd() < 0.3 {
                    let ch = char::from_u32(0x391 + insert_counter % 24).unwrap();
                    insert_counter += 1;
                    Some(ch)
                } else {
                    None
                };
                if len == 0 && ins.is_none() {
                    continue;
                }
                let ins_text = ins.map_or(String::new(), |ch| ch.to_string());
                let untracked = kind > 0.85;
                let w = if untracked { None } else { Some(rng.pick(4)) };
                let label = w.map_or("untracked", |w| AUTHORS[w]);
                log.push(format!("{label} {from}-{} '{ins_text}'", from + len));
                for ch in &text[from..from + len] {
                    let c = chars.get_mut(ch).expect("every text character is tracked");
                    c.state = if c.plain {
                        match w {
                            Some(w) => Mark::Deleted(w),
                            None => Mark::Gone,
                        }
                    } else {
                        Mark::Gone
                    };
                }
                if let (Some(ch), Some(w)) = (ins, w) {
                    chars.insert(
                        ch,
                        CharState {
                            plain: false,
                            by: w,
                            state: Mark::Pending,
                        },
                    );
                } else if let Some(ch) = ins {
                    chars.insert(
                        ch,
                        CharState {
                            plain: true,
                            by: 0,
                            state: Mark::Live,
                        },
                    );
                }
                match w {
                    None => {
                        untracked_edits(&doc, &id, &[edit(from, len, &ins_text)]).unwrap_or_else(
                            |e| panic!("iter={iter} start={start} log={log:?}: {e}"),
                        );
                        prune(&doc);
                    }
                    Some(w) => {
                        apply_tracked_edits(
                            &doc,
                            &id,
                            &[edit(from, len, &ins_text)],
                            &who_of(AUTHORS[w]),
                            None,
                            1,
                        )
                        .unwrap_or_else(|e| panic!("iter={iter} start={start} log={log:?}: {e}"));
                    }
                }
                doc.commit();
            }

            for w in 0..4 {
                decide_author(&doc, &id, &mut chars, &verdict, w);
            }
            let final_text = body(&doc, &id);
            let mut expected: Vec<char> = chars
                .iter()
                .filter(|(_, c)| c.state == Mark::Live)
                .map(|(ch, _)| *ch)
                .collect();
            expected.sort();
            let mut got: Vec<char> = final_text.chars().collect();
            got.sort();
            let originals: String = final_text
                .chars()
                .filter(|ch| start.contains(*ch))
                .collect();
            let want_originals: String = start
                .chars()
                .filter(|ch| chars[ch].state == Mark::Live)
                .collect();
            let left = resolve_changes(&doc, &id).len();
            let label = format!(
                "iter={iter} start={start} verdict={verdict:?} log={log:?} -> {final_text:?} expected={expected:?}"
            );
            assert_eq!(got, expected, "{label}");
            assert_eq!(originals, want_originals, "{label}");
            assert_eq!(left, 0, "{label}");
        }
    }

    /// An empty change record, as a peer's session leaves one: no span and no
    /// fragment, stamped `at`.
    fn empty_record(doc: &LoroDoc, key: &str, file: &str, at: f64) {
        let m = doc
            .get_map(CHANGES)
            .insert_container(key, LoroMap::new())
            .unwrap();
        m.insert("kind", LoroValue::from("change")).unwrap();
        m.insert("file", LoroValue::from(file)).unwrap();
        m.insert("who", LoroValue::from("me")).unwrap();
        m.insert("author", LoroValue::from("Me")).unwrap();
        m.insert("at", LoroValue::Double(at)).unwrap();
        m.insert("run", LoroValue::Null).unwrap();
        m.insert("start", LoroValue::Null).unwrap();
        m.insert("end", LoroValue::Null).unwrap();
    }

    /// Exchanges the updates each peer is missing. Both sides are committed.
    fn sync(a: &LoroDoc, b: &LoroDoc) {
        let to_a = b.export(loro::ExportMode::updates(&a.oplog_vv())).unwrap();
        let to_b = a.export(loro::ExportMode::updates(&b.oplog_vv())).unwrap();
        a.import(&to_a).unwrap();
        b.import(&to_b).unwrap();
    }

    /// The rows of a file as `who:deleted+width`, sorted, to compare two peers.
    fn row_summary(doc: &LoroDoc, id: &str) -> Vec<String> {
        let mut rows: Vec<String> = resolve_changes(doc, id)
            .iter()
            .map(|c| format!("{}:{}+{}", c.who, c.deleted, c.to - c.from))
            .collect();
        rows.sort();
        rows
    }

    /// The next unique inserted character, as the JS fuzzers write them.
    fn next_char(n: &mut u32) -> String {
        let ch = char::from_u32(0x391 + *n % 24).unwrap();
        *n += 1;
        ch.to_string()
    }

    #[test]
    fn two_peers_lose_nothing() {
        // Two peers edit one text at the same moment, each in its own region,
        // then exchange updates. They must converge after every round, and
        // rejecting every change at the end must restore the start text up to
        // order.
        let mut rng = Lcg::new(31);
        let mut ins: u32 = 0;
        let cases = 120;
        let mut misordered = 0;
        for iter in 0..cases {
            let start: String = ALPHA[..7 + rng.pick(6)].to_string();
            let (a, id) = setup(&start);
            let b = new_doc();
            b.import(&a.export(loro::ExportMode::snapshot()).unwrap())
                .unwrap();
            let mut log: Vec<String> = Vec::new();
            let mut round = 0;
            while round < 2 + rng.pick(3) {
                round += 1;
                let text: Vec<char> = body(&a, &id).chars().collect();
                if text.len() < 4 {
                    break;
                }
                // Two regions with at least one character between them.
                let cut = 2 + rng.pick(text.len() - 3);
                let f1 = rng.pick(cut);
                let t1 = f1 + rng.pick((cut - f1).min(3) + 1);
                let f2 = cut + 1 + rng.pick((text.len() - cut - 1).max(1));
                let t2 = (f2 + rng.pick(3)).min(text.len());
                let i1 = if t1 == f1 || rng.rnd() < 0.3 {
                    next_char(&mut ins)
                } else {
                    String::new()
                };
                let i2 = if t2 == f2 || rng.rnd() < 0.3 {
                    next_char(&mut ins)
                } else {
                    String::new()
                };
                if f1 == t1 && i1.is_empty() {
                    continue;
                }
                log.push(format!("A {f2}-{t2} '{i2}' | B {f1}-{t1} '{i1}'"));
                // A edits the right region and B the left one, both against the
                // same text.
                if t2 > f2 || !i2.is_empty() {
                    apply_tracked_edits(
                        &a,
                        &id,
                        &[edit(f2, t2 - f2, &i2)],
                        &who_of("alice"),
                        None,
                        1,
                    )
                    .unwrap();
                    a.commit();
                }
                apply_tracked_edits(&b, &id, &[edit(f1, t1 - f1, &i1)], &who_of("bob"), None, 1)
                    .unwrap();
                b.commit();
                sync(&a, &b);
                assert_eq!(
                    body(&a, &id),
                    body(&b, &id),
                    "diverged: iter={iter} start={start} log={log:?}"
                );
                assert_eq!(
                    row_summary(&a, &id),
                    row_summary(&b, &id),
                    "rows: iter={iter} start={start} log={log:?}"
                );
            }

            let ids: Vec<String> = resolve_changes(&a, &id).into_iter().map(|c| c.id).collect();
            for cid in &ids {
                assert!(reject(&a, cid));
                a.commit();
            }
            sync(&a, &b);
            let mut want: Vec<char> = start.chars().collect();
            want.sort();
            let mut got: Vec<char> = body(&a, &id).chars().collect();
            got.sort();
            let label = format!("iter={iter} start={start} log={log:?}");
            assert_eq!(got, want, "{label}");
            assert_eq!(body(&a, &id), body(&b, &id), "{label}");
            assert!(resolve_changes(&a, &id).is_empty(), "{label}");
            if body(&a, &id) != start {
                misordered += 1;
            }
        }
        assert!(
            misordered * 4 < cases,
            "order differed in {misordered} of {cases} cases"
        );
    }

    #[test]
    fn a_span_shrunk_by_one_peer_and_grown_by_its_author_keeps_both() {
        let (a, id) = setup("abcd");
        apply_tracked_edits(&a, &id, &[edit(2, 0, "XY")], &who_of("alice"), None, 1).unwrap();
        a.commit();
        let b = new_doc();
        b.import(&a.export(loro::ExportMode::snapshot()).unwrap())
            .unwrap();
        // Bob deletes the X, which is alice's own inserted text, so the span
        // only shrinks and nothing is recorded for him.
        apply_tracked_edits(&a, &id, &[edit(2, 1, "")], &who_of("bob"), None, 2).unwrap();
        a.commit();
        // Alice types Z after the Y on the other peer, which extends her span.
        apply_tracked_edits(&b, &id, &[edit(4, 0, "Z")], &who_of("alice"), None, 3).unwrap();
        b.commit();
        sync(&a, &b);
        assert_eq!(body(&a, &id), "abYZcd");
        assert_eq!(body(&b, &id), "abYZcd");
        for doc in [&a, &b] {
            let rows = resolve_changes(doc, &id);
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].who, "alice");
            let text: Vec<char> = body(doc, &id).chars().collect();
            let span: String = text[rows[0].from..rows[0].to].iter().collect();
            assert_eq!(span, "YZ");
        }
        let alice = resolve_changes(&a, &id)[0].id.clone();
        assert!(reject(&a, &alice));
        a.commit();
        assert_eq!(body(&a, &id), "abcd");
    }

    #[test]
    fn settling_keeps_a_fresh_empty_record_and_drops_an_old_one() {
        let (doc, id) = setup("abc");
        empty_record(&doc, "fresh", &id, now_ms() as f64);
        empty_record(&doc, "old", &id, 1.0);
        // The old one was first seen empty long ago.
        let root = doc.get_map(CHANGES);
        record_map(&root, "old")
            .unwrap()
            .insert("emptied", LoroValue::Double(1.0))
            .unwrap();
        doc.commit();
        assert_eq!(prune(&doc), 1);
        doc.commit();
        assert!(doc.get_map(CHANGES).get("fresh").is_some());
        assert!(doc.get_map(CHANGES).get("old").is_none());
        assert!(resolve_changes(&doc, &id).is_empty());
    }

    #[test]
    fn settling_counts_from_the_first_prune_that_saw_the_record_empty() {
        // The first prune that finds an empty record marks it and keeps it. It
        // drops it only once that mark is old.
        let (doc, id) = setup("abc");
        empty_record(&doc, "empty", &id, 1.0);
        doc.commit();
        assert_eq!(prune(&doc), 0);
        doc.commit();
        let root = doc.get_map(CHANGES);
        let m = record_map(&root, "empty").expect("kept by the first prune");
        assert!(num_of(&m, "emptied").is_some());
        m.insert("emptied", LoroValue::Double(0.0)).unwrap();
        doc.commit();
        assert_eq!(prune(&doc), 1);
        doc.commit();
        assert!(root.get("empty").is_none());
        assert!(resolve_changes(&doc, &id).is_empty());
    }

    #[test]
    fn an_own_insertion_deleted_whole_while_another_session_extends_it() {
        // The author's session deletes her own insertion and prunes, while her
        // other session extends it. The extension survives, and rejecting it
        // restores the text.
        let (a, id) = setup("ab");
        let p = who_of("p");
        apply_tracked_edits(&a, &id, &[edit(1, 0, "XY")], &p, None, 1).unwrap();
        a.commit();
        let b = new_doc();
        b.import(&a.export(loro::ExportMode::snapshot()).unwrap())
            .unwrap();
        apply_tracked_edits(&a, &id, &[edit(1, 2, "")], &p, None, 2).unwrap();
        a.commit();
        prune(&a);
        a.commit();
        apply_tracked_edits(&b, &id, &[edit(3, 0, "Z")], &p, None, 3).unwrap();
        b.commit();
        sync(&a, &b);
        assert_eq!(body(&a, &id), body(&b, &id));
        let rows = resolve_changes(&a, &id);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].to - rows[0].from, 1);
        reject_where(&a, &id, |_| true);
        a.commit();
        assert_eq!(body(&a, &id), "ab");
    }

    #[test]
    fn an_old_insertion_deleted_by_a_peer_while_its_author_extends_it() {
        // The insertion is old enough to be healed by age. A peer deletes it and
        // prunes at once, while its author extends it on another session. The
        // extension survives, and rejecting it restores the text.
        let (a, id) = setup("ab");
        let p = who_of("p");
        apply_tracked_edits(&a, &id, &[edit(1, 0, "XY")], &p, None, 1).unwrap();
        a.commit();
        let root = a.get_map(CHANGES);
        let keys: Vec<String> = root.keys().map(|k| k.to_string()).collect();
        for k in &keys {
            if let Some(m) = record_map(&root, k) {
                m.insert("at", LoroValue::Double(0.0)).unwrap();
            }
        }
        a.commit();
        let b = new_doc();
        b.import(&a.export(loro::ExportMode::snapshot()).unwrap())
            .unwrap();
        apply_tracked_edits(&a, &id, &[edit(1, 2, "")], &who_of("q"), None, 2).unwrap();
        a.commit();
        prune(&a);
        a.commit();
        apply_tracked_edits(&b, &id, &[edit(3, 0, "Z")], &p, None, 3).unwrap();
        b.commit();
        sync(&a, &b);
        assert_eq!(row_summary(&a, &id), ["p:+1"]);
        assert_eq!(row_summary(&b, &id), ["p:+1"]);
        reject_where(&a, &id, |_| true);
        a.commit();
        assert_eq!(body(&a, &id), "ab");
    }

    #[test]
    fn extending_a_deletion_while_a_reviewer_accepts_it() {
        let (a, id) = setup("abcde");
        let p = who_of("p");
        apply_tracked_edits(&a, &id, &[edit(1, 1, "")], &p, None, 1).unwrap();
        a.commit();
        let b = new_doc();
        b.import(&a.export(loro::ExportMode::snapshot()).unwrap())
            .unwrap();
        // The author deletes c on this session, extending the deletion of b.
        apply_tracked_edits(&a, &id, &[edit(1, 1, "")], &p, None, 2).unwrap();
        a.commit();
        // A reviewer accepts the change to b on the other session.
        let first = resolve_changes(&b, &id)[0].id.clone();
        assert!(accept(&b, &first));
        b.commit();
        sync(&a, &b);
        prune(&a);
        a.commit();
        sync(&a, &b);
        assert_eq!(body(&a, &id), body(&b, &id));
        let rows = resolve_changes(&a, &id);
        let deleted: Vec<&str> = rows.iter().map(|c| c.deleted.as_str()).collect();
        assert_eq!(deleted, ["c"]);
        reject_where(&a, &id, |_| true);
        a.commit();
        assert_eq!(body(&a, &id), "acde");
    }

    #[test]
    fn extending_a_deletion_while_a_reviewer_rejects_it() {
        let (a, id) = setup("abcde");
        let p = who_of("p");
        apply_tracked_edits(&a, &id, &[edit(1, 1, "")], &p, None, 1).unwrap();
        a.commit();
        let b = new_doc();
        b.import(&a.export(loro::ExportMode::snapshot()).unwrap())
            .unwrap();
        apply_tracked_edits(&a, &id, &[edit(1, 1, "")], &p, None, 2).unwrap();
        a.commit();
        let first = resolve_changes(&b, &id)[0].id.clone();
        assert!(reject(&b, &first));
        b.commit();
        sync(&a, &b);
        prune(&a);
        a.commit();
        sync(&a, &b);
        assert_eq!(body(&a, &id), body(&b, &id));
        reject_where(&a, &id, |_| true);
        a.commit();
        assert_eq!(body(&a, &id), "abcde");
    }

    /// Stores a fragment "X" of bob's at offset 1 of `file`, whose change record
    /// is gone: only its author fields remain.
    fn orphan_fragment(doc: &LoroDoc, file: &str) {
        let text = text_of(doc, file).unwrap();
        let cursor = text.get_cursor(1, Side::Left).unwrap().encode();
        let frag = doc
            .get_map(CHANGES)
            .insert_container("frag", LoroMap::new())
            .unwrap();
        frag.insert("kind", LoroValue::from("fragment")).unwrap();
        frag.insert("file", LoroValue::from(file)).unwrap();
        frag.insert("change", LoroValue::from("gone")).unwrap();
        frag.insert("text", LoroValue::from("X")).unwrap();
        frag.insert("start", LoroValue::from(cursor)).unwrap();
        frag.insert("rank", LoroValue::Double(0.0)).unwrap();
        frag.insert("who", LoroValue::from("bob")).unwrap();
        frag.insert("author", LoroValue::from("Bob")).unwrap();
        frag.insert("run", LoroValue::Null).unwrap();
        frag.insert("at", LoroValue::Double(1.0)).unwrap();
        doc.commit();
    }

    #[test]
    fn an_orphan_fragment_is_read_as_its_own_change() {
        // A fragment whose change was decided while it was written. Its change
        // record is gone, so it reads as a change of its own, with no prune.
        let (doc, id) = setup("abc");
        orphan_fragment(&doc, &id);
        let rows = resolve_changes(&doc, &id);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "frag");
        assert_eq!(
            (
                rows[0].who.as_str(),
                rows[0].author.as_str(),
                rows[0].deleted.as_str()
            ),
            ("bob", "Bob", "X")
        );
        assert_eq!((rows[0].from, rows[0].to), (1, 1));
        // Prune leaves the live record alone: it is not marked empty.
        assert_eq!(prune(&doc), 0);
        doc.commit();
        let root = doc.get_map(CHANGES);
        assert!(record_map(&root, "frag").unwrap().get("emptied").is_none());
        assert!(reject(&doc, "frag"));
        doc.commit();
        assert_eq!(body(&doc, &id), "aXbc");
        assert!(root.keys().next().is_none());
    }

    #[test]
    fn an_orphan_fragment_accepted_keeps_its_text() {
        let (doc, id) = setup("abc");
        orphan_fragment(&doc, &id);
        assert!(accept(&doc, "frag"));
        doc.commit();
        assert_eq!(body(&doc, &id), "abc");
        assert!(resolve_changes(&doc, &id).is_empty());
        assert!(doc.get_map(CHANGES).keys().next().is_none());
    }

    #[test]
    fn an_orphan_fragment_of_mine_is_not_joined() {
        // The author replaces text right before a fragment of theirs that is
        // read as a change of its own. It is not joined into the new change,
        // since it has no record to merge into: the new span is tracked, both
        // deletions stay pending, nothing is written over the fragment record,
        // and rejecting everything restores the text.
        let (doc, id) = setup("abc");
        orphan_fragment(&doc, &id);
        apply_tracked_edits(&doc, &id, &[edit(0, 1, "Q")], &who_of("bob"), None, 2).unwrap();
        doc.commit();
        assert_eq!(body(&doc, &id), "Qbc");
        let text: Vec<char> = body(&doc, &id).chars().collect();
        let mut rows: Vec<(String, String)> = resolve_changes(&doc, &id)
            .iter()
            .map(|c| (text[c.from..c.to].iter().collect(), c.deleted.clone()))
            .collect();
        rows.sort();
        assert_eq!(
            rows,
            vec![
                (String::new(), "X".to_string()),
                ("Q".to_string(), "a".to_string())
            ]
        );
        let mut kinds: Vec<String> = records(&doc)
            .iter()
            .filter_map(|(_, m)| str_of(m, "kind"))
            .collect();
        kinds.sort();
        assert_eq!(kinds, ["change", "fragment", "fragment"]);
        for c in resolve_changes(&doc, &id) {
            assert!(reject(&doc, &c.id));
        }
        doc.commit();
        assert_eq!(body(&doc, &id), "aXbc");
    }

    #[test]
    fn two_own_spans_around_a_deletion_stay_two_changes() {
        // X at 0 and Y at 2 of "ab" are two insertions by one author. Deleting
        // the "a" between them does not merge them: the first span takes the
        // deletion, the second keeps its record, and each is decided alone.
        let (a, id) = setup("ab");
        let p = who_of("p");
        apply_tracked_edits(&a, &id, &[edit(0, 0, "X")], &p, None, 1).unwrap();
        a.commit();
        apply_tracked_edits(&a, &id, &[edit(2, 0, "Y")], &p, None, 1).unwrap();
        a.commit();
        apply_tracked_edits(&a, &id, &[edit(1, 1, "")], &p, None, 2).unwrap();
        a.commit();
        assert_eq!(body(&a, &id), "XYb");
        let text: Vec<char> = body(&a, &id).chars().collect();
        let mut rows: Vec<(String, String)> = resolve_changes(&a, &id)
            .iter()
            .map(|c| (text[c.from..c.to].iter().collect(), c.deleted.clone()))
            .collect();
        rows.sort();
        assert_eq!(
            rows,
            vec![
                ("X".to_string(), "a".to_string()),
                ("Y".to_string(), String::new())
            ]
        );
        let b = new_doc();
        b.import(&a.export(loro::ExportMode::snapshot()).unwrap())
            .unwrap();
        let rows = resolve_changes(&a, &id);
        let first = rows.iter().find(|c| c.deleted == "a").unwrap().id.clone();
        let second = rows
            .iter()
            .find(|c| c.deleted.is_empty())
            .unwrap()
            .id
            .clone();
        assert!(accept(&a, &first));
        assert!(reject(&a, &second));
        a.commit();
        assert_eq!(body(&a, &id), "Xb");
        assert!(reject(&b, &first));
        assert!(reject(&b, &second));
        b.commit();
        assert_eq!(body(&b, &id), "ab");
    }

    #[test]
    fn a_merge_of_own_changes_keeps_a_peer_session_extension() {
        // Session a has two own insertions, and session b, which has not synced,
        // is a copy taken before the merge. Session a merges both into one
        // replacement and prunes. Session b appends after the second insertion.
        // Both sides converge, the two pending spans survive, and rejecting
        // every change restores the text.
        let (a, id) = setup("abcdef");
        let p = who_of("p");
        apply_tracked_edits(&a, &id, &[edit(1, 0, "X")], &p, None, 1).unwrap();
        a.commit();
        apply_tracked_edits(&a, &id, &[edit(4, 0, "Y")], &p, None, 2).unwrap();
        a.commit();
        let b = new_doc();
        b.import(&a.export(loro::ExportMode::snapshot()).unwrap())
            .unwrap();
        apply_tracked_edits(&a, &id, &[edit(1, 4, "Z")], &p, None, 3).unwrap();
        a.commit();
        prune(&a);
        a.commit();
        apply_tracked_edits(&b, &id, &[edit(5, 0, "Q")], &p, None, 4).unwrap();
        b.commit();
        sync(&a, &b);
        assert_eq!(body(&a, &id), body(&b, &id));
        let text: Vec<char> = body(&a, &id).chars().collect();
        let mut spans: Vec<String> = resolve_changes(&a, &id)
            .iter()
            .map(|c| text[c.from..c.to].iter().collect())
            .collect();
        spans.sort();
        assert_eq!(spans, ["Q", "Z"]);
        reject_where(&a, &id, |_| true);
        a.commit();
        sync(&a, &b);
        assert_eq!(body(&a, &id), "abcdef");
        assert_eq!(body(&b, &id), "abcdef");
    }

    #[test]
    fn two_sessions_of_one_author_lose_nothing() {
        // Two sessions of one author edit the same text. The server never joins
        // fragments, so each session keeps its own records.
        let (a, id) = setup("abcdef");
        apply_tracked_edits(&a, &id, &[edit(2, 1, "")], &who_of("p"), None, 1).unwrap();
        a.commit();
        let b = new_doc();
        b.import(&a.export(loro::ExportMode::snapshot()).unwrap())
            .unwrap();
        // Session a deletes b, next to its own c.
        apply_tracked_edits(&a, &id, &[edit(1, 1, "")], &who_of("p"), None, 2).unwrap();
        a.commit();
        // Session b deletes d.
        apply_tracked_edits(&b, &id, &[edit(2, 1, "")], &who_of("p"), None, 2).unwrap();
        b.commit();
        sync(&a, &b);
        assert_eq!(body(&a, &id), body(&b, &id));
        let ids: Vec<String> = resolve_changes(&a, &id).into_iter().map(|c| c.id).collect();
        for cid in &ids {
            assert!(reject(&a, cid), "{cid} is not a change");
            a.commit();
        }
        sync(&a, &b);
        assert_eq!(body(&a, &id), "abcdef");
        assert_eq!(body(&b, &id), "abcdef");
    }

    /// A plain record of the earlier model, as the browser wrote it before the
    /// change records became containers.
    fn legacy_value(
        file: &str,
        who: &str,
        deleted: &str,
        anchor: &[u8],
        points: &[(&str, f64)],
    ) -> LoroValue {
        let mut map: HashMap<&str, LoroValue> = HashMap::new();
        map.insert("file", LoroValue::from(file));
        map.insert("who", LoroValue::from(who));
        map.insert("author", LoroValue::from(who));
        map.insert("at", LoroValue::Double(1.0));
        map.insert("run", LoroValue::Null);
        map.insert("start", LoroValue::from(anchor));
        map.insert("end", LoroValue::Null);
        map.insert("deleted", LoroValue::from(deleted));
        if !points.is_empty() {
            let list: Vec<LoroValue> = points
                .iter()
                .map(|(id, at)| {
                    LoroValue::from(HashMap::from([
                        ("id", LoroValue::from(*id)),
                        ("at", LoroValue::Double(*at)),
                    ]))
                })
                .collect();
            map.insert("points", LoroValue::from(list));
        }
        LoroValue::from(map)
    }

    #[test]
    fn legacy_records_are_read_in_place_and_converted_on_touch() {
        // Alice deleted c. Bob deleted b and d, with a swallowed pure deletion of
        // Alice's c between them, so his record points at hers. Both sit at the
        // same character of "ae".
        let build = || {
            let (doc, id) = setup("ae");
            let anchor = text_of(&doc, &id)
                .unwrap()
                .get_cursor(1, Side::Left)
                .unwrap()
                .encode();
            let root = doc.get_map(CHANGES);
            root.insert("alice", legacy_value(&id, "alice", "c", &anchor, &[]))
                .unwrap();
            root.insert(
                "bob",
                legacy_value(&id, "bob", "bd", &anchor, &[("alice", 1.0)]),
            )
            .unwrap();
            doc.commit();
            (doc, id)
        };
        {
            let (doc, id) = build();
            let rows: Vec<(String, String)> = resolve_changes(&doc, &id)
                .into_iter()
                .map(|c| (c.who, c.deleted))
                .collect();
            assert_eq!(
                rows,
                [
                    ("alice".to_string(), "c".to_string()),
                    ("bob".to_string(), "bd".to_string()),
                ]
            );
            let state = resolve_file(&doc, &id).state;
            assert_eq!(state.gaps.len(), 1);
            let texts: Vec<&str> = state.gaps[0]
                .fragments
                .iter()
                .map(|f| f.text.as_str())
                .collect();
            assert_eq!(texts, ["b", "c", "d"]);
        }
        for order in [["bob", "alice"], ["alice", "bob"]] {
            let (doc, id) = build();
            for cid in order {
                assert!(reject(&doc, cid), "{cid} is not a change");
                doc.commit();
            }
            assert_eq!(body(&doc, &id), "abcde", "{order:?}");
            assert!(resolve_changes(&doc, &id).is_empty());
        }
        {
            // An edit elsewhere leaves the plain records where they are.
            let (doc, id) = build();
            apply_tracked_edits(&doc, &id, &[edit(2, 0, "X")], &who_of("cara"), None, 1).unwrap();
            doc.commit();
            assert_eq!(legacy_records(&doc, &id).len(), 2);
        }
        {
            // Deleting a swallows the gap, but the legacy records keep their
            // anchor and rank, so nothing of theirs is written. Only Cara's new
            // change and fragment are containers, and rejecting everything
            // restores the text.
            let (doc, id) = build();
            apply_tracked_edits(&doc, &id, &[edit(0, 1, "")], &who_of("cara"), None, 1).unwrap();
            doc.commit();
            let root = doc.get_map(CHANGES);
            let mut kinds: Vec<String> = Vec::new();
            let mut plain = 0;
            for k in root.keys().map(|k| k.to_string()).collect::<Vec<String>>() {
                match record_map(&root, &k) {
                    Some(m) => kinds.push(str_of(&m, "kind").unwrap_or_default()),
                    None => plain += 1,
                }
            }
            kinds.sort();
            assert_eq!(kinds, ["change", "fragment"]);
            assert_eq!(plain, 2, "the legacy records are no longer plain values");
            reject_where(&doc, &id, |_| true);
            doc.commit();
            assert_eq!(body(&doc, &id), "abcde");
            assert!(resolve_changes(&doc, &id).is_empty());
        }
    }

    #[test]
    fn a_settled_stale_cursor_is_healed() {
        // A fragment is anchored on d, which a raw edit then deletes. The first
        // prune that sees the cursor stale only marks it. Once that mark is old,
        // a later prune heals the cursor onto a live character and drops the mark.
        let (doc, id) = setup("abcdef");
        apply_tracked_edits(&doc, &id, &[edit(2, 1, "")], &who_of("p"), None, 1).unwrap();
        doc.commit();
        text_of(&doc, &id).unwrap().delete(2, 1).unwrap();
        doc.commit();
        let before = resolve_file(&doc, &id).state.gaps;
        assert!(before[0].stale);
        let fid = before[0].fragments[0].id.clone().unwrap();
        let root = doc.get_map(CHANGES);
        let now = now_ms();
        prune_at(&doc, now);
        doc.commit();
        assert!(num_of(&record_map(&root, &fid).unwrap(), "stale").is_some());
        assert!(resolve_file(&doc, &id).state.gaps[0].stale);
        prune_at(&doc, now + 120_000);
        doc.commit();
        let after = resolve_file(&doc, &id).state.gaps;
        assert!(!after[0].stale);
        assert_eq!(after[0].at, 2);
        assert!(record_map(&root, &fid).unwrap().get("stale").is_none());
    }
}
