//! Tracked changes: Word's model on the shared Loro document.
//!
//! A tracked edit goes into the text at once, like any edit, and the `changes`
//! root records it as a pending change: the original text it removed
//! (`deleted`) and a pair of cursors around the text it put there. Accepting
//! drops the record. Rejecting puts `deleted` back where the new text was,
//! then drops the record. See `SPEC-track-changes.md`.
//!
//! [`plan_edits`] is pure and has a twin in `web/src/lib/track-changes.js`;
//! both run `web/tests/fixtures/track-changes.json`. Everything here counts in
//! Unicode code points, which is what Loro cursors use. The fixtures count in
//! UTF-16 and the test runner converts.
//!
//! None of the writers here commit. The caller owns the transaction, the same
//! as with `apply_edits_at`.

use std::collections::{HashMap, HashSet};

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

/// A foreign deletion this change swallowed: the id of that change and the
/// number of original characters of `deleted` that precede it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Point {
    pub id: String,
    pub at: usize,
}

/// A pending change with its cursors resolved to offsets in the text now.
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
    /// Foreign deletions this change swallowed, by place inside `deleted`.
    pub points: Vec<Point>,
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Planned {
    pub change: ResolvedChange,
    pub state: PlanState,
}

/// Plans a batch of edits against the pending changes of one file.
///
/// `edits` are `(from, to, insert)` in the coordinates of `before`, ascending
/// and non-overlapping. They are applied in order, each one adjusted by the
/// edits before it. The result is every change that was or is now pending,
/// in the coordinates of the text after the whole batch, sorted by `from`.
pub fn plan_edits(
    before: &str,
    changes: &[ResolvedChange],
    edits: &[(usize, usize, String)],
    me: &Author,
    run: Option<&str>,
    now_ms: i64,
    mint: &mut dyn FnMut() -> String,
) -> Vec<Planned> {
    let mut text: Vec<char> = before.chars().collect();
    let mut list: Vec<ResolvedChange> = changes.to_vec();
    list.sort_by_key(|c| (c.from, c.to));
    let mut dirty: HashSet<String> = HashSet::new();
    let mut running: isize = 0;

    for (from, to, insert) in edits {
        let ins: Vec<char> = insert.chars().collect();
        if from == to && ins.is_empty() {
            continue;
        }
        let f = (*from as isize + running) as usize;
        let t = (*to as isize + running) as usize;
        let step = Step {
            f,
            t,
            ins: &ins,
            me,
            run,
            now_ms,
        };
        list = plan_one(&text, list, &step, &mut dirty, mint);
        text.splice(f..t, ins.iter().copied());
        running += ins.len() as isize - (t - f) as isize;
    }

    let live: HashSet<&str> = list.iter().map(|c| c.id.as_str()).collect();
    let mut out: Vec<Planned> = list
        .iter()
        .map(|c| Planned {
            state: if dirty.contains(&c.id) {
                PlanState::Set
            } else {
                PlanState::Kept
            },
            change: c.clone(),
        })
        .collect();
    for c in changes {
        if !live.contains(c.id.as_str()) {
            out.push(Planned {
                change: c.clone(),
                state: PlanState::Removed,
            });
        }
    }
    out.sort_by_key(|p| (p.state == PlanState::Removed, p.change.from, p.change.to));
    out
}

/// One edit of a batch, already in the coordinates of the running text.
struct Step<'a> {
    f: usize,
    t: usize,
    ins: &'a [char],
    me: &'a Author<'a>,
    run: Option<&'a str>,
    now_ms: i64,
}

/// Plans one edit. `list` is sorted by `from`; so is the result.
fn plan_one(
    text: &[char],
    list: Vec<ResolvedChange>,
    step: &Step,
    dirty: &mut HashSet<String>,
    mint: &mut dyn FnMut() -> String,
) -> Vec<ResolvedChange> {
    let (f, t, n) = (step.f, step.t, step.ins.len());
    let delta = n as isize - (t - f) as isize;
    let shift = |p: usize| (p as isize + delta) as usize;

    // Runs of original text the edit deletes: [f, t) outside every change.
    let mut runs: Vec<(usize, usize)> = Vec::new();
    let mut pos = f;
    for c in list.iter() {
        if c.from >= t {
            break;
        }
        if c.to <= pos {
            continue;
        }
        if c.from > pos {
            runs.push((pos, c.from));
        }
        pos = pos.max(c.to);
    }
    if pos < t {
        runs.push((pos, t));
    }

    // Only an edit that deletes original text or inserts makes or merges an
    // own change. A deletion inside insertions just shrinks them.
    let trigger = !runs.is_empty() || n > 0;
    let affected: Vec<usize> = if trigger {
        (0..list.len())
            .filter(|&i| {
                let c = &list[i];
                c.who == step.me.who && c.from <= t && c.to >= f
            })
            .collect()
    } else {
        Vec::new()
    };

    let mut out: Vec<ResolvedChange> = Vec::new();

    if trigger {
        // (position, kind, text): an own change's `deleted` sits at its
        // position, and an own pure deletion inside a run splits the run.
        let mut pieces: Vec<(usize, u8, String)> = Vec::new();
        for &i in &affected {
            pieces.push((list[i].from, 0, list[i].deleted.clone()));
        }
        for &(rs, re) in &runs {
            let mut start = rs;
            for &i in &affected {
                let p = list[i].from;
                if list[i].from == list[i].to && p > start && p < re {
                    pieces.push((start, 1, text[start..p].iter().collect()));
                    start = p;
                }
            }
            pieces.push((start, 1, text[start..re].iter().collect()));
        }
        pieces.sort_by_key(|p| (p.0, p.1));
        // Original characters of the new `deleted` text that precede `p`.
        let before = |p: usize| -> usize {
            let own: usize = affected
                .iter()
                .filter(|&&i| list[i].from <= p)
                .map(|&i| list[i].deleted.chars().count())
                .sum();
            let orig: usize = runs
                .iter()
                .map(|&(rs, re)| re.min(p).saturating_sub(rs))
                .sum();
            own + orig
        };
        let mut points: Vec<Point> = Vec::new();
        for (k, &i) in affected.iter().enumerate() {
            if list[i].points.is_empty() {
                continue;
            }
            let own: usize = affected[..k]
                .iter()
                .map(|&j| list[j].deleted.chars().count())
                .sum();
            let orig: usize = runs
                .iter()
                .map(|&(rs, re)| re.min(list[i].from).saturating_sub(rs))
                .sum();
            for pt in &list[i].points {
                points.push(Point {
                    id: pt.id.clone(),
                    at: pt.at + own + orig,
                });
            }
        }
        if t > f {
            for (i, c) in list.iter().enumerate() {
                if affected.contains(&i) || c.deleted.is_empty() {
                    continue;
                }
                let swallowed = if c.from == c.to {
                    f <= c.from && c.from < t
                } else {
                    c.from >= f && c.to <= t
                };
                if swallowed {
                    points.push(Point {
                        id: c.id.clone(),
                        at: before(c.from),
                    });
                }
            }
        }
        let deleted: String = pieces.into_iter().map(|p| p.2).collect();
        let first = affected.first().map(|&i| &list[i]);
        let mut from = f;
        let mut to = f + n;
        for &i in &affected {
            from = from.min(list[i].from);
            if list[i].to > t {
                to = to.max(shift(list[i].to));
            }
        }
        let merged = ResolvedChange {
            id: first.map_or_else(&mut *mint, |c| c.id.clone()),
            // The writer fills in the file of a new change.
            file: first.map_or_else(String::new, |c| c.file.clone()),
            who: step.me.who.to_string(),
            author: step.me.author.to_string(),
            run: step
                .run
                .map(str::to_string)
                .or_else(|| first.and_then(|c| c.run.clone())),
            at: step.now_ms,
            from,
            to,
            deleted,
            points,
        };
        dirty.insert(merged.id.clone());
        out.push(merged);
    }

    for (i, c) in list.iter().enumerate() {
        if affected.contains(&i) {
            continue;
        }
        if trigger && n > 0 && c.from < f && f < c.to {
            let mut head = c.clone();
            head.to = f;
            dirty.insert(head.id.clone());
            out.push(head);
            if c.to > t {
                let mut tail = c.clone();
                tail.id = mint();
                tail.from = f + n;
                tail.to = shift(c.to);
                tail.deleted = String::new();
                dirty.insert(tail.id.clone());
                out.push(tail);
            }
            continue;
        }
        let mut moved = c.clone();
        if c.from == c.to {
            moved.from = map_point(c.from, f, t, n);
            moved.to = moved.from;
            if t > f && f <= c.from && c.from < t {
                dirty.insert(c.id.clone());
            }
        } else {
            moved.from = map_start(c.from, f, t, n);
            moved.to = map_end(c.to, f, t, n);
            if moved.to < moved.from {
                moved.from = moved.to;
            }
            if c.from < t && c.to > f {
                dirty.insert(c.id.clone());
            }
        }
        out.push(moved);
    }

    out.retain(|c| c.from != c.to || !c.deleted.is_empty());
    out.sort_by_key(|c| (c.from, c.to));
    out
}

fn map_start(p: usize, ef: usize, et: usize, len: usize) -> usize {
    if p < ef {
        p
    } else if p >= et {
        p + len - (et - ef)
    } else {
        ef + len
    }
}

fn map_end(p: usize, ef: usize, et: usize, len: usize) -> usize {
    if p <= ef {
        p
    } else if p > et {
        p + len - (et - ef)
    } else {
        ef
    }
}

fn map_point(p: usize, ef: usize, et: usize, len: usize) -> usize {
    if p < ef {
        p
    } else if p >= et {
        p + len - (et - ef)
    } else {
        ef
    }
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

/// An entry read back, with the replacement cursors Loro offers when an
/// anchor character was deleted.
struct Read {
    change: ResolvedChange,
    start_update: Option<Cursor>,
    end_update: Option<Cursor>,
}

/// The entry for `id` with its cursors resolved, or `None` when it is not a
/// well-formed entry or is orphaned (its file or start cursor is gone). An
/// empty change is returned; callers that list changes drop it.
///
/// A cursor resolves to the index of the character it is attached to. The end
/// cursor is attached to the last inserted character, so `to` is one past it,
/// unless that character was deleted: then Loro answers with an `update` and
/// the position the character had, which is already the end.
fn read_entry(doc: &LoroDoc, id: &str) -> Option<Read> {
    let ValueOrContainer::Value(LoroValue::Map(m)) = doc.get_map(CHANGES).get(id)? else {
        return None;
    };
    let file = string_field(&m, "file")?;
    let text = text_of(doc, &file)?;
    let start = Cursor::decode(&bytes_field(&m, "start")?).ok()?;
    if start.container != text.id() {
        return None;
    }
    let start_pos = doc.get_cursor_pos(&start).ok()?;
    let from = start_pos.current.pos;
    let mut end_update = None;
    let to = match bytes_field(&m, "end") {
        Some(bytes) => {
            let end = Cursor::decode(&bytes).ok()?;
            if end.container != text.id() {
                return None;
            }
            let end_pos = doc.get_cursor_pos(&end).ok()?;
            let live = end_pos.update.is_none();
            end_update = end_pos.update;
            end_pos.current.pos + usize::from(live)
        }
        None => from,
    };
    // An end before the start means the inserted text is gone.
    let to = to.max(from);
    let at = match m.get("at") {
        Some(LoroValue::I64(v)) => *v,
        Some(LoroValue::Double(v)) => *v as i64,
        _ => 0,
    };
    let run = string_field(&m, "run");
    let points = match m.get("points") {
        Some(LoroValue::List(items)) => items
            .iter()
            .filter_map(|item| {
                let LoroValue::Map(pm) = item else {
                    return None;
                };
                let at = match pm.get("at")? {
                    LoroValue::I64(v) => usize::try_from(*v).ok()?,
                    LoroValue::Double(v) if *v >= 0.0 => *v as usize,
                    _ => return None,
                };
                Some(Point {
                    id: string_field(pm, "id")?,
                    at,
                })
            })
            .collect(),
        _ => Vec::new(),
    };
    Some(Read {
        change: ResolvedChange {
            id: id.to_string(),
            file,
            who: string_field(&m, "who")?,
            author: string_field(&m, "author")?,
            run,
            at,
            from,
            to,
            deleted: string_field(&m, "deleted")?,
            points,
        },
        start_update: start_pos.update,
        end_update,
    })
}

/// The pending changes of one file, resolved against its text now and sorted
/// by `from`. Orphaned and empty changes are left out.
pub fn resolve_changes(doc: &LoroDoc, file_id: &str) -> Vec<ResolvedChange> {
    let mut out: Vec<ResolvedChange> = doc
        .get_map(CHANGES)
        .keys()
        .filter_map(|id| read_entry(doc, id.as_ref()).map(|r| r.change))
        .filter(|c| c.file == file_id && (c.from < c.to || !c.deleted.is_empty()))
        .collect();
    out.sort_by(|a, b| (a.from, a.to, &a.id).cmp(&(b.from, b.to, &b.id)));
    out
}

fn entry_value(c: &ResolvedChange, start: &Cursor, end: Option<&Cursor>) -> LoroValue {
    let mut m: HashMap<String, LoroValue> = HashMap::new();
    m.insert("file".into(), LoroValue::from(c.file.as_str()));
    m.insert("author".into(), LoroValue::from(c.author.as_str()));
    m.insert("who".into(), LoroValue::from(c.who.as_str()));
    // The browser writes a JS number, which Loro stores as a double.
    m.insert("at".into(), LoroValue::Double(c.at as f64));
    m.insert(
        "run".into(),
        c.run.as_deref().map_or(LoroValue::Null, LoroValue::from),
    );
    m.insert("start".into(), LoroValue::from(start.encode()));
    m.insert(
        "end".into(),
        end.map_or(LoroValue::Null, |e| LoroValue::from(e.encode())),
    );
    m.insert("deleted".into(), LoroValue::from(c.deleted.as_str()));
    if !c.points.is_empty() {
        let list: Vec<LoroValue> = c
            .points
            .iter()
            .map(|p| {
                let mut pm: HashMap<String, LoroValue> = HashMap::new();
                pm.insert("id".into(), LoroValue::from(p.id.as_str()));
                pm.insert("at".into(), LoroValue::Double(p.at as f64));
                LoroValue::from(pm)
            })
            .collect();
        m.insert("points".into(), LoroValue::from(list));
    }
    LoroValue::from(m)
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

/// Applies `edits` (UTF-16, measured against the file as it is now, ascending
/// and non-overlapping, as `apply_edits_at` takes them) to the file's text and
/// records them as tracked changes by `by`. Returns the ids of the changes it
/// created or changed. Errors, leaving the document untouched, when the file
/// is missing or the edits do not fit its text. Does not commit.
pub fn apply_tracked_edits(
    doc: &LoroDoc,
    file_id: &str,
    edits: &[Edit],
    by: &Author,
    run: Option<&str>,
    now_ms: i64,
) -> Result<Vec<String>, String> {
    let text = text_of(doc, file_id).ok_or_else(|| "the file does not exist".to_string())?;
    let before = text.to_string();
    let pending = resolve_changes(doc, file_id);

    let to_cp = |utf16: usize| {
        text.convert_pos(utf16, PosType::Utf16, PosType::Unicode)
            .ok_or_else(|| "an edit does not fit the file".to_string())
    };
    let mut planned_edits: Vec<(usize, usize, String)> = Vec::with_capacity(edits.len());
    for e in edits {
        let from = to_cp(e.at)?;
        let to = to_cp(e.at.saturating_add(e.delete))?;
        planned_edits.push((from, to, e.insert.clone()));
    }
    if !apply_text_edits(&text, edits) {
        return Err("an edit does not fit the file".to_string());
    }

    let mut mint = mint_uuid;
    let plan = plan_edits(
        &before,
        &pending,
        &planned_edits,
        by,
        run,
        now_ms,
        &mut mint,
    );
    let map = doc.get_map(CHANGES);
    let mut set = Vec::new();
    for p in plan {
        match p.state {
            PlanState::Kept => {}
            PlanState::Removed => {
                map.delete(&p.change.id).ok();
            }
            PlanState::Set => {
                let c = &p.change;
                let Some(start) = text.get_cursor(c.from, Side::Left) else {
                    continue;
                };
                let end = if c.to > c.from {
                    text.get_cursor(c.to - 1, Side::Right)
                } else {
                    None
                };
                let mut c = c.clone();
                c.file = file_id.to_string();
                map.insert(&c.id, entry_value(&c, &start, end.as_ref()))
                    .ok();
                set.push(c.id);
            }
        }
    }
    Ok(set)
}

/// Accepts a change: drops the record and keeps the text. Returns false when
/// there is no such change. An orphaned record is dropped too. Does not
/// commit.
pub fn accept(doc: &LoroDoc, id: &str) -> bool {
    let map = doc.get_map(CHANGES);
    if map.get(id).is_none() {
        return false;
    }
    map.delete(id).is_ok()
}

/// The pending change, if any, that lists `id` in its `points`.
fn find_owner(doc: &LoroDoc, id: &str, file: &str) -> Option<ResolvedChange> {
    let map = doc.get_map(CHANGES);
    for key in map.keys() {
        if key.as_ref() == id {
            continue;
        }
        let Some(r) = read_entry(doc, key.as_ref()) else {
            continue;
        };
        if r.change.file == file && r.change.points.iter().any(|p| p.id == id) {
            return Some(r.change);
        }
    }
    None
}

/// `x` was rejected and its text is back at `x.from`. Its owner swallowed it
/// at offset `a` of the owner's `deleted`, so the owner splits there: the part
/// before stays a pure deletion anchored on x's text, the part after keeps the
/// owner's inserted span.
fn split_owner(doc: &LoroDoc, text: &LoroText, owner_id: &str, x_id: &str, x: &ResolvedChange) {
    // Read again: the text under the owner moved when x's text came back.
    let Some(Read { change: o, .. }) = read_entry(doc, owner_id) else {
        return;
    };
    let Some(mine) = o.points.iter().find(|p| p.id == x_id) else {
        return;
    };
    let chars: Vec<char> = o.deleted.chars().collect();
    let a = mine.at.min(chars.len());
    let left_text: String = chars[..a].iter().collect();
    let right_text: String = chars[a..].iter().collect();
    let others: Vec<&Point> = o.points.iter().filter(|p| p.id != x_id).collect();
    let left_points: Vec<Point> = others.iter().filter(|p| p.at < a).map(|p| (*p).clone()).collect();
    let right_points: Vec<Point> = others
        .iter()
        .filter(|p| p.at >= a)
        .map(|p| Point {
            id: p.id.clone(),
            at: p.at - a,
        })
        .collect();
    let has_inserted = o.to > o.from;
    let map = doc.get_map(CHANGES);

    let mut right = ResolvedChange {
        deleted: right_text,
        points: right_points,
        ..o.clone()
    };
    let right_cursors = if has_inserted {
        let start = text.get_cursor(o.from, Side::Left);
        let end = text.get_cursor(o.to - 1, Side::Right);
        start.map(|s| (s, end))
    } else if !right.deleted.is_empty() {
        text.get_cursor(x.from + x.deleted.chars().count(), Side::Left)
            .map(|s| (s, None))
    } else {
        None
    };

    if left_text.is_empty() {
        match right_cursors {
            Some((s, e)) => {
                map.insert(&o.id, entry_value(&right, &s, e.as_ref())).ok();
            }
            None => {
                map.delete(&o.id).ok();
            }
        }
        return;
    }
    let left = ResolvedChange {
        deleted: left_text,
        points: left_points,
        ..o.clone()
    };
    if let Some(s) = text.get_cursor(x.from, Side::Left) {
        map.insert(&o.id, entry_value(&left, &s, None)).ok();
    }
    if let Some((s, e)) = right_cursors {
        right.id = mint_uuid();
        map.insert(&right.id, entry_value(&right, &s, e.as_ref())).ok();
    }
}

/// Rejects a change: puts `deleted` back in place of the inserted text, then
/// drops the record. Returns false when there is no such change. An orphaned
/// record is only dropped. Does not commit.
pub fn reject(doc: &LoroDoc, id: &str) -> bool {
    let map = doc.get_map(CHANGES);
    if map.get(id).is_none() {
        return false;
    }
    if let Some(Read { change: c, .. }) = read_entry(doc, id) {
        if let Some(text) = text_of(doc, &c.file) {
            let owner = find_owner(doc, id, &c.file);
            if c.to > c.from {
                text.delete(c.from, c.to - c.from).ok();
            }
            if !c.deleted.is_empty() {
                text.insert(c.from, &c.deleted).ok();
            }
            if let Some(o) = owner {
                split_owner(doc, &text, &o.id, id, &c);
            }
            // Deletions this change swallowed move back to where they sat
            // inside it.
            for pt in &c.points {
                let Some(target) = read_entry(doc, &pt.id) else {
                    continue;
                };
                let t = &target.change;
                if t.file != c.file || t.to != t.from {
                    continue;
                }
                let Some(start) = text.get_cursor(c.from + pt.at, Side::Left) else {
                    continue;
                };
                map.insert(&pt.id, entry_value(t, &start, None)).ok();
            }
        }
    }
    map.delete(id).is_ok()
}

/// Removes every orphaned, malformed or empty entry and returns how many it
/// removed. An entry whose cursor came back with a replacement is rewritten
/// with fresh cursors made from its resolved span, so a deleted anchor is
/// looked up the slow way once; that does not count. An editor calls this in its next local commit. Does not commit.
pub fn prune(doc: &LoroDoc) -> usize {
    let map = doc.get_map(CHANGES);
    let ids: Vec<String> = map.keys().map(|k| k.to_string()).collect();
    let mut removed = 0;
    for id in ids {
        let read = read_entry(doc, &id)
            .filter(|r| r.change.from < r.change.to || !r.change.deleted.is_empty());
        let Some(read) = read else {
            if map.delete(&id).is_ok() {
                removed += 1;
            }
            continue;
        };
        if read.start_update.is_none() && read.end_update.is_none() {
            continue;
        }
        // Loro's replacement cursor sits on the next live character, which
        // the end rule would overshoot. Write fresh cursors from the span.
        let c = &read.change;
        let Some(text) = text_of(doc, &c.file) else {
            continue;
        };
        let Some(start) = text.get_cursor(c.from, Side::Left) else {
            continue;
        };
        let end = if c.to > c.from {
            text.get_cursor(c.to - 1, Side::Right)
        } else {
            None
        };
        map.insert(&id, entry_value(c, &start, end.as_ref())).ok();
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
    struct FxPoint {
        id: String,
        at: usize,
    }

    #[derive(Deserialize)]
    struct FxChange {
        id: String,
        who: String,
        from: usize,
        to: usize,
        deleted: String,
        points: Option<Vec<FxPoint>>,
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
        changes: Vec<FxChange>,
        me: FxMe,
        edits: Vec<FxEdit>,
        after: String,
        expect: Vec<FxChange>,
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

    fn resolved(s: &str, c: &FxChange) -> ResolvedChange {
        ResolvedChange {
            id: c.id.clone(),
            file: "f".into(),
            who: c.who.clone(),
            author: c.who.clone(),
            run: None,
            at: 0,
            from: cp(s, c.from),
            to: cp(s, c.to),
            deleted: c.deleted.clone(),
            points: points_of(c),
        }
    }

    /// The fixture's points with `at` converted from UTF-16 to code points.
    fn points_of(c: &FxChange) -> Vec<Point> {
        c.points
            .iter()
            .flatten()
            .map(|p| Point {
                id: p.id.clone(),
                at: cp(&c.deleted, p.at),
            })
            .collect()
    }

    #[test]
    fn shared_fixtures() {
        let fixtures: Vec<Fixture> = serde_json::from_str(FIXTURES).unwrap();
        assert!(!fixtures.is_empty());
        for fx in fixtures {
            let changes: Vec<ResolvedChange> =
                fx.changes.iter().map(|c| resolved(&fx.before, c)).collect();
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
            let plan = plan_edits(&fx.before, &changes, &edits, &me, None, 1, &mut mint);
            // Points are compared only when the fixture lists them.
            let listed: HashSet<String> = fx
                .expect
                .iter()
                .filter(|c| c.points.is_some())
                .map(|c| c.id.clone())
                .collect();
            type Row = (String, String, usize, usize, String, Vec<Point>);
            let mut got: Vec<Row> = plan
                .into_iter()
                .filter(|p| p.state != PlanState::Removed)
                .map(|p| {
                    let c = p.change;
                    let pts = if listed.contains(&c.id) {
                        c.points
                    } else {
                        Vec::new()
                    };
                    (c.id, c.who, c.from, c.to, c.deleted, pts)
                })
                .collect();
            let mut want: Vec<Row> = fx
                .expect
                .iter()
                .map(|c| {
                    let r = resolved(&fx.after, c);
                    (r.id, r.who, r.from, r.to, r.deleted, r.points)
                })
                .collect();
            got.sort_by_key(|c| (c.2, c.3, c.0.clone()));
            want.sort_by_key(|c| (c.2, c.3, c.0.clone()));
            assert_eq!(got, want, "fixture: {}", fx.name);
        }
    }

    const ME: Author<'static> = Author {
        who: "me",
        author: "Me",
    };

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
        assert!(orphan.len() == 1);
        // The orphan's file is in the document, so it resolves. Delete the
        // file to orphan it, then add a record that is not a map.
        doc.get_map(FILES).delete(&other_id).unwrap();
        doc.get_map(CHANGES).insert("junk", 1).unwrap();
        doc.commit();
        assert_eq!(prune(&doc), 2);
        assert_eq!(resolve_changes(&doc, &id).len(), 1);
        assert_eq!(prune(&doc), 0);
    }

    const BOB: Author<'static> = Author {
        who: "bob",
        author: "Bob",
    };

    #[test]
    fn swallowed_deletion_points_reject_in_either_order() {
        for bob_first in [true, false] {
            let (doc, id) = setup("abcd");
            let a = apply_tracked_edits(&doc, &id, &[edit(1, 1, "")], &ME, None, 1).unwrap()[0]
                .clone();
            doc.commit();
            let b = apply_tracked_edits(&doc, &id, &[edit(1, 1, "")], &BOB, None, 2).unwrap()[0]
                .clone();
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
    fn a_swallowed_point_in_the_middle_of_the_new_deletion() {
        for bob_first in [true, false] {
            let (doc, id) = setup("abcde");
            let a = apply_tracked_edits(&doc, &id, &[edit(2, 1, "")], &ME, None, 1).unwrap();
            doc.commit();
            assert_eq!(body(&doc, &id), "abde");
            let b = apply_tracked_edits(&doc, &id, &[edit(1, 2, "")], &BOB, None, 2).unwrap();
            doc.commit();
            assert_eq!(body(&doc, &id), "ae");
            let order = if bob_first { [&b[0], &a[0]] } else { [&a[0], &b[0]] };
            for change in order {
                assert!(reject(&doc, change));
                doc.commit();
            }
            assert_eq!(body(&doc, &id), "abcde");
        }
    }

    /// Alice deletes then Bob swallows her point; reject in the given order
    /// ("a" is Alice's change, "b" is Bob's).
    fn swallow(start: &str, alice: Edit, bob: Edit) -> (LoroDoc, String, String, String) {
        let (doc, id) = setup(start);
        let a = apply_tracked_edits(&doc, &id, &[alice], &ME, None, 1).unwrap()[0].clone();
        doc.commit();
        let b = apply_tracked_edits(&doc, &id, &[bob], &BOB, None, 2).unwrap()[0].clone();
        doc.commit();
        (doc, id, a, b)
    }

    #[test]
    fn rejecting_the_swallowed_change_first_splits_the_owner() {
        let cases = [
            ("abcd", (1, 1), (1, 1), "ad", "abcd"),
            ("abcde", (2, 1), (1, 2), "ae", "abcde"),
        ];
        for (start, alice, bob, mid, end) in cases {
            let (doc, id, a, b) = swallow(
                start,
                edit(alice.0, alice.1, ""),
                edit(bob.0, bob.1, ""),
            );
            assert_eq!(body(&doc, &id), mid);
            assert!(reject(&doc, &a));
            doc.commit();
            while let Some(c) = resolve_changes(&doc, &id).first() {
                assert!(reject(&doc, &c.id.clone()));
                doc.commit();
            }
            assert_eq!(body(&doc, &id), end, "{start}");
            assert!(resolve_changes(&doc, &id).is_empty());
            let _ = b;
        }
    }

    #[test]
    fn the_owner_is_a_replacement_and_the_swallowed_change_goes_first() {
        let (doc, id, a, b) = swallow("abcde", edit(2, 1, ""), edit(1, 2, "Z"));
        assert_eq!(body(&doc, &id), "aZe");
        assert!(reject(&doc, &a));
        doc.commit();
        assert_eq!(body(&doc, &id), "acZe");
        let pending = resolve_changes(&doc, &id);
        assert_eq!(pending.len(), 2);
        let left = pending.iter().find(|c| c.id == b).unwrap();
        assert_eq!((left.from, left.to, left.deleted.as_str()), (1, 1, "b"));
        let right = pending.iter().find(|c| c.id != b).unwrap();
        assert_eq!((right.from, right.to, right.deleted.as_str()), (2, 3, "d"));
        assert!(reject(&doc, &b));
        doc.commit();
        assert_eq!(body(&doc, &id), "abcZe");
        assert!(reject(&doc, &right.id));
        doc.commit();
        assert_eq!(body(&doc, &id), "abcde");
    }
}
