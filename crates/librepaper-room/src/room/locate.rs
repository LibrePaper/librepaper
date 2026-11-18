//! Turning what a reader selected into a range in the source.
//!
//! A comment is about a passage of a document's source at a checkpoint, and
//! the page a reader selects on is only the surface. Something has to cross
//! that gap. The renderers cannot: they are pinned WebAssembly modules
//! released by their own repositories, whose ABI returns a page and a list of
//! diagnostics and says nothing about where either came from. So the crossing
//! is done here, once, at the moment a comment is made -- never again
//! afterwards, which is the whole difference between this and re-anchoring.
//! What it produces is verified against the checkpoint and then frozen; where
//! that passage has since moved to is a separate question, answered in
//! [`super::derive`] from the comment's anchor record.
//!
//! The bridge is the prose. A heading is `# Title` in markdown and `Title` in
//! the page, and a formula is neither, but the words between the markup are
//! the same words. So both sides are flattened -- syntax and list markers
//! blanked, runs of whitespace collapsed -- and the passage is looked for in the flattened
//! source. Blanking preserves length and the collapse carries a map, so a
//! place found in the flattened copy is a place in the file itself.
//!
//! It refuses rather than guesses. A passage that reads the same in several
//! places, with nothing around it to tell them apart, is not located at the
//! first of them: that would make the comment silently about the wrong
//! sentence, which is worse than asking for a longer selection.

use librepaper_document::document::text_index::{Span, TextIndex, Utf16};
use librepaper_engine::storage::annotation::{AnchorSide, FileId, SourceTextTarget};

/// Below this a phrase is too common to identify a place on its own:
/// "the interval" appears throughout a document about intervals. A selection
/// this short is still looked for -- it may occur once, or its context may
/// say which one -- but the search stops rather than shortening past it,
/// because a shorter phrase can only be less certain than the one that has
/// already failed.
const ENOUGH: usize = 16;

/// How many letters of the surrounding text have to agree, and by how many it
/// has to out-agree the runner-up, before that counts as telling two identical
/// passages apart. A word's worth, counted in letters with spacing and
/// punctuation set aside. Two candidates that both merely follow a space are
/// not distinguished by it.
const DECISIVE: usize = 4;

/// How many letters of the page's text on each side of the selection are
/// compared with the source. About eight words either side is enough to tell
/// two passages apart; more would let differences far from the selection count
/// against a match.
const WINDOW: usize = 48;

/// How much of the source is kept beside a range as recovery evidence. The
/// same as the configured context cap, which is what a client may send.
const CONTEXT: usize = 64;

/// One file a passage might have come from.
pub struct Candidate<'a> {
    pub file_id: &'a str,
    pub path: &'a str,
    pub text: &'a str,
}

/// What the browser saw: the selected words and the text around them, as the
/// rendered page had it. None of it is identity -- it is the question, and
/// the range this module returns is the answer.
pub struct Quote<'a> {
    pub exact: &'a str,
    pub prefix: &'a str,
    pub suffix: &'a str,
}

/// Why a selection could not be made into a source range. Each is a different
/// thing to tell the person who selected it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Failure {
    /// Nothing was selected, or all of it was markup.
    Empty,
    /// The words are not in any file of this document.
    NotFound,
    /// The words are in more than one place and the surrounding text does not
    /// say which.
    Ambiguous,
}

impl Failure {
    pub fn message(self) -> &'static str {
        match self {
            Failure::Empty => "that selection has no words to comment on",
            Failure::NotFound => "that passage is not in this document's source",
            Failure::Ambiguous => "those words appear in several places; select a longer passage",
        }
    }
}

/// Markup, as far as this is concerned: characters that are syntax in
/// markdown, typst or LaTeX and never part of a word. Blanked rather than
/// removed, so an offset into the flattened copy is still an offset into the
/// original.
fn is_markup(character: char) -> bool {
    matches!(
        character,
        '#' | '*'
            | '_'
            | '`'
            | '~'
            | '='
            | '$'
            | '@'
            | '<'
            | '>'
            | '['
            | ']'
            | '('
            | ')'
            | '|'
            | '\\'
            | '{'
            | '}'
    )
}

fn is_space(character: char) -> bool {
    matches!(character, ' ' | '\n' | '\t' | '\r' | '\u{0b}' | '\u{0c}')
}

/// A file's prose, and where each character of it came from.
///
/// `chars` is the flattened copy; `from[i]` is the UTF-16 offset in the source
/// that `chars[i]` was taken from, with one extra entry past the end so a
/// match that runs to the end of the file still has somewhere to point.
pub struct Flat {
    pub chars: Vec<char>,
    pub from: Vec<u32>,
}

/// A named entity from the full HTML table, accepted only when it decodes to
/// exactly one character. Anything else is blanked: an entity nobody here
/// knows is not prose, and leaving its letters in would put `amp` in the
/// middle of a sentence. `entity` is the whole reference, `&` and `;` included.
fn named_entity(entity: &str) -> Option<char> {
    let decoded = html_escape::decode_html_entities(entity);
    let mut chars = decoded.chars();
    let only = chars.next()?;
    if chars.next().is_some() || decoded == entity {
        return None;
    }
    // A no-break space is a space here, as it is to a reader.
    Some(if only == '\u{a0}' { ' ' } else { only })
}

/// How far to look for the `;` of an entity: `&`, the longest HTML5 entity
/// name (`CounterClockwiseContourIntegral`, 31 characters) and `;`.
const ENTITY_SCAN_BYTES: usize = 1 + 31 + 1;

fn numeric_entity(body: &str) -> Option<char> {
    let digits = body.strip_prefix('#')?;
    let code = if let Some(hex) = digits.strip_prefix(['x', 'X']) {
        u32::from_str_radix(hex, 16).ok()?
    } else {
        digits.parse::<u32>().ok()?
    };
    // Browsers replace null, surrogate and out-of-range references with the
    // replacement character. Be exactly as forgiving: an invalid entity in
    // somebody's HTML must not stop a comment from being placed.
    if code == 0 || code > 0x10ffff || (0xd800..=0xdfff).contains(&code) {
        return Some('\u{fffd}');
    }
    char::from_u32(code).or(Some('\u{fffd}'))
}

/// Whether this file's markup is tags rather than characters.
pub fn is_html(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.ends_with(".html") || lower.ends_with(".htm")
}

/// How many characters of a list marker `text` begins with, or zero if it
/// begins with none. A marker is a `-` or `+`, or digits closed by `.` or `)`,
/// and either must be followed by a space or a tab: `-1` and `well-known` have
/// no marker. Whether the text starts a line is for the caller to say.
fn list_marker_len(text: &str) -> usize {
    let bytes = text.as_bytes();
    let gap_at = |at: usize| matches!(bytes.get(at).copied(), Some(b' ' | b'\t'));
    let marker = match bytes.first().copied() {
        Some(b'-' | b'+') => 1,
        _ => {
            let digits = bytes
                .iter()
                .take_while(|byte| byte.is_ascii_digit())
                .count();
            if digits == 0 || !matches!(bytes.get(digits).copied(), Some(b'.' | b')')) {
                return 0;
            }
            digits + 1
        }
    };
    if gap_at(marker) {
        marker
    } else {
        0
    }
}

/// Flattens a source file to the prose a reader would see, keeping for every
/// character the offset it came from. Outside HTML, a list marker at the start
/// of a line is syntax like any other and is blanked the same way.
///
/// Blanking keeps the length, so blanked syntax costs nothing; the whitespace
/// collapse does not, which is why the map exists. Without it a match found
/// here would be a position in a text nobody has.
pub fn flatten(text: &str, html: bool) -> Flat {
    let mut chars = Vec::new();
    let mut from = Vec::new();
    let mut at16: u32 = 0;
    let mut was_space = false;
    // Whether the next character begins a line, once the spaces and tabs before
    // it are passed over; and how many characters of a list marker are still to
    // be blanked.
    let mut line_start = true;
    let mut marker_left = 0;
    let mut rest = text;
    while let Some(character) = rest.chars().next() {
        let width = character.len_utf16() as u32;
        let bytes = character.len_utf8();
        let mut emit = character;
        let mut skip = bytes;
        let mut skipped16 = width;
        if !html && line_start && marker_left == 0 {
            marker_left = list_marker_len(rest);
        }
        if marker_left > 0 {
            // Markup, blanked the way a `*` is.
            marker_left -= 1;
            emit = ' ';
        } else if html && character == '<' {
            // A tag is markup the way a `*` is, only longer. The whole of it
            // is whitespace as far as the prose is concerned.
            let end = rest.find('>').map(|at| at + 1).unwrap_or(rest.len());
            skip = end;
            skipped16 = rest[..end].encode_utf16().count() as u32;
            emit = ' ';
        } else if html && character == '&' {
            // An entity is a word spelled sideways. Decode it at its own
            // first character and let the rest of it collapse as whitespace,
            // so the offsets on either side still hold.
            // An entity is short, so look no further than the longest one,
            // and not into the middle of a character, which is a slice the
            // language refuses.
            let mut limit = rest.len().min(ENTITY_SCAN_BYTES);
            while !rest.is_char_boundary(limit) {
                limit -= 1;
            }
            if let Some(semicolon) = rest[..limit].find(';') {
                let body = &rest[1..semicolon];
                let decoded = numeric_entity(body).or_else(|| named_entity(&rest[..semicolon + 1]));
                if let Some(decoded) = decoded {
                    // `&nbsp;` decodes to a space, and a space next to a space
                    // is one space here like anywhere else -- otherwise a
                    // needle with one space in it would not match a haystack
                    // with two.
                    if is_space(decoded) {
                        if !was_space {
                            was_space = true;
                            chars.push(' ');
                            from.push(at16);
                        }
                    } else {
                        was_space = false;
                        chars.push(decoded);
                        from.push(at16);
                    }
                    let consumed = &rest[..semicolon + 1];
                    at16 += consumed.encode_utf16().count() as u32;
                    rest = &rest[semicolon + 1..];
                    continue;
                }
            }
            emit = ' ';
        } else if is_markup(character) {
            emit = ' ';
        }
        if is_space(emit) {
            // One space for the run, remembered at the first character of it.
            if !was_space {
                was_space = true;
                chars.push(' ');
                from.push(at16);
            }
        } else {
            was_space = false;
            chars.push(emit);
            from.push(at16);
        }
        line_start = match character {
            '\n' => true,
            ' ' | '\t' => line_start,
            _ => false,
        };
        at16 += skipped16;
        rest = &rest[skip..];
    }
    // One past the end, so a match that reaches the end of the file has an
    // end offset to map to.
    from.push(at16);
    Flat { chars, from }
}

/// Every place `needle` occurs in `haystack`, as indices into the flattened
/// characters.
fn occurrences(haystack: &[char], needle: &[char]) -> Vec<usize> {
    let mut found = Vec::new();
    if needle.is_empty() || needle.len() > haystack.len() {
        return found;
    }
    let first = needle[0];
    let last = haystack.len() - needle.len();
    for at in 0..=last {
        if haystack[at] == first && &haystack[at..at + needle.len()] == needle {
            found.push(at);
        }
    }
    found
}

/// The letters and digits of some characters, lowercased. Everything else --
/// spaces, punctuation, list markers, markup -- is how the renderer laid the
/// words out rather than what they are, so context is compared without it.
fn letters(chars: &[char]) -> Vec<char> {
    chars
        .iter()
        .filter(|character| character.is_alphanumeric())
        .flat_map(|character| character.to_lowercase())
        .collect()
}

/// The last `count` letters before `end`, in reading order. It walks back from
/// `end` and stops once it has enough, so a long file is not normalized whole
/// for every candidate that is scored.
fn letters_before(chars: &[char], end: usize, count: usize) -> Vec<char> {
    let mut taken = Vec::new();
    for character in chars[..end].iter().rev() {
        if taken.len() >= count {
            break;
        }
        if character.is_alphanumeric() {
            // Collected nearest first, so the reversal below puts them in order.
            taken.extend(character.to_lowercase().rev());
        }
    }
    taken.truncate(count);
    taken.reverse();
    taken
}

/// The first `count` letters from `start` on, in reading order.
fn letters_after(chars: &[char], start: usize, count: usize) -> Vec<char> {
    let mut taken = Vec::new();
    for character in chars.iter().skip(start) {
        if taken.len() >= count {
            break;
        }
        if character.is_alphanumeric() {
            taken.extend(character.to_lowercase());
        }
    }
    taken.truncate(count);
    taken
}

/// How many characters of `page` agree with `source`, where the two are
/// aligned at their ends: the last character of `page` lines up with the last
/// of `source`. What comes before that in `source` may be skipped for nothing,
/// since the source may simply have more before the context than the page
/// kept; every character of `page` still has to be accounted for. Each
/// substitution, insertion or deletion costs one, so a stray character costs
/// one rather than everything after it.
///
/// This is a semi-global edit distance. The first row is all zeros, which is
/// the free skip, and the distance is read from the last row.
fn anchored_agreement(page: &[char], source: &[char]) -> usize {
    let m = page.len();
    let n = source.len();
    let mut previous = vec![0usize; n + 1];
    let mut current = vec![0usize; n + 1];
    for i in 1..=m {
        current[0] = i;
        for j in 1..=n {
            let substitution = previous[j - 1] + usize::from(page[i - 1] != source[j - 1]);
            let deletion = previous[j] + 1;
            let insertion = current[j - 1] + 1;
            current[j] = substitution.min(deletion).min(insertion);
        }
        std::mem::swap(&mut previous, &mut current);
    }
    m.saturating_sub(previous[n])
}

#[derive(Clone)]
struct Hit {
    file: usize,
    at: usize,
    length: usize,
    score: usize,
}

/// How well the text around a candidate agrees with the text that surrounded
/// the selection on the page: the letters before the candidate against the
/// page's prefix, and the letters after it against the page's suffix. Each side
/// is compared as letters only, so a stray character or a list marker the
/// renderer dropped costs one, not the whole comparison. The source window is
/// twice as long as the page's, so the source may hold more than the page did
/// beyond the shared context. This is what tells two identical sentences
/// apart, and it is the only thing allowed to.
fn score_of(flat: &Flat, at: usize, length: usize, prefix: &[char], suffix: &[char]) -> usize {
    let before = letters_before(&flat.chars, at, 2 * prefix.len());
    let after = letters_after(&flat.chars, at + length, 2 * suffix.len());
    // Reversed, so the suffix is anchored at the selection's end like the prefix.
    let suffix_backward: Vec<char> = suffix.iter().rev().copied().collect();
    let after_backward: Vec<char> = after.iter().rev().copied().collect();
    anchored_agreement(prefix, &before) + anchored_agreement(&suffix_backward, &after_backward)
}

/// Finds the source range a rendered selection came from.
///
/// The selection is tried whole first and then a word shorter at a time, which
/// is what recovers a selection that ran into something the renderer invented
/// -- a figure number, a footnote marker, a formatted citation -- and stopped
/// agreeing with the source partway through. A candidate too short to be sure
/// of is only ever accepted when it occurs exactly once.
///
/// Shortening happens from both ends, separately, because the renderer's
/// inventions are not only at the end of a selection. `[@lovelace1843]` is
/// `(Lovelace, 1843)` on the page, and a reader who selects from there to the
/// end of the sentence has a selection whose *first* words are the ones no
/// file contains: trimming the tail can only shorten it into more of what was
/// never there. Trimming the head recovers the rest of the sentence, which is
/// the part they were actually pointing at.
///
/// The longer of the two survivals wins. Both are the author's own words and
/// both are anchored the same way, so the tie-break is simply which recovers
/// more of what was selected; an exact tie keeps the head, which is where a
/// selection usually starts on purpose.
pub fn locate(files: &[Candidate<'_>], quote: &Quote<'_>) -> Result<SourceTextTarget, Failure> {
    let wanted = trimmed(&flatten(quote.exact, false).chars);
    if wanted.is_empty() {
        return Err(Failure::Empty);
    }
    let search = Search::over(files, quote);
    let keeping_the_head = first_hit(&search, shortening_from_the_end(&wanted));
    // The whole phrase is what the pass above began with, so this one starts a
    // word in.
    let keeping_the_tail = first_hit(&search, shortening_from_the_start(&wanted).skip(1));
    let hit = match (keeping_the_head, keeping_the_tail) {
        (Some(head), Some(tail)) => Some(if tail.length > head.length {
            tail
        } else {
            head
        }),
        (Some(only), None) | (None, Some(only)) => Some(only),
        (None, None) => None,
    };
    let Some(hit) = hit else {
        return Err(search.failure());
    };
    let (html, flat) = &search.flattened[hit.file];
    Ok(place(&files[hit.file], flat, *html, hit.at, hit.length))
}

/// The first of these needles that lands in exactly one place.
///
/// They arrive longest first, so the first that lands is the most of the
/// selection this direction of trimming can account for. The floor is a floor
/// on trying again, not on accepting: a phrase below it is still tried once,
/// because it may well occur only once, but nothing shorter is -- a shorter
/// phrase can only be less certain than one that has already failed.
fn first_hit<'a>(search: &Search, needles: impl Iterator<Item = &'a [char]>) -> Option<Hit> {
    for needle in needles {
        if let Some(hit) = search.find(needle) {
            return Some(hit);
        }
        if needle.len() < ENOUGH {
            break;
        }
    }
    None
}

fn trimmed(chars: &[char]) -> Vec<char> {
    let mut out = chars.to_vec();
    while out.first().is_some_and(|c| is_space(*c)) {
        out.remove(0);
    }
    while out.last().is_some_and(|c| is_space(*c)) {
        out.pop();
    }
    out
}

/// Where each word of a flattened phrase begins.
fn word_starts(phrase: &[char]) -> Vec<usize> {
    std::iter::once(0)
        .chain(
            phrase
                .iter()
                .enumerate()
                .filter(|(_, c)| is_space(**c))
                .map(|(at, _)| at + 1),
        )
        .collect()
}

/// The phrase, then the phrase without its first word, and so on: the other
/// direction, for a selection that began in something the renderer invented.
fn shortening_from_the_start(phrase: &[char]) -> impl Iterator<Item = &[char]> {
    let length = phrase.len();
    word_starts(phrase)
        .into_iter()
        .filter(move |at| *at < length)
        .map(move |at| &phrase[at..])
}

/// The phrase, then the phrase without its last word, and so on. Shortening by
/// words rather than by characters: half a word is not a shorter phrase, it is
/// a different one.
fn shortening_from_the_end(phrase: &[char]) -> impl Iterator<Item = &[char]> {
    let starts = word_starts(phrase);
    let length = phrase.len();
    (1..=starts.len()).rev().filter_map(move |take| {
        let end = if take == starts.len() {
            length
        } else {
            // Up to, but not including, the space before the next word.
            starts[take] - 1
        };
        (end > 0).then(|| &phrase[..end])
    })
}

/// One question, asked of every file of a document.
///
/// It borrowed the candidate files as well as flattening them, and never
/// read the borrowed copy: the flattened text is what a search is over.
struct Search {
    flattened: Vec<(bool, Flat)>,
    prefix: Vec<char>,
    suffix: Vec<char>,
    ambiguous: std::cell::Cell<bool>,
}

impl Search {
    fn over(files: &[Candidate<'_>], quote: &Quote<'_>) -> Search {
        // The page's context as letters: the end of what came before the
        // selection and the start of what came after it, each one window long.
        let before = letters(&flatten(quote.prefix, false).chars);
        let after = letters(&flatten(quote.suffix, false).chars);
        Search {
            flattened: files
                .iter()
                .map(|file| (is_html(file.path), flatten(file.text, is_html(file.path))))
                .collect(),
            prefix: before[before.len().saturating_sub(WINDOW)..].to_vec(),
            suffix: after[..after.len().min(WINDOW)].to_vec(),
            ambiguous: std::cell::Cell::new(false),
        }
    }

    /// The one place this phrase is, or nothing.
    ///
    /// Nothing covers two different situations, and the difference is
    /// remembered rather than returned: a phrase that is nowhere may be worth
    /// trying again a word shorter, while a phrase that is in several places
    /// that the context cannot separate is a refusal in the making. Where it is
    /// in several places, the context picks one only if that place's score
    /// (see `score_of`) is at least DECISIVE and beats the runner-up's by at
    /// least DECISIVE.
    fn find(&self, needle: &[char]) -> Option<Hit> {
        let mut hits = Vec::new();
        for (index, (_, flat)) in self.flattened.iter().enumerate() {
            for at in occurrences(&flat.chars, needle) {
                let score = score_of(flat, at, needle.len(), &self.prefix, &self.suffix);
                hits.push(Hit {
                    file: index,
                    at,
                    length: needle.len(),
                    score,
                });
            }
        }
        if hits.len() <= 1 {
            return hits.pop();
        }
        // Several places, so the words alone do not say which. What the page
        // had on either side of the selection does, if it agrees with one of
        // them and not the others -- and by enough letters to be an agreement
        // rather than a coincidence of a letter or two.
        hits.sort_by_key(|hit| std::cmp::Reverse(hit.score));
        let best = hits[0].score;
        let runner_up = hits[1].score;
        if best >= DECISIVE && best - runner_up >= DECISIVE {
            return hits.into_iter().next();
        }
        self.ambiguous.set(true);
        None
    }

    fn failure(&self) -> Failure {
        if self.ambiguous.get() {
            Failure::Ambiguous
        } else {
            Failure::NotFound
        }
    }
}

/// Turns a place in the flattened copy into a range in the file, and takes the
/// evidence that goes with it.
fn place(
    file: &Candidate<'_>,
    flat: &Flat,
    html: bool,
    at: usize,
    length: usize,
) -> SourceTextTarget {
    let index = TextIndex::new(file.text);
    let mut start = flat.from[at] as usize;
    let mut end = flat.from[at + length] as usize;

    // A word wrapped in markup with nothing between them -- `**bold**` -- has
    // flattened to the same characters as a bare word, so the map alone puts
    // the boundary on the letter rather than on the marker. What is wanted is
    // what the author actually wrote, so each edge walks out over the source
    // run that collapsed to get here, stopping at the first real whitespace.
    //
    // Only a run is walked, which is what a neighbouring space in the
    // flattened copy marks. Walking past a neighbour that is itself a
    // character would take a letter or a full stop that the selection did not
    // include. HTML has no such single-character markers to reclaim -- what
    // sits between two of its words is a tag, whose own text is not
    // whitespace either -- so it keeps the boundary where the words are.
    if !html && at > 0 && flat.chars[at - 1] == ' ' {
        let run_start = flat.from[at - 1] as usize;
        let mut cut = start;
        while cut > run_start && !is_space(unit_char(file.text, cut - 1)) {
            cut -= 1;
        }
        start = cut;
    }
    if !html
        && at + length < flat.chars.len()
        && flat.chars[at + length] == ' '
        && at + length + 1 < flat.from.len()
    {
        let run_end = flat.from[at + length + 1] as usize;
        let mut cut = end;
        while cut < run_end && !is_space(unit_char(file.text, cut)) {
            cut += 1;
        }
        end = cut;
    }
    // Whatever is left is trimmed inward rather than dropped, so whitespace
    // that made it this far never ends up inside the quote.
    while start < end && is_space(unit_char(file.text, start)) {
        start += 1;
    }
    while end > start && is_space(unit_char(file.text, end - 1)) {
        end -= 1;
    }

    SourceTextTarget {
        file_id: FileId(file.file_id.to_string()),
        start_utf16: start as u32,
        end_utf16: end as u32,
        // A passage grows at neither end when someone types against its
        // edges: the start belongs to what precedes it and the end to what
        // follows, which is what keeps an insertion beside a quotation out of
        // the quotation.
        start_side: AnchorSide::Left,
        end_side: AnchorSide::Right,
        exact: lossy(&index, start, end),
        prefix: lossy(&index, start.saturating_sub(CONTEXT), start),
        suffix: lossy(&index, end, (end + CONTEXT).min(index.len_utf16().0)),
    }
}

/// The source range an editor selected in the code pane, taken as it was
/// selected.
///
/// This is the one path that does not look for words: the selection is made
/// against the source itself, so there is no markup to walk and no whitespace
/// to trim. What is checked is only that the range is a range of this file --
/// not empty, not past the end, and not splitting a character. None of it is
/// identity; the caller compares the text it was given with `exact`.
pub fn at_range(file: &Candidate<'_>, start: usize, end: usize) -> Option<SourceTextTarget> {
    let index = TextIndex::new(file.text);
    if start >= end || end > index.len_utf16().0 {
        return None;
    }
    index.slice16(Span {
        start: Utf16(start),
        end: Utf16(end),
    })?;
    Some(SourceTextTarget {
        file_id: FileId(file.file_id.to_string()),
        start_utf16: u32::try_from(start).ok()?,
        end_utf16: u32::try_from(end).ok()?,
        start_side: AnchorSide::Left,
        end_side: AnchorSide::Right,
        exact: lossy(&index, start, end),
        prefix: lossy(&index, start.saturating_sub(CONTEXT), start),
        suffix: lossy(&index, end, (end + CONTEXT).min(index.len_utf16().0)),
    })
}

/// The text of a UTF-16 range for the evidence that goes with a target. A
/// bound inside a surrogate pair keeps the lone half as U+FFFD.
fn lossy(index: &TextIndex<'_>, start: usize, end: usize) -> String {
    index
        .slice16_lossy(Span {
            start: Utf16(start),
            end: Utf16(end),
        })
        .into_owned()
}

/// The character a UTF-16 offset lands on, for the boundary walks above. An
/// offset inside a surrogate pair lands on the whole character, which is not
/// whitespace, so it stops the walk as a lone surrogate would.
fn unit_char(text: &str, at: usize) -> char {
    let from = TextIndex::new(text).utf8_floor(Utf16(at)).0;
    text[from..].chars().next().unwrap_or('\u{fffd}')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one(text: &str) -> Vec<Candidate<'_>> {
        vec![Candidate {
            file_id: "f1",
            path: "paper.md",
            text,
        }]
    }

    fn quote<'a>(exact: &'a str, prefix: &'a str, suffix: &'a str) -> Quote<'a> {
        Quote {
            exact,
            prefix,
            suffix,
        }
    }

    #[test]
    fn a_plain_passage_is_found_where_it_is_written() {
        let text = "# Title\n\nThe interval covers the mean.\n";
        let found = locate(&one(text), &quote("covers the mean", "", "")).unwrap();
        assert_eq!(found.exact, "covers the mean");
        let index = TextIndex::new(text);
        assert_eq!(
            lossy(&index, found.start_utf16 as usize, found.end_utf16 as usize),
            "covers the mean"
        );
    }

    #[test]
    fn markup_around_a_word_is_taken_with_it() {
        // The page says "a bold claim"; the source says "a **bold** claim".
        let text = "We make a **bold** claim about intervals.\n";
        let found = locate(&one(text), &quote("a bold claim", "", "")).unwrap();
        assert_eq!(found.exact, "a **bold** claim");
    }

    #[test]
    fn a_passage_broken_over_two_lines_is_still_one_range() {
        let text = "The interval\ncovers the mean of the posterior.\n";
        let found = locate(&one(text), &quote("interval covers the mean", "", "")).unwrap();
        assert_eq!(found.exact, "interval\ncovers the mean");
    }

    #[test]
    fn context_tells_two_identical_sentences_apart() {
        // The prefix is what the page had before the selection, which is what
        // a browser sends: the heading and the paragraph before it, not the
        // markup that produced them.
        let text = "## One\n\nIt is true.\n\n## Two\n\nIt is true.\n";
        let found = locate(
            &one(text),
            &quote("It is true.", "One\nIt is true.\nTwo\n", ""),
        )
        .unwrap();
        let index = TextIndex::new(text);
        let before = lossy(&index, 0, found.start_utf16 as usize);
        assert!(before.contains("## Two"), "picked the first occurrence");
    }

    #[test]
    fn context_that_agrees_only_by_a_space_decides_nothing() {
        // Both occurrences follow a space, and that is the whole of the
        // agreement. One character of coincidence is not evidence.
        let text = "Said: It is true. Also: It is true.\n";
        assert_eq!(
            locate(&one(text), &quote("It is true.", " ", "")).unwrap_err(),
            Failure::Ambiguous
        );
    }

    #[test]
    fn identical_sentences_with_nothing_to_tell_them_apart_are_refused() {
        let text = "It is true.\n\nIt is true.\n";
        assert_eq!(
            locate(&one(text), &quote("It is true.", "", "")).unwrap_err(),
            Failure::Ambiguous
        );
    }

    #[test]
    fn words_that_are_not_in_the_source_are_not_placed() {
        let text = "The interval covers the mean.\n";
        assert_eq!(
            locate(&one(text), &quote("a sentence from somewhere else", "", "")).unwrap_err(),
            Failure::NotFound
        );
    }

    #[test]
    fn a_selection_of_pure_markup_has_nothing_to_comment_on() {
        assert_eq!(
            locate(&one("# Title\n"), &quote("###", "", "")).unwrap_err(),
            Failure::Empty
        );
    }

    #[test]
    fn a_selection_running_into_invented_text_keeps_the_words_that_are_real() {
        // "Figure 1:" is the renderer's, not the author's: the selection is
        // shortened until what is left is what was actually written.
        let text = "See the figure below.\n\n![A plot](plot.png)\n";
        let found = locate(
            &one(text),
            &quote("See the figure below. Figure 1: A plot", "", ""),
        )
        .unwrap();
        assert_eq!(found.exact, "See the figure below.");
    }

    #[test]
    fn a_selection_starting_in_invented_text_keeps_the_words_that_are_real() {
        // The page reads "(Lovelace, 1843). Type ..." where the source says
        // "[@lovelace1843]. Type ...", so a selection that starts at the
        // citation starts on words no file contains. Trimming the tail can
        // only shorten it into more of what was never there; trimming the
        // head is what recovers the sentence the reader was pointing at.
        let text = "Weaving algebraic patterns [@lovelace1843]. Type `@` to cite another entry.\n";
        let found = locate(
            &one(text),
            &quote(
                "(Lovelace, 1843). Type @ to cite another entry.",
                "Weaving algebraic patterns ",
                "",
            ),
        )
        .unwrap();
        assert!(
            found.exact.ends_with("Type `@` to cite another entry."),
            "{:?}",
            found.exact
        );
        // Never a range that says one thing and covers another.
        let index = TextIndex::new(text);
        assert_eq!(
            lossy(&index, found.start_utf16 as usize, found.end_utf16 as usize),
            found.exact
        );
    }

    #[test]
    fn the_longer_survival_is_the_one_kept() {
        // Invented text in the middle: both directions land, and the tail is
        // the larger part of what was selected, so the tail is the answer.
        let text = "A short lead [@ref2020] and then a good deal more prose after it.\n";
        let found = locate(
            &one(text),
            &quote(
                "lead (Ref, 2020) and then a good deal more prose after it.",
                "",
                "",
            ),
        )
        .unwrap();
        assert!(
            found.exact.contains("a good deal more prose after it."),
            "{:?}",
            found.exact
        );
    }

    #[test]
    fn the_right_file_of_several_is_chosen() {
        let files = vec![
            Candidate {
                file_id: "f1",
                path: "paper.md",
                text: "Introduction here.\n",
            },
            Candidate {
                file_id: "f2",
                path: "methods.md",
                text: "We fit the model by maximum likelihood.\n",
            },
        ];
        let found = locate(&files, &quote("by maximum likelihood", "", "")).unwrap();
        assert_eq!(found.file_id.0, "f2");
    }

    #[test]
    fn html_tags_and_entities_flatten_to_their_prose() {
        let files = vec![Candidate {
            file_id: "f1",
            path: "index.html",
            text: "<p>Tea &amp; coffee, <em>strong</em> ones.</p>\n",
        }];
        let found = locate(&files, &quote("Tea & coffee", "", "")).unwrap();
        assert_eq!(found.exact, "Tea &amp; coffee");
    }

    #[test]
    fn named_entities_beyond_the_basic_six_anchor() {
        let files = vec![Candidate {
            file_id: "f1",
            path: "index.html",
            text: "<p>A caf&eacute; &mdash; open late.</p>\n",
        }];
        let found = locate(&files, &quote("caf\u{e9} \u{2014} open", "", "")).unwrap();
        assert_eq!(found.exact, "caf&eacute; &mdash; open");
    }

    #[test]
    fn a_long_entity_name_anchors_and_keeps_utf16_offsets() {
        let text = "<p>\u{1f600} a &LeftRightArrow; b</p>\n";
        let files = vec![Candidate {
            file_id: "f1",
            path: "index.html",
            text,
        }];
        let found = locate(&files, &quote("a \u{2194} b", "", "")).unwrap();
        let index = TextIndex::new(text);
        assert_eq!(
            lossy(&index, found.start_utf16 as usize, found.end_utf16 as usize),
            "a &LeftRightArrow; b"
        );
        assert_eq!(found.exact, "a &LeftRightArrow; b");
    }

    #[test]
    fn offsets_are_utf16_so_astral_characters_do_not_shift_a_range() {
        let text = "An emoji 😀 then the interval covers the mean.\n";
        let found = locate(&one(text), &quote("the interval covers", "", "")).unwrap();
        let index = TextIndex::new(text);
        assert_eq!(
            lossy(&index, found.start_utf16 as usize, found.end_utf16 as usize),
            "the interval covers"
        );
        assert_eq!(found.exact, "the interval covers");
    }

    #[test]
    fn the_context_kept_is_the_source_around_the_range() {
        let text = "Before it. The interval covers the mean. After it.\n";
        let found = locate(&one(text), &quote("The interval covers the mean.", "", "")).unwrap();
        assert!(found.prefix.ends_with("Before it. "));
        assert!(found.suffix.starts_with(" After it."));
    }

    fn file(text: &str) -> Candidate<'_> {
        Candidate {
            file_id: "f1",
            path: "paper.md",
            text,
        }
    }

    #[test]
    fn a_selected_range_is_taken_exactly_as_selected() {
        let text = "The interval covers the mean.\n";
        let found = at_range(&file(text), 4, 12).unwrap();
        assert_eq!(found.file_id.0, "f1");
        assert_eq!((found.start_utf16, found.end_utf16), (4, 12));
        assert_eq!(found.exact, "interval");
        assert_eq!(found.start_side, AnchorSide::Left);
        assert_eq!(found.end_side, AnchorSide::Right);
        assert_eq!(found.prefix, "The ");
        assert!(found.suffix.starts_with(" covers the mean."));
        // Whitespace the editor selected stays in the range.
        let found = at_range(&file(text), 3, 12).unwrap();
        assert_eq!(found.exact, " interval");
    }

    #[test]
    fn an_empty_or_inverted_range_is_not_a_range() {
        let text = "The interval covers the mean.\n";
        assert!(at_range(&file(text), 3, 3).is_none());
        assert!(at_range(&file(text), 5, 4).is_none());
    }

    #[test]
    fn a_range_past_the_end_of_the_file_is_refused() {
        let text = "naive done";
        assert_eq!(TextIndex::new(text).len_utf16().0, 10);
        assert!(at_range(&file(text), 0, 11).is_none());
        assert!(at_range(&file(text), 0, 10).is_some());
    }

    /// `𝔸` is two UTF-16 units, so a range that starts or ends inside it
    /// splits a character and is refused.
    #[test]
    fn a_range_that_splits_a_surrogate_pair_is_refused() {
        let text = "na\u{ef}ve \u{1d538}x done";
        // n a ï v e space 𝔸(2 units) x space d o n e: 14 units.
        assert_eq!(TextIndex::new(text).len_utf16().0, 14);
        assert!(at_range(&file(text), 7, 9).is_none());
        assert!(at_range(&file(text), 6, 7).is_none());
        // The space before the pair is a boundary, so this one is a range.
        assert!(at_range(&file(text), 5, 6).is_some());
    }

    /// Offsets are UTF-16, so the characters before a range count as the
    /// browser counts them: `ï` is one unit, `𝔸` is two.
    #[test]
    fn a_range_after_non_ascii_text_uses_utf16_offsets() {
        let text = "na\u{ef}ve \u{1d538}x done";
        let found = at_range(&file(text), 2, 5).unwrap();
        assert_eq!(found.exact, "\u{ef}ve");
        let found = at_range(&file(text), 6, 9).unwrap();
        assert_eq!(found.exact, "\u{1d538}x");
        assert_eq!((found.start_utf16, found.end_utf16), (6, 9));
        assert_eq!(found.prefix, "na\u{ef}ve ");
        assert_eq!(found.suffix, " done");
    }

    // The page has no bullets, so its context has no `- ` where the source has
    // one. The two lists differ only in their letters around the word, which is
    // what has to tell them apart.
    #[test]
    fn repeated_list_items_are_told_apart_past_the_bullets() {
        let text = "- `source` (object)\n    - `category_fit` (one of `fits`, `miscategorized`, `uncertain`)\n    - `rationale` (string)\n- `target` (object)\n    - `category_fit` (one of `fits`, `miscategorized`, `uncertain`)\n    - `rationale` (string)\n- `target_kind` (one of `fixed_attribute`, `other`)\n";
        let found = locate(
            &one(text),
            &quote(
                "rationale",
                "source (object)\ncategory_fit (one of fits, miscategorized, uncertain)\nrationale (string)\ntarget (object)\ncategory_fit (one of fits, miscategorized, uncertain)\n",
                " (string)\ntarget_kind (one of fixed_attribute, other)",
            ),
        )
        .unwrap();
        let index = TextIndex::new(text);
        let before = lossy(&index, 0, found.start_utf16 as usize);
        assert!(before.contains("`target` (object)"), "{:?}", before);
    }

    // The marker that starts the second item is markup, so the selection's
    // words meet across it and the range runs from one item into the next.
    #[test]
    fn a_selection_across_two_list_items_matches_over_the_dropped_marker() {
        let text = "- alpha beta\n- gamma delta\n";
        let found = locate(&one(text), &quote("beta\ngamma", "", "")).unwrap();
        assert_eq!(found.exact, "beta\n- gamma");
    }

    // A marker is a bullet or a number at the start of a line followed by a
    // space. A hyphen inside a word is part of the word.
    #[test]
    fn list_markers_are_blanked_but_a_hyphen_in_a_word_is_not() {
        assert!(flatten("well-known", false).chars.contains(&'-'));
        let numbered: String = flatten("1. one\n", false).chars.iter().collect();
        assert_eq!(numbered.trim(), "one");
    }

    // The page says "[3]" where the source says nothing: a number the renderer
    // invented. It costs the context one letter, and does not stop the match.
    #[test]
    fn an_invented_number_in_the_context_costs_a_letter_not_the_match() {
        let text = "See Smith for this. It is true.\n\nSee Jones for this. It is true.\n";
        let found = locate(
            &one(text),
            &quote("It is true.", "See Jones [3] for this. ", ""),
        )
        .unwrap();
        let index = TextIndex::new(text);
        let before = lossy(&index, 0, found.start_utf16 as usize);
        assert!(before.contains("See Jones"), "{:?}", before);
    }
}

#[cfg(test)]
mod html_whitespace_tests {
    use super::*;

    /// `&nbsp;` is a space, and a space beside a space is one space -- in the
    /// file as in the selection, or a passage quoted with one space in it
    /// would not be found in a source that wrote two.
    #[test]
    fn a_decoded_space_collapses_like_any_other() {
        let files = vec![Candidate {
            file_id: "f1",
            path: "index.html",
            text: "<p>Tea &nbsp; and coffee, together.</p>\n",
        }];
        let found = locate(
            &files,
            &Quote {
                exact: "Tea and coffee",
                prefix: "",
                suffix: "",
            },
        )
        .unwrap();
        assert_eq!(found.exact, "Tea &nbsp; and coffee");
    }

    /// An `&` that is not an entity, with a character straddling the twelfth
    /// byte after it. The window an entity is looked for in is bytes, and the
    /// character is not: slicing one in half is a panic, and it took nothing
    /// more than an ampersand and a euro sign in an HTML source to reach it.
    #[test]
    fn an_ampersand_before_a_wide_character_is_not_sliced_in_half() {
        let files = vec![Candidate {
            file_id: "f1",
            path: "index.html",
            text: "<p>Tea &aaaaaaaaaa\u{20ac}; and coffee, together.</p>\n",
        }];
        // The point is that this returns at all.
        let _ = locate(
            &files,
            &Quote {
                exact: "and coffee",
                prefix: "",
                suffix: "",
            },
        );
    }
}
