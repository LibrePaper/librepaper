//! Comment anchors: where a comment sits in the shared document.
//!
//! An anchor is one record in the `anchors` root, one map container per comment
//! id. A record holds the file it sits in, two Loro cursors (start and end)
//! while it is attached, and the quote the comment was made on. A record whose
//! text was deleted whole is detached: it keeps its quote and, when a tracked
//! deletion took the text, the id of the fragment that holds it.
//!
//! [`plan_operation`] is pure. It moves the attached anchors through one batch
//! of edits, and re-pins a detached anchor when an insertion brings its text
//! back exactly once. A deletion that cuts one end off an attached anchor
//! remembers the cut piece, so an insertion that brings that piece back at the
//! same edge rejoins the anchor. The [`MoveMemory`] is per session: deleted text
//! is remembered, so that cut and paste, drag and drop, undo and agent patches
//! keep comments exact. Nothing searches the document for words.
//!
//! This is a port of `web/src/lib/anchors.js`. Both share the record keys and
//! `web/tests/fixtures/anchors.json`. The browser's AnchorIndex cache and the
//! planner's foreign mode are browser-only, so they are not ported here.
//!
//! Offsets are UTF-16 code units, as in the JavaScript module and in the
//! `offset` and `length` keys that browsers write. This planner therefore counts
//! differently from [`super::track`], which counts code points. Strings are
//! sliced through helpers that map UTF-16 offsets to byte offsets. The
//! conversion to a Loro code point position happens only where a cursor is made
//! or read.
//!
//! No function here commits; the caller owns the transaction. Each writer calls
//! `doc.set_next_commit_origin("anchors")` before its first write, and does
//! every read before its first write, because resolving a stale cursor makes
//! Loro commit pending operations implicitly.

use std::collections::{HashMap, HashSet};

use loro::{
    cursor::{Cursor, PosType, Side as CursorSide},
    Container, ContainerTrait, LoroDoc, LoroMap, LoroText, LoroValue, ValueOrContainer,
};

use crate::document::session::{Edit, CHANGES, FILES};

pub const ANCHORS: &str = "anchors";
pub const ANCHORS_ORIGIN: &str = "anchors";

/// An anchor as the planner and the readers see it. `from` and `to` are UTF-16
/// offsets, and mean nothing while the anchor is detached.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Anchor {
    pub id: String,
    pub file: String,
    pub from: usize,
    pub to: usize,
    pub detached: bool,
    /// The fragment that holds the deleted text, when a tracked deletion took it.
    pub fragment: Option<String>,
    /// Where the anchor starts within the fragment's deleted text.
    pub offset: Option<usize>,
    /// The span's length at detachment, in UTF-16 units. None while attached,
    /// and when the span is gone.
    pub length: Option<usize>,
    pub exact: String,
    pub prefix: String,
    pub suffix: String,
    /// Attached, but its cursors resolve to `to <= from`.
    pub collapsed: bool,
    /// A cursor cannot be resolved at all.
    pub unresolved: bool,
}

impl Anchor {
    /// Only an attached anchor resolves to a span, so only it moves.
    fn attached(&self) -> bool {
        !self.detached && !self.collapsed && !self.unresolved
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlanState {
    Kept,
    Set,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlannedAnchor {
    pub anchor: Anchor,
    pub state: PlanState,
    /// Its text was cut, overlapped by a deletion, or re-pinned in this
    /// operation. False for an anchor that only shifted or grew.
    pub touched: bool,
    /// The file a re-pinned anchor came from.
    pub moved_from: Option<String>,
}

/// A fragment of the deleted text. `edit` is the index of the deleting edit,
/// and `start` and `end` are offsets within that edit's deleted text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FragmentRange {
    pub id: String,
    pub edit: usize,
    pub start: usize,
    pub end: usize,
}

/// One file of an operation: its text before the edits, the anchors resolved
/// before them, and the edits in before-coordinates (UTF-16, ascending).
#[derive(Clone, Debug)]
pub struct OperationFile {
    pub file_id: String,
    pub before: String,
    pub anchors: Vec<Anchor>,
    pub edits: Vec<Edit>,
    pub fragments: Vec<FragmentRange>,
}

#[derive(Clone, Debug)]
pub struct PlannedFile {
    pub file_id: String,
    pub after: String,
    pub anchors: Vec<PlannedAnchor>,
}

/// The end of an attached anchor that a deletion cut away.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Head,
    Tail,
}

/// Text that a deletion took from an anchor. `offset` is where the text starts
/// in the deleted text, and `text` is what it covered.
///
/// With `side` of None, the anchor was wholly inside the deleted text and is
/// now detached. With a side, the deletion cut one end off an attached anchor,
/// and the anchor stays attached and shorter. `Head` is the start that was cut,
/// and `Tail` is the end. An insertion that brings the piece back at that edge
/// puts the end back, so the anchor covers its passage again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Held {
    pub id: String,
    pub offset: usize,
    pub text: String,
    pub side: Option<Side>,
}

/// A remembered deletion that an insertion brought back. `at` is the UTF-16
/// offset of its text in the insertion.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Taken {
    pub text: String,
    pub held: Vec<Held>,
    pub at: usize,
}

/// What the session deleted whole, oldest first. Each entry keeps the anchors
/// that were wholly inside the deleted text, with their offsets in it.
#[derive(Debug)]
pub struct MoveMemory {
    limit: usize,
    entries: Vec<Entry>,
}

#[derive(Debug)]
struct Entry {
    text: String,
    held: Vec<Held>,
}

impl MoveMemory {
    /// `limit` is how many deletions are remembered.
    pub fn new(limit: usize) -> Self {
        Self {
            limit,
            entries: Vec::new(),
        }
    }

    /// Remembers deleted text. The offsets in `held` are relative to `text`.
    pub fn remember(&mut self, text: &str, held: Vec<Held>) {
        if text.is_empty() || held.is_empty() {
            return;
        }
        self.entries.push(Entry {
            text: text.to_string(),
            held,
        });
        while self.entries.len() > self.limit {
            self.entries.remove(0);
        }
    }

    /// Takes the newest remembered text that occurs exactly once in `inserted`,
    /// when `free` accepts the span that the text would take there. `free` gets
    /// the span's UTF-16 start and end in `inserted`. An entry that `free`
    /// refuses stays remembered, and the scan goes on to older entries.
    pub fn take(&mut self, inserted: &str, free: impl Fn(usize, usize) -> bool) -> Option<Taken> {
        for i in (0..self.entries.len()).rev() {
            let text = self.entries[i].text.as_str();
            let Some(byte) = inserted.find(text) else {
                continue;
            };
            // Overlapping occurrences count as two: "aa" in "aaa" is ambiguous.
            let next = byte + text.chars().next().map_or(0, char::len_utf8);
            if inserted[next..].contains(text) {
                continue;
            }
            let at = utf16_len(&inserted[..byte]);
            if !free(at, at + utf16_len(text)) {
                continue;
            }
            let entry = self.entries.remove(i);
            return Some(Taken {
                text: entry.text,
                held: entry.held,
                at,
            });
        }
        None
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// The length of `s` in UTF-16 code units.
fn utf16_len(s: &str) -> usize {
    s.chars().map(char::len_utf16).sum()
}

/// The byte offset of the UTF-16 offset `units` in `s`. An offset inside a
/// surrogate pair means the character after it.
fn byte_at(s: &str, units: usize) -> usize {
    let mut at = 0;
    for (byte, ch) in s.char_indices() {
        if at >= units {
            return byte;
        }
        at += ch.len_utf16();
    }
    s.len()
}

/// The text between two UTF-16 offsets. Empty when `to` is not past `from`.
fn slice16(s: &str, from: usize, to: usize) -> &str {
    if to <= from {
        return "";
    }
    &s[byte_at(s, from)..byte_at(s, to)]
}

/// The UTF-16 offset where the last character before `to` starts.
fn last_char_start(s: &str, to: usize) -> usize {
    let mut start = 0;
    let mut at = 0;
    for ch in s.chars() {
        if at >= to {
            break;
        }
        start = at;
        at += ch.len_utf16();
    }
    start
}

/// The UTF-16 width of the character at `units`. One at the end of `s`, as in
/// the JavaScript module.
fn width_at(s: &str, units: usize) -> usize {
    s[byte_at(s, units)..]
        .chars()
        .next()
        .map_or(1, char::len_utf16)
}

/// Pure, operation-level planner. No Loro here.
///
/// Phase 1 runs each file on its own, edits in order, and moves its attached
/// anchors. Phase 2 runs after every file. Every remembered text that occurs
/// exactly once in an insertion is taken, newest first, unless its span
/// overlaps a span that an earlier take claimed in the same insertion. One
/// paste therefore claims one occurrence: a second cut of the same word waits
/// for a second paste. Each taken text re-pins the detached anchors that were
/// in it, into the file the insertion landed in, and rejoins the cut pieces of
/// attached anchors at their edges. Deleted text is remembered, and the texts
/// an insertion takes leave the memory.
///
/// Phase 1 keeps each insertion at its offset in the text as the later edits
/// leave it. A later edit before the insertion moves it, and a later edit that
/// reaches into it drops it, since its text no longer reads as one piece. Phase
/// 2 therefore reads every insertion against the final text.
pub fn plan_operation(files: &[OperationFile], memory: &mut MoveMemory) -> Vec<PlannedFile> {
    let mut work: Vec<Work> = files.iter().map(Work::new).collect();
    for w in &mut work {
        plan_file(w, memory);
    }

    for wi in 0..work.len() {
        let inserts = work[wi].inserts.clone();
        for (at, text) in inserts {
            // The spans of this insertion that a take has claimed, in the
            // insertion's own offsets. A later take may not overlap one of them.
            let mut claimed: Vec<(usize, usize)> = Vec::new();
            while let Some(hit) = memory.take(&text, |from, to| {
                !claimed.iter().any(|c| from < c.1 && to > c.0)
            }) {
                let hit_at = hit.at;
                claimed.push((hit_at, hit_at + utf16_len(&hit.text)));
                for h in hit.held {
                    if h.text.is_empty() {
                        continue;
                    }
                    let Some(oi) = work
                        .iter()
                        .position(|o| o.anchors.iter().any(|a| a.anchor.id == h.id))
                    else {
                        continue;
                    };
                    let Some(index) = work[oi].anchors.iter().position(|a| a.anchor.id == h.id)
                    else {
                        continue;
                    };
                    // A cut piece goes back only onto the attached anchor it was cut
                    // from, and only when that anchor is in this same file.
                    if let Some(side) = h.side {
                        if oi == wi {
                            let from = at + hit_at + h.offset;
                            rejoin_piece(&mut work[wi], index, side, from, &h.text);
                        }
                        continue;
                    }
                    let source = work[oi].anchors[index].clone();
                    // A record with no length has no span to bring back.
                    if source.anchor.attached() || source.anchor.length.is_none() {
                        continue;
                    }
                    let from = at + hit_at + h.offset;
                    let to = from + utf16_len(&h.text);
                    if slice16(&work[wi].after, from, to) != h.text {
                        continue;
                    }
                    work[oi].anchors.remove(index);
                    let mut pinned = source;
                    pinned.anchor.file = work[wi].file_id.clone();
                    pinned.anchor.from = from;
                    pinned.anchor.to = to;
                    pinned.anchor.detached = false;
                    pinned.anchor.fragment = None;
                    pinned.anchor.offset = None;
                    pinned.anchor.length = None;
                    pinned.anchor.collapsed = false;
                    pinned.anchor.unresolved = false;
                    pinned.state = PlanState::Set;
                    pinned.touched = true;
                    if oi != wi {
                        pinned.moved_from = Some(work[oi].file_id.clone());
                    }
                    work[wi].anchors.push(pinned);
                }
            }
        }
    }

    work.into_iter()
        .map(|w| PlannedFile {
            file_id: w.file_id,
            after: w.after,
            anchors: w.anchors,
        })
        .collect()
}

/// Puts a cut piece back onto the attached anchor it was cut from. The text must
/// be in the file at `from`, and the piece must sit at the edge of the anchor
/// that it was cut from.
fn rejoin_piece(w: &mut Work, index: usize, side: Side, from: usize, text: &str) {
    let to = from + utf16_len(text);
    if slice16(&w.after, from, to) != text {
        return;
    }
    let planned = &mut w.anchors[index];
    if !planned.anchor.attached() {
        return;
    }
    let at_edge = match side {
        Side::Head => to == planned.anchor.from,
        Side::Tail => from == planned.anchor.to,
    };
    if !at_edge {
        return;
    }
    match side {
        Side::Head => planned.anchor.from = from,
        Side::Tail => planned.anchor.to = to,
    }
    planned.state = PlanState::Set;
    planned.touched = true;
}

/// One file's state while the operation is planned.
struct Work {
    file_id: String,
    edits: Vec<Edit>,
    fragments: Vec<FragmentRange>,
    anchors: Vec<PlannedAnchor>,
    /// The text after the file's edits, once planned. Starts as the text before.
    after: String,
    /// Insertions as (UTF-16 offset, text), for phase 2.
    inserts: Vec<(usize, String)>,
}

impl Work {
    fn new(file: &OperationFile) -> Self {
        Self {
            file_id: file.file_id.clone(),
            edits: file.edits.clone(),
            fragments: file.fragments.clone(),
            anchors: file
                .anchors
                .iter()
                .map(|anchor| PlannedAnchor {
                    anchor: anchor.clone(),
                    state: PlanState::Kept,
                    touched: false,
                    moved_from: None,
                })
                .collect(),
            after: file.before.clone(),
            inserts: Vec::new(),
        }
    }
}

/// Phase 1 for one file: its edits in order, with its attached anchors moved,
/// its deleted text and the pieces cut from its anchors remembered, and its
/// insertions kept at their offsets in the text as it stands.
fn plan_file(w: &mut Work, memory: &mut MoveMemory) {
    let mut moved: HashSet<usize> = HashSet::new();
    let mut cur = w.after.clone();
    let mut adj: isize = 0;
    for (index, edit) in w.edits.iter().enumerate() {
        let ef = (edit.at as isize + adj) as usize;
        let et = ef + edit.delete;
        let n = utf16_len(&edit.insert);
        if ef == et && n == 0 {
            continue;
        }
        let (b_ef, b_et) = (byte_at(&cur, ef), byte_at(&cur, et));
        // Anchors this edit's deletion overlapped without covering them whole.
        let mut partial: HashSet<usize> = HashSet::new();
        if et > ef {
            let deleted = cur[b_ef..b_et].to_string();
            let mut held: Vec<Held> = Vec::new();
            for (i, pa) in w.anchors.iter_mut().enumerate() {
                if !pa.anchor.attached() {
                    continue;
                }
                let (from, to) = (pa.anchor.from, pa.anchor.to);
                if ef <= from && et >= to {
                    pa.touched = true;
                    let fragment = w
                        .fragments
                        .iter()
                        .find(|f| f.edit == index && from - ef >= f.start && to - ef <= f.end);
                    held.push(Held {
                        id: pa.anchor.id.clone(),
                        offset: from - ef,
                        text: slice16(&cur, from, to).to_string(),
                        side: None,
                    });
                    // The quote keys stay as they are. The length tells a later
                    // re-pin how much text to bring back.
                    pa.anchor.detached = true;
                    pa.anchor.from = 0;
                    pa.anchor.to = 0;
                    pa.anchor.length = Some(to - from);
                    pa.anchor.fragment = fragment.map(|f| f.id.clone());
                    pa.anchor.offset = fragment.map(|f| from - ef - f.start);
                    pa.state = PlanState::Set;
                } else {
                    if from < et && to > ef {
                        partial.insert(i);
                        pa.touched = true;
                        // A cut at one end remembers the piece it took, so that an
                        // insertion bringing it back at that edge rejoins the anchor.
                        // A cut strictly inside the anchor remembers nothing.
                        if ef <= from && et < to {
                            held.push(Held {
                                id: pa.anchor.id.clone(),
                                offset: from - ef,
                                text: slice16(&cur, from, et).to_string(),
                                side: Some(Side::Head),
                            });
                        } else if ef > from && et >= to {
                            held.push(Held {
                                id: pa.anchor.id.clone(),
                                offset: 0,
                                text: slice16(&cur, ef, to).to_string(),
                                side: Some(Side::Tail),
                            });
                        }
                    }
                    pa.anchor.from = after_delete(from, ef, et);
                    pa.anchor.to = after_delete(to, ef, et);
                    if pa.anchor.from != from || pa.anchor.to != to {
                        moved.insert(i);
                    }
                }
            }
            memory.remember(&deleted, held);
        }
        // Insertions that earlier edits made move past this edit. One that this
        // edit reaches into is dropped, since its text no longer reads as one piece.
        follow_edit(&mut w.inserts, ef, et, n);
        if n > 0 {
            for (i, pa) in w.anchors.iter_mut().enumerate() {
                if !pa.anchor.attached() {
                    continue;
                }
                let a = &mut pa.anchor;
                // An anchor the deletion overlapped takes the new text inside it.
                // Otherwise the start side is Left: an insertion at the start
                // moves the anchor. The end side is Right: an insertion at the
                // end stays outside.
                if partial.contains(&i) {
                    a.to += n;
                    moved.insert(i);
                } else if ef <= a.from {
                    a.from += n;
                    a.to += n;
                    moved.insert(i);
                } else if ef < a.to {
                    a.to += n;
                    moved.insert(i);
                }
            }
            w.inserts.push((ef, edit.insert.clone()));
        }
        cur.replace_range(b_ef..b_et, &edit.insert);
        adj += n as isize - (et - ef) as isize;
    }
    w.after = cur;
    // A moved anchor is written again so that its cursors are checked. Its quote
    // keys stay as they are: a change inside the passage must show as modified.
    for i in moved {
        let pa = &mut w.anchors[i];
        if pa.anchor.attached() {
            pa.state = PlanState::Set;
        }
    }
}

/// Moves the insertions that earlier edits made past an edit that deletes
/// `[ef, et)` and inserts `n` units. An insertion that this edit reaches into is
/// dropped, since its text no longer reads as one piece.
fn follow_edit(inserts: &mut Vec<(usize, String)>, ef: usize, et: usize, n: usize) {
    inserts.retain_mut(|(at, text)| {
        if et <= *at {
            *at = (*at as isize + n as isize - (et - ef) as isize) as usize;
            true
        } else {
            ef >= *at + utf16_len(text.as_str())
        }
    });
}

/// Where a UTF-16 position goes when [ef, et) is deleted.
fn after_delete(p: usize, ef: usize, et: usize) -> usize {
    if p < ef {
        p
    } else if p >= et {
        p - (et - ef)
    } else {
        ef
    }
}

// ---------------------------------------------------------------------------
// Loro layer

/// The text at `file_id`, when the file is a text.
fn text_of(doc: &LoroDoc, file_id: &str) -> Option<LoroText> {
    match doc.get_map(FILES).get(file_id)? {
        ValueOrContainer::Container(Container::Text(t)) => Some(t),
        _ => None,
    }
}

/// A file's text and its body, read once for a pass over its records.
struct Live {
    text: LoroText,
    body: String,
}

fn live_of(doc: &LoroDoc, file_id: &str) -> Option<Live> {
    let text = text_of(doc, file_id)?;
    let body = text.to_string();
    Some(Live { text, body })
}

/// A cursor on the character at the UTF-16 offset `units`, or None when the
/// offset is outside the text.
fn cursor_at(text: &LoroText, units: usize, side: CursorSide) -> Option<Cursor> {
    let pos = text.convert_pos(units, PosType::Utf16, PosType::Unicode)?;
    text.get_cursor(pos, side)
}

/// A cursor resolved to its UTF-16 offset. `stale` when Loro moved it because
/// its character was deleted.
struct Pos {
    offset: usize,
    stale: bool,
}

/// Resolves encoded cursor bytes in `text`. A cursor made for another text does
/// not resolve here.
fn cursor_pos(doc: &LoroDoc, text: &LoroText, bytes: &[u8]) -> Option<Pos> {
    let cursor = Cursor::decode(bytes).ok()?;
    if cursor.container != text.id() {
        return None;
    }
    let found = doc.get_cursor_pos(&cursor).ok()?;
    let offset = text.convert_pos(found.current.pos, PosType::Unicode, PosType::Utf16)?;
    Some(Pos {
        offset,
        stale: found.update.is_some(),
    })
}

/// The span a record's cursors resolve to now. A live end sits on the last
/// character of the span, and a stale end is already the exclusive end.
fn span_of(doc: &LoroDoc, live: &Live, record: &LoroMap) -> Option<(usize, usize)> {
    let s = cursor_pos(doc, &live.text, &bytes_of(record, "start")?)?;
    let e = cursor_pos(doc, &live.text, &bytes_of(record, "end")?)?;
    let to = if e.stale {
        e.offset
    } else {
        e.offset + width_at(&live.body, e.offset)
    };
    Some((s.offset, to.max(s.offset)))
}

/// The record `id` of the anchors root, when it is a map container.
fn record_map(root: &LoroMap, id: &str) -> Option<LoroMap> {
    match root.get(id)? {
        ValueOrContainer::Container(Container::Map(m)) => Some(m),
        _ => None,
    }
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

/// A count field. Browsers write integers, which Loro may store as I64 or as
/// Double.
fn count_of(m: &LoroMap, key: &str) -> Option<usize> {
    match m.get(key)? {
        ValueOrContainer::Value(LoroValue::I64(v)) => usize::try_from(v).ok(),
        ValueOrContainer::Value(LoroValue::Double(v)) if v >= 0.0 && v.fract() == 0.0 => {
            Some(v as usize)
        }
        _ => None,
    }
}

fn is_true(m: &LoroMap, key: &str) -> bool {
    matches!(
        m.get(key),
        Some(ValueOrContainer::Value(LoroValue::Bool(true)))
    )
}

/// The fields every record carries, read without resolving its cursors.
fn record_fields(record: &LoroMap, id: &str, file_id: &str) -> Anchor {
    Anchor {
        id: id.to_string(),
        file: file_id.to_string(),
        from: 0,
        to: 0,
        detached: false,
        fragment: str_of(record, "fragment"),
        offset: count_of(record, "offset"),
        length: count_of(record, "length"),
        exact: str_of(record, "exact").unwrap_or_default(),
        prefix: str_of(record, "prefix").unwrap_or_default(),
        suffix: str_of(record, "suffix").unwrap_or_default(),
        collapsed: false,
        unresolved: false,
    }
}

/// A detached record as an anchor. Its cursors are not resolved.
fn detached_anchor(record: &LoroMap, id: &str, file_id: &str) -> Anchor {
    let mut anchor = record_fields(record, id, file_id);
    anchor.detached = true;
    anchor
}

/// One record as an anchor. Its cursors are resolved against `live`, when the
/// file has text. A detached record is returned without resolving them.
fn record_anchor(
    doc: &LoroDoc,
    id: &str,
    record: &LoroMap,
    file_id: &str,
    live: Option<&Live>,
) -> Anchor {
    if is_true(record, "detached") {
        return detached_anchor(record, id, file_id);
    }
    let mut anchor = record_fields(record, id, file_id);
    match live.and_then(|live| span_of(doc, live, record)) {
        Some((from, to)) => {
            anchor.from = from;
            anchor.to = to;
            anchor.collapsed = to <= from;
        }
        None => anchor.unresolved = true,
    }
    anchor
}

/// The anchor records of one file, resolved to offsets. Detached records are
/// returned without resolving their cursors.
pub fn resolve_anchors(doc: &LoroDoc, file_id: &str) -> Vec<Anchor> {
    let live = live_of(doc, file_id);
    let map = doc.get_map(ANCHORS);
    let mut out = Vec::new();
    for key in map.keys() {
        let id = key.to_string();
        let Some(record) = record_map(&map, &id) else {
            continue;
        };
        if str_of(&record, "file").as_deref() != Some(file_id) {
            continue;
        }
        out.push(record_anchor(doc, &id, &record, file_id, live.as_ref()));
    }
    out
}

/// One anchor record, read by its comment id and resolved as
/// [`resolve_anchors`] resolves it. None when there is no such record.
pub fn resolve_anchor(doc: &LoroDoc, id: &str) -> Option<Anchor> {
    let map = doc.get_map(ANCHORS);
    let record = record_map(&map, id)?;
    let file_id = str_of(&record, "file")?;
    let live = live_of(doc, &file_id);
    Some(record_anchor(doc, id, &record, &file_id, live.as_ref()))
}

/// Creates an attached anchor record. Does not commit. Returns false when the
/// file has no text, `to` is not past `from`, or a cursor cannot be made.
#[allow(clippy::too_many_arguments)]
pub fn create_anchor(
    doc: &LoroDoc,
    file_id: &str,
    id: &str,
    from: usize,
    to: usize,
    exact: &str,
    prefix: &str,
    suffix: &str,
) -> bool {
    let Some((start, end)) = capture_cursors(doc, file_id, from, to) else {
        return false;
    };
    create_anchor_with_cursors(doc, file_id, id, &start, &end, exact, prefix, suffix)
}

/// The pair of cursors a record holds for the passage `from..to`: the start on
/// its first character and the end on its last, which is how the browser's
/// planner writes them too. Does not commit. None when the file has no text,
/// `to` is not past `from`, or a cursor cannot be made.
pub fn capture_cursors(
    doc: &LoroDoc,
    file_id: &str,
    from: usize,
    to: usize,
) -> Option<(Vec<u8>, Vec<u8>)> {
    let live = live_of(doc, file_id)?;
    if to <= from {
        return None;
    }
    let start = cursor_at(&live.text, from, CursorSide::Left)?;
    let end = cursor_at(
        &live.text,
        last_char_start(&live.body, to),
        CursorSide::Right,
    )?;
    Some((start.encode(), end.encode()))
}

/// Creates an attached anchor record from cursors that are already encoded,
/// such as the ones a comment's frontier gives when it is captured. Does not
/// commit. Returns false when the record cannot be made.
#[allow(clippy::too_many_arguments)]
pub fn create_anchor_with_cursors(
    doc: &LoroDoc,
    file_id: &str,
    id: &str,
    start: &[u8],
    end: &[u8],
    exact: &str,
    prefix: &str,
    suffix: &str,
) -> bool {
    doc.set_next_commit_origin(ANCHORS_ORIGIN);
    let Ok(record) = doc.get_map(ANCHORS).insert_container(id, LoroMap::new()) else {
        return false;
    };
    record.insert("file", LoroValue::from(file_id)).ok();
    record.insert("start", LoroValue::from(start.to_vec())).ok();
    record.insert("end", LoroValue::from(end.to_vec())).ok();
    record.insert("exact", LoroValue::from(exact)).ok();
    record.insert("prefix", LoroValue::from(prefix)).ok();
    record.insert("suffix", LoroValue::from(suffix)).ok();
    true
}

/// Creates a detached anchor record: the file and the quote, with no cursors
/// and no length. The passage is taken to be gone, so nothing can re-pin it.
/// Does not commit. Returns false when the record cannot be made.
pub fn create_detached_anchor(
    doc: &LoroDoc,
    file_id: &str,
    id: &str,
    exact: &str,
    prefix: &str,
    suffix: &str,
) -> bool {
    doc.set_next_commit_origin(ANCHORS_ORIGIN);
    let Ok(record) = doc.get_map(ANCHORS).insert_container(id, LoroMap::new()) else {
        return false;
    };
    record.insert("file", LoroValue::from(file_id)).ok();
    record.insert("detached", LoroValue::Bool(true)).ok();
    record.insert("exact", LoroValue::from(exact)).ok();
    record.insert("prefix", LoroValue::from(prefix)).ok();
    record.insert("suffix", LoroValue::from(suffix)).ok();
    true
}

/// A pending record write, read in pass 1 of `write_anchors`.
struct Pending {
    id: String,
    record: LoroMap,
    sets: Vec<(&'static str, LoroValue)>,
    deletes: Vec<&'static str>,
}

/// Queues `key` to be set to `value`, unless the record holds that value.
fn put(
    sets: &mut Vec<(&'static str, LoroValue)>,
    record: &LoroMap,
    key: &'static str,
    value: LoroValue,
) {
    if !matches!(record.get(key), Some(ValueOrContainer::Value(v)) if v == value) {
        sets.push((key, value));
    }
}

/// Queues `key` to be deleted, when the record holds it.
fn drop_key(deletes: &mut Vec<&'static str>, record: &LoroMap, key: &'static str) {
    if record.get(key).is_some() {
        deletes.push(key);
    }
}

/// Writes the anchors marked set in a plan. A detached record keeps its
/// cursors. An attached anchor that was touched gets both cursors, fresh. One
/// that only shifted (touched false) is skipped: its cursors follow its text.
/// Does not commit. Returns the ids written.
pub fn write_anchors(doc: &LoroDoc, planned: &[PlannedFile]) -> Vec<String> {
    let map = doc.get_map(ANCHORS);
    // Pass 1 reads everything and works out the writes. Resolving a stale
    // cursor commits pending operations, so no read may come after the first
    // write.
    let mut pending: Vec<Pending> = Vec::new();
    for file in planned {
        let live = live_of(doc, &file.file_id);
        for pa in &file.anchors {
            let a = &pa.anchor;
            if pa.state != PlanState::Set || (!a.detached && !pa.touched) {
                continue;
            }
            let Some(record) = record_map(&map, &a.id) else {
                continue;
            };
            let mut sets = Vec::new();
            let mut deletes = Vec::new();
            if a.detached {
                put(
                    &mut sets,
                    &record,
                    "file",
                    LoroValue::from(file.file_id.as_str()),
                );
                put(&mut sets, &record, "detached", LoroValue::Bool(true));
                match a.length {
                    Some(length) => {
                        put(&mut sets, &record, "length", LoroValue::I64(length as i64))
                    }
                    None => drop_key(&mut deletes, &record, "length"),
                }
                match &a.fragment {
                    Some(fragment) => put(
                        &mut sets,
                        &record,
                        "fragment",
                        LoroValue::from(fragment.as_str()),
                    ),
                    None => drop_key(&mut deletes, &record, "fragment"),
                }
                match a.offset {
                    Some(offset) => {
                        put(&mut sets, &record, "offset", LoroValue::I64(offset as i64))
                    }
                    None => drop_key(&mut deletes, &record, "offset"),
                }
            } else {
                let Some(live) = live.as_ref() else {
                    continue;
                };
                if a.to <= a.from {
                    continue;
                }
                let (Some(start), Some(end)) = (
                    cursor_at(&live.text, a.from, CursorSide::Left),
                    cursor_at(
                        &live.text,
                        last_char_start(&live.body, a.to),
                        CursorSide::Right,
                    ),
                ) else {
                    continue;
                };
                put(
                    &mut sets,
                    &record,
                    "file",
                    LoroValue::from(file.file_id.as_str()),
                );
                sets.push(("start", LoroValue::from(start.encode())));
                sets.push(("end", LoroValue::from(end.encode())));
                drop_key(&mut deletes, &record, "detached");
                drop_key(&mut deletes, &record, "length");
                drop_key(&mut deletes, &record, "fragment");
                drop_key(&mut deletes, &record, "offset");
            }
            pending.push(Pending {
                id: a.id.clone(),
                record,
                sets,
                deletes,
            });
        }
    }
    // Pass 2 writes. The commit origin is set before the first write.
    let dirty = pending
        .iter()
        .any(|p| !p.sets.is_empty() || !p.deletes.is_empty());
    if dirty {
        doc.set_next_commit_origin(ANCHORS_ORIGIN);
    }
    for p in &pending {
        for (key, value) in &p.sets {
            p.record.insert(key, value.clone()).ok();
        }
        for key in &p.deletes {
            p.record.delete(key).ok();
        }
    }
    pending.into_iter().map(|p| p.id).collect()
}

/// Marks the attached anchors of a file whose cursors resolve collapsed as
/// detached. Their quotes stay as they are, and no length is recorded: the span
/// is gone, so nothing can be re-pinned. Does not commit. Returns the ids marked.
pub fn mark_collapsed(doc: &LoroDoc, file_id: &str) -> Vec<String> {
    let map = doc.get_map(ANCHORS);
    // Reads come first. The commit origin is set before the first write.
    let mut marked: Vec<(String, LoroMap)> = Vec::new();
    for a in resolve_anchors(doc, file_id) {
        if !a.collapsed {
            continue;
        }
        if let Some(record) = record_map(&map, &a.id) {
            marked.push((a.id, record));
        }
    }
    if !marked.is_empty() {
        doc.set_next_commit_origin(ANCHORS_ORIGIN);
    }
    for (_, record) in &marked {
        record.insert("detached", LoroValue::Bool(true)).ok();
    }
    marked.into_iter().map(|(id, _)| id).collect()
}

/// The id of every text in the files map. A text is listed whether or not a
/// path still names it, since a record can point at a text no path names.
pub fn text_file_ids(doc: &LoroDoc) -> Vec<String> {
    let files = doc.get_map(FILES);
    files
        .keys()
        .map(|key| key.to_string())
        .filter(|id| {
            matches!(
                files.get(id.as_str()),
                Some(ValueOrContainer::Container(Container::Text(_)))
            )
        })
        .collect()
}

/// Whether either cursor of a record resolves onto a deleted character.
fn has_stale_cursor(doc: &LoroDoc, live: &Live, record: &LoroMap) -> bool {
    ["start", "end"].into_iter().any(|key| {
        bytes_of(record, key)
            .and_then(|bytes| cursor_pos(doc, &live.text, &bytes))
            .is_some_and(|pos| pos.stale)
    })
}

/// Heals the attached anchors of a file before a shallow snapshot drops the
/// history their cursors resolve through. A record with a cursor on a deleted
/// character gets fresh cursors on the span it resolves to now. A record whose
/// span has collapsed is marked detached, as the next edit would mark it, since
/// no cursor into the deleted text survives the snapshot. Quotes are kept. Reads
/// come first. Does not commit. Returns the ids changed.
pub fn heal_stale(doc: &LoroDoc, file_id: &str) -> Vec<String> {
    let Some(live) = live_of(doc, file_id) else {
        return Vec::new();
    };
    let map = doc.get_map(ANCHORS);
    let mut fresh: Vec<(String, LoroMap, Vec<u8>, Vec<u8>)> = Vec::new();
    let mut collapsed: Vec<(String, LoroMap)> = Vec::new();
    for a in resolve_anchors(doc, file_id) {
        if a.detached || a.unresolved {
            continue;
        }
        let Some(record) = record_map(&map, &a.id) else {
            continue;
        };
        if a.collapsed {
            collapsed.push((a.id, record));
            continue;
        }
        if !has_stale_cursor(doc, &live, &record) {
            continue;
        }
        let (Some(start), Some(end)) = (
            cursor_at(&live.text, a.from, CursorSide::Left),
            cursor_at(
                &live.text,
                last_char_start(&live.body, a.to),
                CursorSide::Right,
            ),
        ) else {
            continue;
        };
        fresh.push((a.id, record, start.encode(), end.encode()));
    }
    // Every read is done. The commit origin is set before the first write.
    if !fresh.is_empty() || !collapsed.is_empty() {
        doc.set_next_commit_origin(ANCHORS_ORIGIN);
    }
    let mut ids = Vec::with_capacity(fresh.len() + collapsed.len());
    for (id, record, start, end) in fresh {
        record.insert("start", LoroValue::from(start)).ok();
        record.insert("end", LoroValue::from(end)).ok();
        ids.push(id);
    }
    for (id, record) in collapsed {
        record.insert("detached", LoroValue::Bool(true)).ok();
        ids.push(id);
    }
    ids
}

/// The detached anchors of the files a write does not name. Each file comes
/// back as a pseudo-file with no edits, so that an insertion in the named files
/// can re-pin them.
fn detached_elsewhere(doc: &LoroDoc, named: &[OperationFile]) -> Vec<OperationFile> {
    let map = doc.get_map(ANCHORS);
    let mut groups: Vec<(String, Vec<Anchor>)> = Vec::new();
    for key in map.keys() {
        let id = key.to_string();
        let Some(record) = record_map(&map, &id) else {
            continue;
        };
        let Some(file_id) = str_of(&record, "file") else {
            continue;
        };
        if !is_true(&record, "detached") || named.iter().any(|f| f.file_id == file_id) {
            continue;
        }
        let anchor = detached_anchor(&record, &id, &file_id);
        match groups.iter().position(|group| group.0 == file_id) {
            Some(at) => groups[at].1.push(anchor),
            None => groups.push((file_id, vec![anchor])),
        }
    }
    groups
        .into_iter()
        .map(|(file_id, anchors)| {
            let before = live_of(doc, &file_id)
                .map(|live| live.body)
                .unwrap_or_default();
            OperationFile {
                file_id,
                before,
                anchors,
                edits: Vec::new(),
                fragments: Vec::new(),
            }
        })
        .collect()
}

/// Plans and writes an operation whose text is already edited. The anchors in
/// `files` are the ones resolved before the edits. An anchor that arrived
/// collapsed is treated as detached and written detached. Detached anchors of
/// files this operation does not name join the plan with no edits, so that an
/// insertion here can re-pin them. Those come from `others` when it is given,
/// and otherwise from a scan of the document. Does not commit.
pub fn record_operation(
    doc: &LoroDoc,
    files: &[OperationFile],
    memory: &mut MoveMemory,
    others: Option<Vec<OperationFile>>,
) -> Vec<PlannedFile> {
    let mut collapsed: HashSet<String> = HashSet::new();
    let mut input: Vec<OperationFile> = Vec::with_capacity(files.len());
    for file in files {
        let anchors: Vec<Anchor> = file
            .anchors
            .iter()
            .map(|a| {
                let mut a = a.clone();
                if !a.detached && !a.unresolved && a.collapsed {
                    collapsed.insert(a.id.clone());
                    a.detached = true;
                    a.collapsed = false;
                    a.from = 0;
                    a.to = 0;
                    a.length = None;
                }
                a
            })
            .collect();
        input.push(OperationFile {
            file_id: file.file_id.clone(),
            before: file.before.clone(),
            anchors,
            edits: file.edits.clone(),
            fragments: file.fragments.clone(),
        });
    }
    match others {
        Some(others) => input.extend(others),
        None => input.extend(detached_elsewhere(doc, files)),
    }
    let mut plan = plan_operation(&input, memory);
    for file in &mut plan {
        for pa in &mut file.anchors {
            if collapsed.contains(&pa.anchor.id) {
                pa.state = PlanState::Set;
            }
        }
    }
    write_anchors(doc, &plan);
    plan
}

/// What a reader shows for an anchor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Exact,
    Modified,
    PendingDeletion,
    Deleted,
    Unresolved,
}

/// The status of an anchor, from its record and the text now.
pub fn status_of(doc: &LoroDoc, anchor: &Anchor) -> Status {
    if anchor.unresolved {
        return Status::Unresolved;
    }
    if anchor.detached {
        let pending = anchor.fragment.as_deref().is_some_and(|fragment| {
            matches!(
                doc.get_map(CHANGES).get(fragment),
                Some(ValueOrContainer::Container(Container::Map(_)))
            )
        });
        return if pending {
            Status::PendingDeletion
        } else {
            Status::Deleted
        };
    }
    if anchor.collapsed {
        return Status::Deleted;
    }
    let body = text_of(doc, &anchor.file)
        .map(|t| t.to_string())
        .unwrap_or_default();
    if slice16(&body, anchor.from, anchor.to) == anchor.exact {
        Status::Exact
    } else {
        Status::Modified
    }
}

/// Text that a rejected tracked deletion put back. `at` is its UTF-16 offset
/// in the file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Restored {
    pub fragment: String,
    pub at: usize,
    pub text: String,
}

/// Re-pins the anchors whose fragment was restored as new text: a rejected
/// tracked deletion put its text back. The restored text must already be in the
/// document. Does not commit. Returns the ids re-pinned.
pub fn restore_by_fragments(doc: &LoroDoc, file_id: &str, restored: &[Restored]) -> Vec<String> {
    let Some(live) = live_of(doc, file_id) else {
        return Vec::new();
    };
    if restored.is_empty() {
        return Vec::new();
    }
    let by_fragment: HashMap<&str, &Restored> =
        restored.iter().map(|r| (r.fragment.as_str(), r)).collect();
    let map = doc.get_map(ANCHORS);
    // Reads come first. The commit origin is set before the first write.
    let mut repins: Vec<(String, LoroMap, Vec<u8>, Vec<u8>)> = Vec::new();
    for key in map.keys() {
        let id = key.to_string();
        let Some(record) = record_map(&map, &id) else {
            continue;
        };
        if str_of(&record, "file").as_deref() != Some(file_id) || !is_true(&record, "detached") {
            continue;
        }
        let Some(r) = str_of(&record, "fragment")
            .and_then(|fragment| by_fragment.get(fragment.as_str()).copied())
        else {
            continue;
        };
        let (Some(offset), Some(length)) =
            (count_of(&record, "offset"), count_of(&record, "length"))
        else {
            continue;
        };
        let from = r.at + offset;
        let to = (from + length).min(r.at + utf16_len(&r.text));
        if to <= from {
            continue;
        }
        let (Some(start), Some(end)) = (
            cursor_at(&live.text, from, CursorSide::Left),
            cursor_at(
                &live.text,
                last_char_start(&live.body, to),
                CursorSide::Right,
            ),
        ) else {
            continue;
        };
        repins.push((id, record, start.encode(), end.encode()));
    }
    if !repins.is_empty() {
        doc.set_next_commit_origin(ANCHORS_ORIGIN);
    }
    let mut ids = Vec::with_capacity(repins.len());
    for (id, record, start, end) in repins {
        record.insert("start", LoroValue::from(start)).ok();
        record.insert("end", LoroValue::from(end)).ok();
        record.delete("detached").ok();
        record.delete("length").ok();
        record.delete("fragment").ok();
        record.delete("offset").ok();
        ids.push(id);
    }
    ids
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;
    use serde_json::{json, Value};

    use super::*;
    use crate::document::session::{apply_text_edits, new_doc, put_text};
    use crate::document::track::{self, Author};

    const FIXTURES: &str = include_str!("../../../../web/tests/fixtures/anchors.json");
    const CUT: &str = "beta ";

    #[derive(Deserialize)]
    struct FxAnchor {
        id: String,
        file: Option<String>,
        #[serde(default)]
        from: usize,
        #[serde(default)]
        to: usize,
        #[serde(default)]
        detached: bool,
        #[serde(default)]
        fragment: Option<String>,
        #[serde(default)]
        offset: Option<usize>,
        #[serde(default)]
        length: Option<usize>,
        #[serde(default)]
        exact: String,
        #[serde(default)]
        prefix: String,
        #[serde(default)]
        suffix: String,
        #[serde(default)]
        collapsed: bool,
        #[serde(default)]
        unresolved: bool,
    }

    #[derive(Deserialize)]
    struct FxEdit {
        from: usize,
        to: usize,
        insert: String,
    }

    #[derive(Deserialize)]
    struct FxFragment {
        id: String,
        edit: usize,
        start: usize,
        end: usize,
    }

    #[derive(Deserialize)]
    struct FxFile {
        #[serde(rename = "fileId")]
        file_id: String,
        before: String,
        anchors: Vec<FxAnchor>,
        edits: Vec<FxEdit>,
        #[serde(default)]
        fragments: Vec<FxFragment>,
    }

    #[derive(Deserialize)]
    struct FxHeld {
        id: String,
        offset: usize,
        text: String,
        /// "head" or "tail" for a cut piece; absent for a whole anchor.
        #[serde(default)]
        side: Option<String>,
    }

    #[derive(Deserialize)]
    struct FxMemory {
        text: String,
        held: Vec<FxHeld>,
    }

    #[derive(Deserialize)]
    struct FxExpectFile {
        #[serde(rename = "fileId")]
        file_id: String,
        after: String,
        /// Partial anchors: only the keys a fixture names are compared.
        anchors: Vec<serde_json::Map<String, Value>>,
    }

    #[derive(Deserialize)]
    struct FxExpect {
        files: Vec<FxExpectFile>,
        #[serde(rename = "memorySize")]
        memory_size: Option<usize>,
    }

    #[derive(Deserialize)]
    struct Fixture {
        name: String,
        files: Vec<FxFile>,
        #[serde(default)]
        memory: Vec<FxMemory>,
        expect: FxExpect,
    }

    /// The side of a cut piece, spelled as the fixtures spell it.
    fn fixture_side(side: &str) -> Side {
        match side {
            "head" => Side::Head,
            "tail" => Side::Tail,
            other => panic!("no held side {other}"),
        }
    }

    /// One field of a planned anchor, spelled as the fixtures spell it.
    fn field(a: &PlannedAnchor, key: &str) -> Value {
        let an = &a.anchor;
        match key {
            "id" => json!(an.id),
            "file" => json!(an.file),
            "from" => json!(an.from),
            "to" => json!(an.to),
            "detached" => json!(an.detached),
            "fragment" => json!(an.fragment),
            "offset" => json!(an.offset),
            "length" => json!(an.length),
            "exact" => json!(an.exact),
            "prefix" => json!(an.prefix),
            "suffix" => json!(an.suffix),
            "collapsed" => json!(an.collapsed),
            "unresolved" => json!(an.unresolved),
            "state" => {
                let state = match a.state {
                    PlanState::Kept => "kept",
                    PlanState::Set => "set",
                };
                json!(state)
            }
            "movedFrom" => json!(a.moved_from),
            other => panic!("no anchor field {other}"),
        }
    }

    #[test]
    fn shared_fixtures() {
        let fixtures: Vec<Fixture> = serde_json::from_str(FIXTURES).unwrap();
        assert!(!fixtures.is_empty());
        for fx in fixtures {
            let name = fx.name.as_str();
            let files: Vec<OperationFile> = fx
                .files
                .iter()
                .map(|file| OperationFile {
                    file_id: file.file_id.clone(),
                    before: file.before.clone(),
                    anchors: file
                        .anchors
                        .iter()
                        .map(|a| Anchor {
                            id: a.id.clone(),
                            file: a.file.clone().unwrap_or_else(|| file.file_id.clone()),
                            from: a.from,
                            to: a.to,
                            detached: a.detached,
                            fragment: a.fragment.clone(),
                            offset: a.offset,
                            length: a.length,
                            exact: a.exact.clone(),
                            prefix: a.prefix.clone(),
                            suffix: a.suffix.clone(),
                            collapsed: a.collapsed,
                            unresolved: a.unresolved,
                        })
                        .collect(),
                    edits: file
                        .edits
                        .iter()
                        .map(|e| Edit {
                            at: e.from,
                            delete: e.to - e.from,
                            insert: e.insert.clone(),
                        })
                        .collect(),
                    fragments: file
                        .fragments
                        .iter()
                        .map(|f| FragmentRange {
                            id: f.id.clone(),
                            edit: f.edit,
                            start: f.start,
                            end: f.end,
                        })
                        .collect(),
                })
                .collect();

            let mut memory = MoveMemory::new(32);
            for entry in &fx.memory {
                let held: Vec<Held> = entry
                    .held
                    .iter()
                    .map(|h| Held {
                        id: h.id.clone(),
                        offset: h.offset,
                        text: h.text.clone(),
                        side: h.side.as_deref().map(fixture_side),
                    })
                    .collect();
                memory.remember(&entry.text, held);
            }

            let planned = plan_operation(&files, &mut memory);
            let got_order: Vec<&str> = planned.iter().map(|f| f.file_id.as_str()).collect();
            let want_order: Vec<&str> =
                fx.expect.files.iter().map(|f| f.file_id.as_str()).collect();
            assert_eq!(got_order, want_order, "{name}: file order");

            for want in &fx.expect.files {
                let got = planned
                    .iter()
                    .find(|f| f.file_id == want.file_id)
                    .unwrap_or_else(|| panic!("{name}: no file {}", want.file_id));
                assert_eq!(got.after, want.after, "{name}: {} after text", want.file_id);

                let mut got_ids: Vec<&str> =
                    got.anchors.iter().map(|a| a.anchor.id.as_str()).collect();
                let mut want_ids: Vec<&str> = want
                    .anchors
                    .iter()
                    .map(|a| a.get("id").and_then(Value::as_str).unwrap())
                    .collect();
                got_ids.sort_unstable();
                want_ids.sort_unstable();
                assert_eq!(got_ids, want_ids, "{name}: {} anchor ids", want.file_id);

                for want_anchor in &want.anchors {
                    let id = want_anchor.get("id").and_then(Value::as_str).unwrap();
                    let got_anchor = got.anchors.iter().find(|a| a.anchor.id == id).unwrap();
                    for (key, value) in want_anchor {
                        assert_eq!(field(got_anchor, key), *value, "{name}: {id}.{key}");
                    }
                }
            }

            if let Some(size) = fx.expect.memory_size {
                assert_eq!(memory.len(), size, "{name}: memory size");
            }
        }
    }

    /// An attached anchor with no quote, for the planner tests.
    fn anchor_at(id: &str, file: &str, from: usize, to: usize) -> Anchor {
        Anchor {
            id: id.to_string(),
            file: file.to_string(),
            from,
            to,
            detached: false,
            fragment: None,
            offset: None,
            length: None,
            exact: String::new(),
            prefix: String::new(),
            suffix: String::new(),
            collapsed: false,
            unresolved: false,
        }
    }

    #[test]
    fn the_head_of_a_passage_cut_and_put_back_rejoins_it() {
        let mut memory = MoveMemory::new(32);
        let cut = OperationFile {
            file_id: "main.md".to_string(),
            before: "one two three".to_string(),
            anchors: vec![anchor_at("c1", "main.md", 4, 7)],
            edits: vec![Edit {
                at: 4,
                delete: 2,
                insert: String::new(),
            }],
            fragments: Vec::new(),
        };
        let first = plan_operation(&[cut], &mut memory);
        assert_eq!(first[0].after, "one o three");
        let head = &first[0].anchors[0];
        assert_eq!((head.anchor.from, head.anchor.to), (4, 5));
        assert!(!head.anchor.detached);
        assert_eq!(memory.len(), 1);

        let put_back = OperationFile {
            file_id: "main.md".to_string(),
            before: first[0].after.clone(),
            anchors: vec![head.anchor.clone()],
            edits: vec![Edit {
                at: 4,
                delete: 0,
                insert: "tw".to_string(),
            }],
            fragments: Vec::new(),
        };
        let planned = plan_operation(&[put_back], &mut memory);
        let rejoined = &planned[0].anchors[0];
        assert_eq!(planned[0].after, "one two three");
        assert_eq!((rejoined.anchor.from, rejoined.anchor.to), (4, 7));
        assert_eq!(slice16(&planned[0].after, 4, 7), "two");
        assert!(!rejoined.anchor.detached);
        assert_eq!(rejoined.state, PlanState::Set);
        assert!(rejoined.touched);
        assert!(memory.is_empty());
    }

    #[test]
    fn a_composed_paste_followed_by_an_insertion_before_it_still_repins() {
        let mut memory = MoveMemory::new(32);
        let cut = OperationFile {
            file_id: "main.md".to_string(),
            before: "one two three".to_string(),
            anchors: vec![anchor_at("c1", "main.md", 4, 7)],
            edits: vec![Edit {
                at: 4,
                delete: 4,
                insert: String::new(),
            }],
            fragments: Vec::new(),
        };
        let first = plan_operation(&[cut], &mut memory);
        assert!(first[0].anchors[0].anchor.detached);
        assert_eq!(memory.len(), 1);

        // The insertion comes second in the list and lands before the paste, so
        // the paste's entry has to follow it.
        let paste = OperationFile {
            file_id: "main.md".to_string(),
            before: first[0].after.clone(),
            anchors: vec![first[0].anchors[0].anchor.clone()],
            edits: vec![
                Edit {
                    at: 9,
                    delete: 0,
                    insert: "two ".to_string(),
                },
                Edit {
                    at: 0,
                    delete: 0,
                    insert: "XX".to_string(),
                },
            ],
            fragments: Vec::new(),
        };
        let planned = plan_operation(&[paste], &mut memory);
        let repinned = &planned[0].anchors[0];
        assert!(!repinned.anchor.detached);
        assert_eq!((repinned.anchor.from, repinned.anchor.to), (11, 14));
        assert_eq!(slice16(&planned[0].after, 11, 14), "two");
        assert_eq!(repinned.state, PlanState::Set);
        assert!(repinned.touched);
        assert!(memory.is_empty());
    }

    #[test]
    fn a_merged_undo_of_two_backspaces_rejoins_both_pieces() {
        let mut memory = MoveMemory::new(32);
        let first = OperationFile {
            file_id: "main.md".to_string(),
            before: "one two three".to_string(),
            anchors: vec![anchor_at("c1", "main.md", 4, 7)],
            edits: vec![Edit {
                at: 4,
                delete: 1,
                insert: String::new(),
            }],
            fragments: Vec::new(),
        };
        let one = plan_operation(&[first], &mut memory);
        assert_eq!(one[0].after, "one wo three");
        assert_eq!(memory.len(), 1);

        let second = OperationFile {
            file_id: "main.md".to_string(),
            before: one[0].after.clone(),
            anchors: vec![one[0].anchors[0].anchor.clone()],
            edits: vec![Edit {
                at: 4,
                delete: 1,
                insert: String::new(),
            }],
            fragments: Vec::new(),
        };
        let two = plan_operation(&[second], &mut memory);
        assert_eq!(two[0].after, "one o three");
        assert_eq!(memory.len(), 2);

        // The undo puts back "tw" as one insertion, so both pieces must rejoin.
        let undo = OperationFile {
            file_id: "main.md".to_string(),
            before: two[0].after.clone(),
            anchors: vec![two[0].anchors[0].anchor.clone()],
            edits: vec![Edit {
                at: 4,
                delete: 0,
                insert: "tw".to_string(),
            }],
            fragments: Vec::new(),
        };
        let planned = plan_operation(&[undo], &mut memory);
        let rejoined = &planned[0].anchors[0];
        assert_eq!(planned[0].after, "one two three");
        assert_eq!((rejoined.anchor.from, rejoined.anchor.to), (4, 7));
        assert_eq!(slice16(&planned[0].after, 4, 7), "two");
        assert!(!rejoined.anchor.detached);
        assert!(rejoined.touched);
        assert!(memory.is_empty());
    }

    #[test]
    fn two_cuts_of_the_same_word_and_one_paste_attach_one_comment() {
        let mut memory = MoveMemory::new(32);
        let late = OperationFile {
            file_id: "main.md".to_string(),
            before: "one two three two four".to_string(),
            anchors: vec![
                anchor_at("a", "main.md", 4, 7),
                anchor_at("b", "main.md", 14, 17),
            ],
            edits: vec![Edit {
                at: 14,
                delete: 3,
                insert: String::new(),
            }],
            fragments: Vec::new(),
        };
        let one = plan_operation(&[late], &mut memory);
        assert_eq!(one[0].after, "one two three  four");
        assert_eq!(memory.len(), 1);

        let early = OperationFile {
            file_id: "main.md".to_string(),
            before: one[0].after.clone(),
            anchors: one[0].anchors.iter().map(|pa| pa.anchor.clone()).collect(),
            edits: vec![Edit {
                at: 4,
                delete: 3,
                insert: String::new(),
            }],
            fragments: Vec::new(),
        };
        let two = plan_operation(&[early], &mut memory);
        assert_eq!(two[0].after, "one  three  four");
        assert_eq!(memory.len(), 2);

        // The paste lands at the end of the text, which is 16 units long.
        let paste = OperationFile {
            file_id: "main.md".to_string(),
            before: two[0].after.clone(),
            anchors: two[0].anchors.iter().map(|pa| pa.anchor.clone()).collect(),
            edits: vec![Edit {
                at: 16,
                delete: 0,
                insert: "two".to_string(),
            }],
            fragments: Vec::new(),
        };
        let planned = plan_operation(&[paste], &mut memory);
        assert_eq!(planned[0].after, "one  three  fourtwo");
        let attached = planned[0]
            .anchors
            .iter()
            .find(|pa| pa.anchor.id == "a")
            .unwrap();
        assert!(!attached.anchor.detached);
        assert_eq!((attached.anchor.from, attached.anchor.to), (16, 19));
        assert_eq!(slice16(&planned[0].after, 16, 19), "two");
        let detached = planned[0]
            .anchors
            .iter()
            .find(|pa| pa.anchor.id == "b")
            .unwrap();
        assert!(detached.anchor.detached);
        assert_eq!(memory.len(), 1);
    }

    fn setup(body: &str) -> (LoroDoc, String) {
        let doc = new_doc();
        let file = put_text(&doc, "main.md", body);
        doc.commit();
        (doc, file)
    }

    fn body(doc: &LoroDoc, file: &str) -> String {
        text_of(doc, file).unwrap().to_string()
    }

    fn record_file(doc: &LoroDoc, id: &str) -> Option<String> {
        let root = doc.get_map(ANCHORS);
        str_of(&record_map(&root, id)?, "file")
    }

    fn sync(a: &LoroDoc, b: &LoroDoc) {
        let to_a = b.export(loro::ExportMode::updates(&a.oplog_vv())).unwrap();
        let to_b = a.export(loro::ExportMode::updates(&b.oplog_vv())).unwrap();
        a.import(&to_a).unwrap();
        b.import(&to_b).unwrap();
    }

    /// The fragment record that holds the deleted text of `change`.
    fn fragment_of(doc: &LoroDoc, change: &str) -> String {
        let root = doc.get_map(CHANGES);
        let fragment: String = root
            .keys()
            .map(|k| k.to_string())
            .find(|k| {
                record_map(&root, k).is_some_and(|m| {
                    str_of(&m, "kind").as_deref() == Some("fragment")
                        && str_of(&m, "change").as_deref() == Some(change)
                })
            })
            .unwrap();
        fragment
    }

    /// The UTF-16 length of the common prefix of two texts.
    fn common_prefix16(a: &str, b: &str) -> usize {
        let mut units = 0;
        for (x, y) in a.chars().zip(b.chars()) {
            if x != y {
                break;
            }
            units += x.len_utf16();
        }
        units
    }

    /// Applies one edit to a file's text, then records it. The anchors are the
    /// ones resolved before the edit.
    fn edit_and_record(
        doc: &LoroDoc,
        file: &str,
        memory: &mut MoveMemory,
        at: usize,
        delete: usize,
        insert: &str,
    ) -> Vec<PlannedFile> {
        let before = body(doc, file);
        let anchors = resolve_anchors(doc, file);
        let edit = Edit {
            at,
            delete,
            insert: insert.to_string(),
        };
        let text = text_of(doc, file).unwrap();
        assert!(apply_text_edits(&text, std::slice::from_ref(&edit)));
        let plan = record_operation(
            doc,
            &[OperationFile {
                file_id: file.to_string(),
                before,
                anchors,
                edits: vec![edit],
                fragments: Vec::new(),
            }],
            memory,
            None,
        );
        doc.commit();
        plan
    }

    #[test]
    fn a_record_round_trips() {
        let (doc, file) = setup("alpha beta gamma");
        assert!(create_anchor(
            &doc, &file, "c1", 6, 10, "beta", "alpha ", " gamma"
        ));
        doc.commit();
        let anchors = resolve_anchors(&doc, &file);
        assert_eq!(anchors.len(), 1);
        assert_eq!((anchors[0].from, anchors[0].to), (6, 10));
        assert_eq!(anchors[0].exact, "beta");
        assert_eq!(status_of(&doc, &anchors[0]), Status::Exact);
    }

    #[test]
    fn cut_then_paste_repins_in_the_same_file() {
        let (doc, file) = setup("alpha beta gamma");
        assert!(create_anchor(
            &doc, &file, "c1", 6, 10, "beta", "alpha ", " gamma"
        ));
        doc.commit();
        let mut memory = MoveMemory::new(32);

        edit_and_record(&doc, &file, &mut memory, 6, 5, "");
        let cut = resolve_anchors(&doc, &file);
        assert_eq!(cut.len(), 1);
        assert!(cut[0].detached);
        assert_eq!(cut[0].length, Some(4));

        edit_and_record(&doc, &file, &mut memory, 6, 0, CUT);
        let pasted = resolve_anchors(&doc, &file);
        assert_eq!(pasted.len(), 1);
        assert!(!pasted[0].detached);
        assert_eq!((pasted[0].from, pasted[0].to), (6, 10));
        assert_eq!(status_of(&doc, &pasted[0]), Status::Exact);
        assert_eq!(record_file(&doc, "c1").as_deref(), Some(file.as_str()));
        assert!(memory.is_empty());
    }

    #[test]
    fn cut_in_one_file_paste_in_another_moves_the_record() {
        let doc = new_doc();
        let a = put_text(&doc, "a.md", "alpha beta gamma");
        let b = put_text(&doc, "b.md", "delta epsilon");
        doc.commit();
        assert!(create_anchor(
            &doc, &a, "c1", 6, 10, "beta", "alpha ", " gamma"
        ));
        doc.commit();
        let mut memory = MoveMemory::new(32);

        edit_and_record(&doc, &a, &mut memory, 6, 5, "");
        assert!(resolve_anchors(&doc, &b).is_empty());
        edit_and_record(&doc, &b, &mut memory, 6, 0, CUT);

        assert!(resolve_anchors(&doc, &a).is_empty());
        let moved = resolve_anchors(&doc, &b);
        assert_eq!(moved.len(), 1);
        assert!(!moved[0].detached);
        assert_eq!((moved[0].from, moved[0].to), (6, 10));
        assert_eq!(record_file(&doc, "c1").as_deref(), Some(b.as_str()));
        assert!(memory.is_empty());
    }

    #[test]
    fn two_peers_collapsing_a_passage_mark_it_detached() {
        let (a, file) = setup("alpha beta gamma");
        assert!(create_anchor(
            &a, &file, "c1", 6, 10, "beta", "alpha ", " gamma"
        ));
        a.commit();
        let b = new_doc();
        b.import(&a.export(loro::ExportMode::snapshot()).unwrap())
            .unwrap();

        // Each peer deletes one half of the passage: "be" on one, "ta" on the other.
        let text_a = text_of(&a, &file).unwrap();
        assert!(apply_text_edits(
            &text_a,
            &[Edit {
                at: 6,
                delete: 2,
                insert: String::new(),
            }]
        ));
        a.commit();
        let text_b = text_of(&b, &file).unwrap();
        assert!(apply_text_edits(
            &text_b,
            &[Edit {
                at: 8,
                delete: 2,
                insert: String::new(),
            }]
        ));
        b.commit();
        sync(&a, &b);

        let merged = resolve_anchors(&a, &file);
        assert_eq!(merged.len(), 1);
        assert!(merged[0].collapsed);

        assert_eq!(mark_collapsed(&a, &file), vec!["c1".to_string()]);
        a.commit();
        let marked = resolve_anchors(&a, &file);
        assert!(marked[0].detached);
        assert_eq!(status_of(&a, &marked[0]), Status::Deleted);
        let root = a.get_map(ANCHORS);
        let record = record_map(&root, "c1").unwrap();
        assert!(is_true(&record, "detached"));
    }

    #[test]
    fn a_rejected_tracked_deletion_restores_the_anchor() {
        let (doc, file) = setup("alpha beta gamma");
        assert!(create_anchor(
            &doc, &file, "c1", 6, 10, "beta", "alpha ", " gamma"
        ));
        doc.commit();
        let anchors = resolve_anchors(&doc, &file);
        let text_before = body(&doc, &file);
        let edit = Edit {
            at: 6,
            delete: 5,
            insert: String::new(),
        };
        let by = Author {
            who: "u1",
            author: "Ann",
        };
        let changes =
            track::apply_tracked_edits(&doc, &file, std::slice::from_ref(&edit), &by, None, 1_000)
                .unwrap();
        assert_eq!(changes.len(), 1);
        let change = changes[0].clone();
        doc.commit();

        let fragment = fragment_of(&doc, &change);
        let mut memory = MoveMemory::new(32);
        let range = FragmentRange {
            id: fragment.clone(),
            edit: 0,
            start: 0,
            end: utf16_len(CUT),
        };
        record_operation(
            &doc,
            &[OperationFile {
                file_id: file.clone(),
                before: text_before.clone(),
                anchors,
                edits: vec![edit],
                fragments: vec![range],
            }],
            &mut memory,
            None,
        );
        doc.commit();
        let pending = resolve_anchors(&doc, &file);
        assert!(pending[0].detached);
        assert_eq!(status_of(&doc, &pending[0]), Status::PendingDeletion);

        let text_deleted = body(&doc, &file);
        assert!(track::reject(&doc, &change));
        doc.commit();
        let text_restored = body(&doc, &file);
        assert_eq!(text_restored, text_before);
        let at = common_prefix16(&text_deleted, &text_restored);
        let restored = Restored {
            fragment: fragment.clone(),
            at,
            text: slice16(&text_restored, at, at + utf16_len(CUT)).to_string(),
        };
        assert_eq!(restored.text, CUT);
        assert_eq!(
            restore_by_fragments(&doc, &file, &[restored]),
            vec!["c1".to_string()]
        );
        doc.commit();

        let back = resolve_anchors(&doc, &file);
        assert!(!back[0].detached);
        assert_eq!((back[0].from, back[0].to), (6, 10));
        assert_eq!(status_of(&doc, &back[0]), Status::Exact);
    }

    #[test]
    fn a_replacement_inside_a_passage_reads_modified() {
        let (doc, file) = setup("alpha beta gamma");
        assert!(create_anchor(
            &doc, &file, "c1", 6, 10, "beta", "alpha ", " gamma"
        ));
        doc.commit();
        let mut memory = MoveMemory::new(32);

        // "et" is replaced by "XYZ": the passage keeps its "b" and "a" and gains the insert.
        edit_and_record(&doc, &file, &mut memory, 7, 2, "XYZ");
        assert_eq!(body(&doc, &file), "alpha bXYZa gamma");
        let anchors = resolve_anchors(&doc, &file);
        assert_eq!(anchors.len(), 1);
        assert_eq!((anchors[0].from, anchors[0].to), (6, 11));
        assert_eq!(slice16(&body(&doc, &file), 6, 11), "bXYZa");
        assert_eq!(status_of(&doc, &anchors[0]), Status::Modified);
    }

    #[test]
    fn a_healed_anchor_survives_a_shallow_snapshot() {
        let (doc, file) = setup("alpha beta gamma");
        assert!(create_anchor(
            &doc, &file, "c1", 6, 10, "beta", "alpha ", " gamma"
        ));
        doc.commit();
        // "ta" goes, and the end cursor's character with it.
        let text = text_of(&doc, &file).unwrap();
        assert!(apply_text_edits(
            &text,
            &[Edit {
                at: 8,
                delete: 2,
                insert: String::new(),
            }]
        ));
        doc.commit();

        assert_eq!(heal_stale(&doc, &file), vec!["c1".to_string()]);
        doc.commit();
        assert!(
            heal_stale(&doc, &file).is_empty(),
            "a healed record is not stale again"
        );

        let snapshot = doc
            .export(loro::ExportMode::ShallowSnapshot(std::borrow::Cow::Owned(
                doc.oplog_frontiers(),
            )))
            .unwrap();
        let shallow = LoroDoc::new();
        shallow.import(&snapshot).unwrap();
        assert!(shallow.is_shallow());
        let found = resolve_anchors(&shallow, &file);
        assert_eq!((found[0].from, found[0].to), (6, 8));
        assert_eq!(status_of(&shallow, &found[0]), Status::Modified);
    }
}
