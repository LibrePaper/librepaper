//! The word diff, and the three-way merge that `komodoc sync` needs when a
//! text editor saves a buffer that was read before the session moved on.
//!
//! A token here is either a maximal run of whitespace or a maximal run of
//! non-whitespace, and the two alternate. A whitespace run is a token of its
//! own so that a doubled space or an added trailing newline is one small edit
//! rather than a rewrite of the words either side of it; punctuation carries
//! along with the word it is attached to, because an author who changes
//! `interval.` to `interval,` changed one thing. The unit is the word and
//! never the line: the editable examples are one line per paragraph, so a
//! merge by line would call any two edits to the same paragraph a conflict,
//! which is most of the conflicts this command would ever see.
//!
//! Every position out of this module -- in `Edit` and in `Conflict` alike --
//! is a UTF-16 code unit offset, because that is what the document counts in
//! (`session::replace_text` does the same arithmetic) and what a JavaScript
//! string counts in. A caller holding a `yrs::Text` or a browser string can
//! use these numbers without converting anything.
//!
//! Nothing here knows about Yrs, Tokio, the store or the room. It is a pure
//! function over three strings, in a crate of its own so that the binary and
//! the engine -- which exports it to the browser for the timeline's per-file
//! diff -- can both reach it.

#[cfg(test)]
mod tests;

/// One replacement in the old text: delete `delete` UTF-16 units at `at`, then
/// insert `insert`. Edits from `diff` are sorted by `at` and never overlap, so
/// a caller applies them back to front, or front to back with a running
/// offset, and lands exactly on the new text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    pub at: usize,
    pub delete: usize,
    pub insert: String,
}

/// A region both sides changed. `at` and `len` locate it in the merged text,
/// which per `docs/specs/sync.md` carries `remote`'s version there because the
/// session is what every other peer and every reader is looking at. `local` is
/// what the file said, kept so the caller can print the line that names what
/// it gave up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    pub at: usize,
    pub len: usize,
    pub local: String,
    pub remote: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Merged {
    pub text: String,
    pub conflicts: Vec<Conflict>,
}

/// One token of the text, with where it sits in UTF-16 units.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Token<'a> {
    pub text: &'a str,
    pub at: usize,
    pub len: usize,
}

/// Splits into alternating runs of whitespace and non-whitespace. The tokens
/// partition the string: concatenating them gives it back, and their `at`
/// offsets run consecutively, which is what lets an edit over a token range be
/// stated as one contiguous delete.
pub fn tokenize(text: &str) -> Vec<Token<'_>> {
    let mut out = Vec::new();
    let mut at = 0;
    let mut start = 0;
    let mut len = 0;
    let mut space: Option<bool> = None;
    for (byte, ch) in text.char_indices() {
        let ws = ch.is_whitespace();
        if space.is_some_and(|was| was != ws) {
            out.push(Token {
                text: &text[start..byte],
                at,
                len,
            });
            at += len;
            start = byte;
            len = 0;
        }
        space = Some(ws);
        len += ch.len_utf16();
    }
    if space.is_some() {
        out.push(Token {
            text: &text[start..],
            at,
            len,
        });
    }
    out
}

/// The word-level diff, as the smallest set of replacements that turns `old`
/// into `new`. Shared with the reader's "what changed since" list
/// (`docs/specs/history.md`), which wants the same hunks over the same tokens.
pub fn diff(old: &str, new: &str) -> Vec<Edit> {
    let a = tokenize(old);
    let b = tokenize(new);
    let end = a.last().map_or(0, |t| t.at + t.len);
    hunks(&a, &b)
        .into_iter()
        .map(|h| Edit {
            at: a.get(h.a0).map_or(end, |t| t.at),
            delete: if h.a1 > h.a0 {
                a[h.a1 - 1].at + a[h.a1 - 1].len - a[h.a0].at
            } else {
                0
            },
            insert: join(&b[h.b0..h.b1]),
        })
        .collect()
}

/// Three-way merge, `diff3` in miniature. Edits to disjoint token ranges all
/// go through; where both sides moved the same tokens the session wins and the
/// region is reported.
pub fn merge(base: &str, local: &str, remote: &str) -> Merged {
    // The common case, and it is exact: the session did not move, so the file
    // is simply right.
    if remote == base {
        return Merged {
            text: local.to_string(),
            conflicts: Vec::new(),
        };
    }
    let b = tokenize(base);
    let mut mine = side(&b, local);
    let mut theirs = side(&b, remote);

    // An edit both sides made identically is agreement, not a conflict. Take
    // it once and keep it out of the clustering below.
    let mut agreed = Vec::new();
    mine.retain(|h| {
        if let Some(i) = theirs.iter().position(|o| o == h) {
            theirs.remove(i);
            agreed.push(h.clone());
            false
        } else {
            true
        }
    });

    let mut regions = cluster(&b, &mine, &theirs);
    regions.extend(agreed.into_iter().map(|h| Region {
        start: h.start,
        end: h.end,
        text: h.text,
        clash: None,
    }));
    regions.sort_by_key(|r| (r.start, r.end));

    let mut text = String::new();
    let mut width = 0;
    let mut conflicts = Vec::new();
    let mut pos = 0;
    for region in regions {
        let kept = join(&b[pos..region.start]);
        width += utf16_len(&kept);
        text.push_str(&kept);
        let len = utf16_len(&region.text);
        if let Some(local_side) = region.clash {
            conflicts.push(Conflict {
                at: width,
                len,
                local: local_side,
                remote: region.text.clone(),
            });
        }
        width += len;
        text.push_str(&region.text);
        pos = region.end;
    }
    text.push_str(&join(&b[pos..]));
    Merged { text, conflicts }
}

// ---------------------------------------------------------------------------
// The diff, over tokens

/// A differing stretch: base tokens `a0..a1` became new tokens `b0..b1`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Hunk {
    a0: usize,
    a1: usize,
    b0: usize,
    b1: usize,
}

/// One side's edits, as replacements of base token ranges. `start == end` is
/// an insertion at a seam.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SideHunk {
    start: usize,
    end: usize,
    text: String,
}

/// A stretch of base tokens the merged text replaces. `clash` holds the file's
/// version when both sides changed it, in which case `text` is the session's.
#[derive(Debug, Clone)]
struct Region {
    start: usize,
    end: usize,
    text: String,
    clash: Option<String>,
}

fn utf16_len(text: &str) -> usize {
    text.chars().map(char::len_utf16).sum()
}

fn join(tokens: &[Token<'_>]) -> String {
    tokens.iter().map(|t| t.text).collect()
}

fn side(base: &[Token<'_>], other: &str) -> Vec<SideHunk> {
    let tokens = tokenize(other);
    hunks(base, &tokens)
        .into_iter()
        .map(|h| SideHunk {
            start: h.a0,
            end: h.a1,
            text: join(&tokens[h.b0..h.b1]),
        })
        .collect()
}

/// Myers is bounded by the size of the difference rather than the size of the
/// text, which is the shape this wants: two 200 KB drafts a few words apart
/// cost a few words of work once the shared head and tail are off.
const TRACE_CEILING: usize = 2000;

fn hunks(a: &[Token<'_>], b: &[Token<'_>]) -> Vec<Hunk> {
    let mut head = 0;
    while head < a.len() && head < b.len() && a[head].text == b[head].text {
        head += 1;
    }
    let mut tail = 0;
    while tail < a.len() - head
        && tail < b.len() - head
        && a[a.len() - 1 - tail].text == b[b.len() - 1 - tail].text
    {
        tail += 1;
    }
    let (an, bn) = (a.len() - tail, b.len() - tail);
    if head == an && head == bn {
        return Vec::new();
    }
    let left: Vec<&str> = a[head..an].iter().map(|t| t.text).collect();
    let right: Vec<&str> = b[head..bn].iter().map(|t| t.text).collect();

    let Some(pairs) = myers(&left, &right) else {
        // Two texts with nothing in common past the ends. One replacement is
        // both correct and, at that distance, the honest description.
        return vec![Hunk {
            a0: head,
            a1: an,
            b0: head,
            b1: bn,
        }];
    };

    let mut out = Vec::new();
    let (mut ai, mut bi) = (head, head);
    for (i, j) in pairs {
        let (i, j) = (i + head, j + head);
        if i > ai || j > bi {
            out.push(Hunk {
                a0: ai,
                a1: i,
                b0: bi,
                b1: j,
            });
        }
        ai = i + 1;
        bi = j + 1;
    }
    if ai < an || bi < bn {
        out.push(Hunk {
            a0: ai,
            a1: an,
            b0: bi,
            b1: bn,
        });
    }
    out
}

/// Greedy Myers, returning the matched index pairs -- the longest common
/// subsequence of tokens. `None` when the two are further apart than
/// `TRACE_CEILING` edits, where the trace would cost more memory than the
/// answer is worth.
///
/// Written out rather than pulled in: it is sixty lines, it has to run over
/// our own `Token` slices, and a crate would be the larger of the two.
fn myers(a: &[&str], b: &[&str]) -> Option<Vec<(usize, usize)>> {
    let (n, m) = (a.len() as isize, b.len() as isize);
    let ceiling = ((n + m) as usize).min(TRACE_CEILING) as isize;
    // Furthest-reaching x on each diagonal k, indexed k + offset.
    let offset = (n + m) as usize + 1;
    let mut v = vec![0isize; 2 * offset + 1];
    // One snapshot per round, kept only over the diagonals that round can
    // reach, so the trace costs O(d^2) and not O(d * (n + m)).
    let mut trace: Vec<Vec<isize>> = Vec::new();

    let mut found = None;
    for d in 0..=ceiling {
        let lo = offset - (d as usize) - 1;
        let hi = offset + (d as usize) + 1;
        trace.push(v[lo..=hi].to_vec());
        let mut k = -d;
        while k <= d {
            let idx = (k + offset as isize) as usize;
            let mut x = if k == -d || (k != d && v[idx - 1] < v[idx + 1]) {
                v[idx + 1]
            } else {
                v[idx - 1] + 1
            };
            let mut y = x - k;
            while x < n && y < m && a[x as usize] == b[y as usize] {
                x += 1;
                y += 1;
            }
            v[idx] = x;
            if x >= n && y >= m {
                found = Some(d);
                break;
            }
            k += 2;
        }
        if found.is_some() {
            break;
        }
    }
    let d_end = found?;

    let mut pairs = Vec::new();
    let (mut x, mut y) = (n, m);
    for d in (0..=d_end).rev() {
        let snapshot = &trace[d as usize];
        // The snapshot spans k in -(d+1)..=(d+1); centre it the same way.
        let centre = d + 1;
        let k = x - y;
        let prev_k = if k == -d
            || (k != d && snapshot[(k - 1 + centre) as usize] < snapshot[(k + 1 + centre) as usize])
        {
            k + 1
        } else {
            k - 1
        };
        let prev_x = snapshot[(prev_k + centre) as usize];
        let prev_y = prev_x - prev_k;
        while x > prev_x && y > prev_y {
            x -= 1;
            y -= 1;
            pairs.push((x as usize, y as usize));
        }
        x = prev_x;
        y = prev_y;
    }
    pairs.reverse();
    Some(pairs)
}

// ---------------------------------------------------------------------------
// The merge, over the two sides' hunks

/// Two edits collide when they move overlapping base tokens, or when both are
/// insertions at the same seam. Touching without sharing a token -- an
/// insertion at the seam of the other side's deletion -- is not a collision:
/// both go through, in that order.
fn collides(a: &SideHunk, b: &SideHunk) -> bool {
    if a.start == a.end && b.start == b.end {
        return a.start == b.start;
    }
    a.start < b.end && b.start < a.end
}

/// Groups colliding edits into regions, resolving each mixed group to the
/// session's text and reporting it.
fn cluster(base: &[Token<'_>], mine: &[SideHunk], theirs: &[SideHunk]) -> Vec<Region> {
    // (is_local, hunk), grouped by the collision relation and then by plain
    // interval overlap, so a region can never contain a stray edit that was
    // only nested inside it.
    let all: Vec<(bool, &SideHunk)> = mine
        .iter()
        .map(|h| (true, h))
        .chain(theirs.iter().map(|h| (false, h)))
        .collect();
    let mut group: Vec<usize> = (0..all.len()).collect();
    for i in 0..all.len() {
        for j in 0..i {
            if collides(all[i].1, all[j].1) {
                let (gi, gj) = (group[i], group[j]);
                let keep = gi.min(gj);
                let drop = gi.max(gj);
                group
                    .iter_mut()
                    .filter(|g| **g == drop)
                    .for_each(|g| *g = keep);
            }
        }
    }
    let mut spans: Vec<(usize, usize, Vec<usize>)> = Vec::new();
    for g in 0..all.len() {
        let members: Vec<usize> = (0..all.len()).filter(|i| group[*i] == g).collect();
        if members.is_empty() {
            continue;
        }
        let start = members.iter().map(|i| all[*i].1.start).min().unwrap();
        let end = members.iter().map(|i| all[*i].1.end).max().unwrap();
        spans.push((start, end, members));
    }
    spans.sort_by_key(|(start, end, _)| (*start, *end));
    let mut merged: Vec<(usize, usize, Vec<usize>)> = Vec::new();
    for (start, end, members) in spans {
        match merged.last_mut() {
            Some(last) if start < last.1 && last.0 < end => {
                last.1 = last.1.max(end);
                last.2.extend(members);
            }
            _ => merged.push((start, end, members)),
        }
    }

    merged
        .into_iter()
        .map(|(start, end, members)| {
            let local = rewrite(base, start, end, &members, &all, true);
            let remote = rewrite(base, start, end, &members, &all, false);
            let both = members.iter().any(|i| all[*i].0) && members.iter().any(|i| !all[*i].0);
            Region {
                start,
                end,
                clash: both.then(|| local.clone()),
                text: if both || !members.iter().any(|i| all[*i].0) {
                    remote
                } else {
                    local
                },
            }
        })
        .collect()
}

/// One side's reading of `start..end`: base with that side's edits in it.
fn rewrite(
    base: &[Token<'_>],
    start: usize,
    end: usize,
    members: &[usize],
    all: &[(bool, &SideHunk)],
    want_local: bool,
) -> String {
    let mut picked: Vec<&SideHunk> = members
        .iter()
        .filter(|i| all[**i].0 == want_local)
        .map(|i| all[*i].1)
        .collect();
    picked.sort_by_key(|h| (h.start, h.end));
    let mut out = String::new();
    let mut pos = start;
    for h in picked {
        out.push_str(&join(&base[pos..h.start]));
        out.push_str(&h.text);
        pos = h.end;
    }
    out.push_str(&join(&base[pos..end]));
    out
}
