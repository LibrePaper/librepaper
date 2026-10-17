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
    Container, ContainerTrait, LoroDoc, LoroText, LoroValue, ValueOrContainer,
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
}

/// Original text deleted by one change, as it sits in a gap.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fragment {
    pub change: String,
    pub text: String,
}

/// The deleted texts at one point of the text, in document order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Gap {
    pub id: String,
    pub at: usize,
    pub fragments: Vec<Fragment>,
}

/// The live records of one file, resolved against its text now.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
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
#[derive(Clone, Debug, Default, PartialEq, Eq)]
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

fn push_fragment(list: &mut Vec<Fragment>, change: &str, text: &str) {
    match list.last_mut() {
        Some(last) if last.change == change => last.text.push_str(text),
        _ => list.push(Fragment {
            change: change.to_string(),
            text: text.to_string(),
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
    let mut owned: Vec<String> = Vec::new();
    for (c, _) in &changes {
        if c.who == me.who && has_span(c) && c.from <= et && c.to >= ef {
            push_unique(&mut owned, &c.id);
        }
    }
    for g in &swallowed {
        for f in &g.fragments {
            if let Some((c, _)) = changes.iter().find(|(c, _)| c.id == f.change) {
                if c.who == me.who && !has_span(c) {
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
                push_fragment(target, change, &f.text);
            }
        }
        if let (Some(m), Some(ch)) = (&mine, cur.get(p)) {
            if p < et && span_end_at(&changes, p).is_none() {
                push_fragment(&mut merged, &m.id, &ch.to_string());
            }
        }
    }

    // Spans: foreign spans shrink, move or split around the edit.
    let mut out: Vec<Row> = Vec::new();
    let mut new_spans: Vec<Row> = Vec::new();
    for (c, st) in &changes {
        if mine.is_some() && owned.contains(&c.id) {
            continue;
        }
        if !has_span(c) || c.to <= ef {
            out.push((c.clone(), *st));
            continue;
        }
        if c.from >= et {
            out.push((
                ChangeRecord {
                    from: shift(c.from),
                    to: shift(c.to),
                    ..c.clone()
                },
                *st,
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
            out.push((
                ChangeRecord {
                    from: ef + len,
                    to: ef + len,
                    ..c.clone()
                },
                PlanState::Set,
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
                    push_fragment(&mut list, &f.change, &f.text);
                }
            }
            for f in &merged {
                push_fragment(&mut list, &f.change, &f.text);
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
                push_fragment(&mut list, change, &f.text);
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
    let mut live: Vec<Row> = out
        .into_iter()
        .filter(|(c, _)| has_span(c) || referenced.contains(&c.id))
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

/// The raw entry `id` in the `changes` root, when it is a map.
fn record(doc: &LoroDoc, id: &str) -> Option<loro::LoroMapValue> {
    match doc.get_map(CHANGES).get(id)? {
        ValueOrContainer::Value(LoroValue::Map(m)) => Some(m),
        _ => None,
    }
}

fn record_ids(doc: &LoroDoc) -> Vec<String> {
    doc.get_map(CHANGES).keys().map(|k| k.to_string()).collect()
}

fn text_of(doc: &LoroDoc, file_id: &str) -> Option<LoroText> {
    match doc.get_map(FILES).get(file_id)? {
        ValueOrContainer::Container(Container::Text(t)) => Some(t),
        _ => None,
    }
}

fn string_field(m: &loro::LoroMapValue, key: &str) -> Option<String> {
    match m.get(key)? {
        LoroValue::String(s) => Some(s.to_string()),
        _ => None,
    }
}

fn bytes_field(m: &loro::LoroMapValue, key: &str) -> Option<Vec<u8>> {
    match m.get(key)? {
        LoroValue::Binary(b) => Some(b.to_vec()),
        _ => None,
    }
}

/// The fragments of a gap record. `None` when the field is not a list;
/// malformed and empty-text fragments are left out.
fn fragments_field(m: &loro::LoroMapValue) -> Option<Vec<Fragment>> {
    let LoroValue::List(items) = m.get("fragments")? else {
        return None;
    };
    Some(
        items
            .iter()
            .filter_map(|item| {
                let LoroValue::Map(pm) = item else {
                    return None;
                };
                let change = string_field(pm, "change")?;
                let text = string_field(pm, "text")?;
                (!text.is_empty()).then_some(Fragment { change, text })
            })
            .collect(),
    )
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

/// The ids whose cursors came back with a replacement.
#[derive(Default)]
struct Stale {
    changes: HashSet<String>,
    gaps: HashSet<String>,
}

/// Resolves the live records of one file against its text now. A change with
/// no span takes its place from its first fragment; one with neither span nor
/// fragment is empty and left out. Records without a `kind` are ignored.
fn resolve_file(doc: &LoroDoc, file_id: &str) -> (FileState, Stale) {
    let mut stale = Stale::default();
    let Some(text) = text_of(doc, file_id) else {
        return (FileState::default(), stale);
    };
    let mut changes: Vec<ChangeRecord> = Vec::new();
    let mut gaps: Vec<Gap> = Vec::new();
    for id in record_ids(doc) {
        let Some(m) = record(doc, &id) else {
            continue;
        };
        if string_field(&m, "file").as_deref() != Some(file_id) {
            continue;
        }
        match string_field(&m, "kind").as_deref() {
            Some("gap") => {
                let Some(start) = bytes_field(&m, "start") else {
                    continue;
                };
                let Some(fragments) = fragments_field(&m) else {
                    continue;
                };
                if fragments.is_empty() {
                    continue;
                }
                let Some(pos) = cursor_pos(doc, &text, &start) else {
                    continue;
                };
                if pos.stale {
                    stale.gaps.insert(id.clone());
                }
                gaps.push(Gap {
                    id,
                    at: pos.offset,
                    fragments,
                });
            }
            Some("change") => {
                let Some(who) = string_field(&m, "who") else {
                    continue;
                };
                let Some(author) = string_field(&m, "author") else {
                    continue;
                };
                let at = match m.get("at") {
                    Some(LoroValue::I64(v)) => *v,
                    Some(LoroValue::Double(v)) => *v as i64,
                    _ => 0,
                };
                let (from, to, is_stale) = match bytes_field(&m, "start") {
                    None => (0, 0, false),
                    Some(start) => {
                        let Some(s) = cursor_pos(doc, &text, &start) else {
                            continue;
                        };
                        let mut to = s.offset;
                        let mut is_stale = s.stale;
                        if let Some(end) = bytes_field(&m, "end") {
                            let Some(e) = cursor_pos(doc, &text, &end) else {
                                continue;
                            };
                            if e.stale {
                                is_stale = true;
                            }
                            // A live end cursor sits on the last inserted
                            // character; a replaced one already is the end.
                            to = if e.stale { e.offset } else { e.offset + 1 };
                            to = to.max(s.offset);
                        }
                        (s.offset, to, is_stale)
                    }
                };
                if is_stale {
                    stale.changes.insert(id.clone());
                }
                changes.push(ChangeRecord {
                    id,
                    who,
                    author,
                    run: string_field(&m, "run"),
                    at,
                    from,
                    to,
                });
            }
            _ => {}
        }
    }
    gaps.sort_by(|a, b| a.at.cmp(&b.at).then_with(|| a.id.cmp(&b.id)));
    let mut live: Vec<ChangeRecord> = Vec::new();
    for mut c in changes {
        if c.to == c.from {
            let Some(g) = gaps
                .iter()
                .find(|g| g.fragments.iter().any(|f| f.change == c.id))
            else {
                continue;
            };
            c.from = g.at;
            c.to = g.at;
        }
        live.push(c);
    }
    live.sort_by(|a, b| (a.from, a.to, &a.id).cmp(&(b.from, b.to, &b.id)));
    (
        FileState {
            changes: live,
            gaps,
        },
        stale,
    )
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
/// by `(from, to, id)`. Orphaned and empty changes are left out.
pub fn resolve_changes(doc: &LoroDoc, file_id: &str) -> Vec<ResolvedChange> {
    let (state, _) = resolve_file(doc, file_id);
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

/// The stored form of a gap.
fn gap_map(file: &str, start: Vec<u8>, fragments: &[Fragment]) -> LoroValue {
    let mut m: HashMap<String, LoroValue> = HashMap::new();
    m.insert("kind".into(), LoroValue::from("gap"));
    m.insert("file".into(), LoroValue::from(file));
    m.insert("start".into(), LoroValue::from(start));
    let list: Vec<LoroValue> = fragments
        .iter()
        .map(|f| {
            let mut fm: HashMap<String, LoroValue> = HashMap::new();
            fm.insert("change".into(), LoroValue::from(f.change.as_str()));
            fm.insert("text".into(), LoroValue::from(f.text.as_str()));
            LoroValue::from(fm)
        })
        .collect();
    m.insert("fragments".into(), LoroValue::from(list));
    LoroValue::from(m)
}

/// The stored form of a gap at `at`. `None` when the text has no cursor there.
fn gap_value(text: &LoroText, file: &str, at: usize, fragments: &[Fragment]) -> Option<LoroValue> {
    let start = text.get_cursor(at, Side::Left)?;
    Some(gap_map(file, start.encode(), fragments))
}

/// The stored form of a change. A change with a span gets cursors on its
/// first and last inserted characters; one without gets none.
fn change_value(text: &LoroText, file: &str, c: &ChangeRecord) -> Option<LoroValue> {
    let (start, end) = if c.to > c.from {
        (
            Some(text.get_cursor(c.from, Side::Left)?),
            Some(text.get_cursor(c.to - 1, Side::Right)?),
        )
    } else {
        (None, None)
    };
    let mut m: HashMap<String, LoroValue> = HashMap::new();
    m.insert("kind".into(), LoroValue::from("change"));
    m.insert("file".into(), LoroValue::from(file));
    m.insert("author".into(), LoroValue::from(c.author.as_str()));
    m.insert("who".into(), LoroValue::from(c.who.as_str()));
    // The browser writes a JS number, which Loro stores as a double.
    m.insert("at".into(), LoroValue::Double(c.at as f64));
    m.insert(
        "run".into(),
        c.run.as_deref().map_or(LoroValue::Null, LoroValue::from),
    );
    m.insert(
        "start".into(),
        start.map_or(LoroValue::Null, |s| LoroValue::from(s.encode())),
    );
    m.insert(
        "end".into(),
        end.map_or(LoroValue::Null, |e| LoroValue::from(e.encode())),
    );
    Some(LoroValue::from(m))
}

/// Writes the planned records of `file_id` against its current text. Does not
/// commit. Returns the ids of the change records it set.
pub fn write_planned(doc: &LoroDoc, file_id: &str, plan: &Plan) -> Vec<String> {
    let Some(text) = text_of(doc, file_id) else {
        return Vec::new();
    };
    let map = doc.get_map(CHANGES);
    for (g, st) in &plan.gaps {
        match st {
            PlanState::Removed => {
                map.delete(&g.id).ok();
            }
            PlanState::Set => {
                if let Some(v) = gap_value(&text, file_id, g.at, &g.fragments) {
                    map.insert(&g.id, v).ok();
                }
            }
            PlanState::Kept => {}
        }
    }
    let mut written = Vec::new();
    for (c, st) in &plan.changes {
        match st {
            PlanState::Removed => {
                map.delete(&c.id).ok();
            }
            PlanState::Set => {
                if let Some(v) = change_value(&text, file_id, c) {
                    map.insert(&c.id, v).ok();
                    written.push(c.id.clone());
                }
            }
            PlanState::Kept => {}
        }
    }
    written
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
    let (state, _) = resolve_file(doc, file_id);

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

/// Accepts a change: its fragments leave every gap (a gap left empty goes),
/// and its record is dropped. The text stays. Returns false when `id` is not
/// a change record. Does not commit.
pub fn accept(doc: &LoroDoc, id: &str) -> bool {
    let Some(m) = record(doc, id) else {
        return false;
    };
    if string_field(&m, "kind").as_deref() != Some("change") {
        return false;
    }
    let map = doc.get_map(CHANGES);
    for gid in record_ids(doc) {
        let Some(gm) = record(doc, &gid) else {
            continue;
        };
        if string_field(&gm, "kind").as_deref() != Some("gap") {
            continue;
        }
        let Some(fragments) = fragments_field(&gm) else {
            continue;
        };
        let kept: Vec<Fragment> = fragments
            .iter()
            .filter(|f| f.change != id)
            .cloned()
            .collect();
        if kept.len() == fragments.len() {
            continue;
        }
        if kept.is_empty() {
            map.delete(&gid).ok();
            continue;
        }
        let (Some(file), Some(start)) = (string_field(&gm, "file"), bytes_field(&gm, "start"))
        else {
            continue;
        };
        map.insert(&gid, gap_map(&file, start, &kept)).ok();
    }
    map.delete(id).ok();
    true
}

/// Writes the pending run of other changes' fragments as a new gap at
/// `anchor`, and empties the run.
fn put_run(doc: &LoroDoc, text: &LoroText, file: &str, anchor: usize, pending: &mut Vec<Fragment>) {
    if pending.is_empty() {
        return;
    }
    if let Some(v) = gap_value(text, file, anchor, pending.as_slice()) {
        doc.get_map(CHANGES).insert(&mint_uuid(), v).ok();
    }
    pending.clear();
}

/// Puts the gaps in `list` back at `at`. Walks their fragments in order: the
/// rejected change's text is inserted where the walk stands, and each run of
/// other changes' fragments becomes a gap anchored on the character after it.
fn put_back(doc: &LoroDoc, text: &LoroText, file: &str, id: &str, at: usize, mut list: Vec<Gap>) {
    list.sort_by_key(|g| g.at);
    let fragments: Vec<Fragment> = list
        .iter()
        .flat_map(|g| g.fragments.iter().cloned())
        .collect();
    let map = doc.get_map(CHANGES);
    for g in &list {
        map.delete(&g.id).ok();
    }
    let mut pos = at;
    let mut pending: Vec<Fragment> = Vec::new();
    for f in fragments {
        if f.change == id {
            text.insert(pos, &f.text).ok();
            put_run(doc, text, file, pos, &mut pending);
            pos += f.text.chars().count();
        } else {
            pending.push(f);
        }
    }
    put_run(doc, text, file, pos, &mut pending);
}

/// Puts back the text of change `id` and the gaps it sat in. The gaps beyond
/// its span come back first, while its text still holds their offsets; then
/// the span goes, and the gaps at and before it come back.
fn restore_change(doc: &LoroDoc, text: &LoroText, file: &str, id: &str) {
    let (state, _) = resolve_file(doc, file);
    let span = state
        .changes
        .iter()
        .find(|c| c.id == id)
        .filter(|c| c.to > c.from)
        .map(|c| (c.from, c.to));
    // The gaps to put back: every gap holding this change's fragments and,
    // with a span, every gap from its start to the character after it. Those
    // collapse onto the span's start when it goes.
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
                put_back(doc, text, file, id, *at, list);
            }
        }
    }
    if let Some((from, to)) = span {
        text.delete(from, to - from).ok();
    }
    for at in &positions {
        if let Some(list) = groups.remove(at) {
            put_back(doc, text, file, id, *at, list);
        }
    }
}

/// Rejects a change: puts its text back where it was, then drops its record.
/// Returns false when `id` is not a change record. Does not commit.
pub fn reject(doc: &LoroDoc, id: &str) -> bool {
    let Some(m) = record(doc, id) else {
        return false;
    };
    if string_field(&m, "kind").as_deref() != Some("change") {
        return false;
    }
    if let Some(file) = string_field(&m, "file") {
        if let Some(text) = text_of(doc, &file) {
            restore_change(doc, &text, &file, id);
        }
    }
    doc.get_map(CHANGES).delete(id).ok();
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

/// Removes every change or gap record that does not resolve to a live one, and
/// returns how many it removed. A record whose cursor came back with a
/// replacement is rewritten with fresh cursors, which does not count. Records
/// without a `kind` are left alone. Does not commit.
pub fn prune(doc: &LoroDoc) -> usize {
    let map = doc.get_map(CHANGES);
    let files: BTreeSet<String> = record_ids(doc)
        .iter()
        .filter_map(|id| {
            let m = record(doc, id)?;
            let kind = string_field(&m, "kind")?;
            if kind != "change" && kind != "gap" {
                return None;
            }
            string_field(&m, "file")
        })
        .collect();
    let mut removed = 0;
    for file in files {
        let (state, stale) = resolve_file(doc, &file);
        let live: HashSet<String> = state
            .changes
            .iter()
            .map(|c| c.id.clone())
            .chain(state.gaps.iter().map(|g| g.id.clone()))
            .collect();
        for id in record_ids(doc) {
            let Some(m) = record(doc, &id) else {
                continue;
            };
            if string_field(&m, "file").as_deref() != Some(file.as_str()) {
                continue;
            }
            if !matches!(
                string_field(&m, "kind").as_deref(),
                Some("change") | Some("gap")
            ) {
                continue;
            }
            if !live.contains(&id) {
                map.delete(&id).ok();
                removed += 1;
            }
        }
        if let Some(text) = text_of(doc, &file) {
            for g in &state.gaps {
                if stale.gaps.contains(&g.id) {
                    if let Some(v) = gap_value(&text, &file, g.at, &g.fragments) {
                        map.insert(&g.id, v).ok();
                    }
                }
            }
            for c in &state.changes {
                if c.to > c.from && stale.changes.contains(&c.id) {
                    if let Some(v) = change_value(&text, &file, c) {
                        map.insert(&c.id, v).ok();
                    }
                }
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
                change: f.change.clone(),
                text: f.text.clone(),
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
                .filter(|(_, st)| *st != PlanState::Removed)
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
        // An empty record: no span and no fragment.
        let mut empty: HashMap<String, LoroValue> = HashMap::new();
        for (k, v) in [
            ("kind", LoroValue::from("change")),
            ("file", LoroValue::from(id.as_str())),
            ("who", LoroValue::from("me")),
            ("author", LoroValue::from("Me")),
            ("at", LoroValue::Double(1.0)),
            ("run", LoroValue::Null),
            ("start", LoroValue::Null),
            ("end", LoroValue::Null),
        ] {
            empty.insert(k.to_string(), v);
        }
        doc.get_map(CHANGES)
            .insert("empty", LoroValue::from(empty))
            .unwrap();
        // A record without a kind is not ours to touch.
        doc.get_map(CHANGES).insert("junk", 1).unwrap();
        doc.commit();
        assert_eq!(prune(&doc), 2);
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
                assert_eq!(doc.get_map(CHANGES).keys().count(), 0, "{label}");
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
                assert_eq!(doc.get_map(CHANGES).keys().count(), 0, "{label}");
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
            let left = doc.get_map(CHANGES).keys().count();
            let label = format!(
                "iter={iter} start={start} verdict={verdict:?} log={log:?} -> {final_text:?} expected={expected:?}"
            );
            assert_eq!(got, expected, "{label}");
            assert_eq!(originals, want_originals, "{label}");
            assert_eq!(left, 0, "{label}");
        }
    }
}
