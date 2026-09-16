//! A written history for a seeded example.
//!
//! An example document arrives finished: one publish, one moment, nothing to
//! look at on a timeline. That makes the history panel impossible to see
//! working on a fresh deployment, and impossible to design against -- a
//! calendar with one cell in it teaches nobody anything.
//!
//! So this writes the example the way somebody would have: in sittings, over
//! a few days, with weekends and quiet days in between, and with the drafting
//! that real writing has in it -- paragraphs put down in the wrong words and
//! written again, asides that go in and come back out. The stand-in prose is
//! lorem ipsum, because the point of it is to be cut.
//!
//! **The operations are real.** The document genuinely grows piece by piece,
//! every frontier recorded here is a moment `fork_at` can reproduce, every
//! version is an archive somebody can open and compare, and the last of them
//! is byte-for-byte the published source. Only the clock is invented, and it
//! is invented in the two places a clock is a column: `document_activity
//! .bucket` and `document_versions.created_at`.
//!
//! Nothing here runs unless an operator asks for it (`admin seed
//! --simulate-activity <DAYS>`), and the seed says what it wrote for each
//! example. A deployment that does not ask keeps the honest history of a
//! document published once. It is for demonstration deployments and for
//! anyone designing against the panel; it is not something to do to documents
//! people believe they wrote.

use std::collections::BTreeMap;
use std::sync::Arc;

use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use crate::document::session;
use crate::storage::blob::BlobStore;
use crate::storage::collaboration::CollaborationStorage;
use crate::storage::postgres::PostgresCatalog;
use crate::storage::source::{CommitArchive, SourceStorage};
use crate::storage::source_archive::{SourceArchive, SourceFile};

/// What a simulation left behind, for the operator to read back.
pub struct Simulated {
    pub steps: usize,
    pub versions: usize,
    pub buckets: usize,
    pub first: OffsetDateTime,
}

/// How far back a simulation may reach.
///
/// Routine retention drops a version older than thirty days on the next write
/// to the same document, so a history written further back than this would
/// delete itself as it was being written. Twenty-eight days is four whole
/// weeks, which is also what the calendar reads best over.
const MAX_DAYS: u32 = 28;

/// How many versions a simulated history may hold. Retention keeps the fifty
/// newest; staying under that means every version written here survives the
/// writing of the next one.
const MAX_VERSIONS: usize = 40;

/// The same deployment, seeded twice, should look the same twice: an example
/// whose calendar reshuffles on every seed is a moving target for anyone
/// designing against it. This is a small LCG over a hash of the slug, which
/// is all the randomness a plausible-looking month needs.
struct Dice(u64);

impl Dice {
    fn new(seed: &str) -> Self {
        let mut hash = 0xcbf2_9ce4_8422_2325_u64;
        for byte in seed.bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        Self(hash)
    }

    fn roll(&mut self, upper: u64) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 33) % upper.max(1)
    }
}

/// The words somebody types when they are drafting rather than writing.
///
/// Lorem ipsum is the right stand-in precisely because it says nothing: what
/// goes in at this step is going out again at the next one, and prose that
/// meant something would invite a reader of the example to take a deleted
/// paragraph for a deleted thought.
const LOREM: [&str; 48] = [
    "lorem",
    "ipsum",
    "dolor",
    "sit",
    "amet",
    "consectetur",
    "adipiscing",
    "elit",
    "sed",
    "do",
    "eiusmod",
    "tempor",
    "incididunt",
    "ut",
    "labore",
    "et",
    "dolore",
    "magna",
    "aliqua",
    "enim",
    "ad",
    "minim",
    "veniam",
    "quis",
    "nostrud",
    "exercitation",
    "ullamco",
    "laboris",
    "nisi",
    "aliquip",
    "ex",
    "ea",
    "commodo",
    "consequat",
    "duis",
    "aute",
    "irure",
    "in",
    "reprehenderit",
    "voluptate",
    "velit",
    "esse",
    "cillum",
    "fugiat",
    "nulla",
    "pariatur",
    "excepteur",
    "sint",
];

fn lorem_sentence(dice: &mut Dice) -> String {
    let words = 7 + dice.roll(12) as usize;
    let mut out = String::new();
    for index in 0..words {
        let word = LOREM[dice.roll(LOREM.len() as u64) as usize];
        if index == 0 {
            let mut letters = word.chars();
            match letters.next() {
                Some(first) => {
                    out.extend(first.to_uppercase());
                    out.push_str(letters.as_str());
                }
                None => out.push_str(word),
            }
        } else {
            out.push(' ');
            out.push_str(word);
        }
        if index + 2 < words && dice.roll(9) == 0 {
            out.push(',');
        }
    }
    out.push('.');
    out
}

/// A paragraph of draft, ending the way a paragraph ends so that whatever is
/// written after it keeps its own shape.
fn lorem_paragraph(dice: &mut Dice) -> String {
    let mut out = String::new();
    for _ in 0..2 + dice.roll(3) {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(&lorem_sentence(dice));
    }
    out.push_str("\n\n");
    out
}

/// Roughly `length` bytes of draft: the shape of the passage somebody is
/// about to write, standing in for the passage until they write it.
fn lorem_like(dice: &mut Dice, length: usize) -> String {
    let mut out = String::new();
    while out.len() < length.max(60) {
        out.push_str(&lorem_sentence(dice));
        out.push(' ');
    }
    out.push_str("\n\n");
    out
}

/// The pieces somebody writes between one pause and the next.
///
/// Paragraphs are the unit a person stops at, and a long one is cut at its
/// line breaks into pieces of about a sentence or two, so that a sitting is
/// several minutes of writing rather than one keystroke of it. Every break is
/// on a boundary the text already has, so the pieces reassemble into exactly
/// the document that was published.
fn pieces(body: &str) -> Vec<String> {
    const MOUTHFUL: usize = 180;
    let mut out: Vec<String> = Vec::new();
    for paragraph in body.split_inclusive("\n\n") {
        let mut rest = paragraph;
        while rest.len() > MOUTHFUL {
            let cut = rest[..MOUTHFUL]
                .rfind('\n')
                .or_else(|| rest[..MOUTHFUL].rfind(". "))
                .map_or(MOUTHFUL, |at| at + 1);
            let (piece, tail) = rest.split_at(cut.min(rest.len()));
            out.push(piece.to_string());
            rest = tail;
        }
        if !rest.is_empty() {
            out.push(rest.to_string());
        }
    }
    if out.is_empty() {
        out.push(body.to_string());
    }
    out
}

/// What somebody did at one step, which is what a version taken there would
/// be recording.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    /// The project arriving: its figures, and an entrypoint with nothing in
    /// it yet.
    Arrive,
    /// A piece of the paper, written.
    Write,
    /// Words put down to be replaced: the passage in the wrong words, or an
    /// aside that is about to come back out.
    Draft,
    /// The draft, taken out again.
    Cut,
}

/// One moment in the writing: what the entrypoint said then, which of the
/// other files had been written by then, and what the author was doing.
#[derive(Clone, Debug)]
struct Step {
    body: String,
    arriving: Vec<String>,
    kind: Kind,
}

impl Step {
    fn at(body: &str, kind: Kind) -> Self {
        Self {
            body: body.to_string(),
            arriving: Vec::new(),
            kind,
        }
    }
}

/// How the document was written: every state the entrypoint passed through,
/// in order, ending on exactly what was published.
///
/// Writing is not appending. Most pieces go in and stay, but about one in six
/// goes in as lorem first and is written again at the next step, and about
/// one in six is followed by an aside that comes back out -- so the history
/// has real deletions in it, the size bars go both ways, and comparing two
/// versions shows something other than a longer document.
fn script(main_body: &str, others: &[String], dice: &mut Dice) -> Vec<Step> {
    let parts = pieces(main_body);
    let mut steps = vec![Step::at("", Kind::Arrive)];
    let mut written = String::new();
    for piece in &parts {
        match dice.roll(6) {
            // Put down in the wrong words, then written properly.
            0 => {
                let draft = format!("{written}{}", lorem_like(dice, piece.len()));
                steps.push(Step::at(&draft, Kind::Draft));
                written.push_str(piece);
                steps.push(Step::at(&written, Kind::Write));
            }
            // Written, then followed somewhere that turned out not to belong.
            1 => {
                written.push_str(piece);
                steps.push(Step::at(&written, Kind::Write));
                let aside = format!("{written}{}", lorem_paragraph(dice));
                steps.push(Step::at(&aside, Kind::Draft));
                steps.push(Step::at(&written, Kind::Cut));
            }
            _ => {
                written.push_str(piece);
                steps.push(Step::at(&written, Kind::Write));
            }
        }
    }
    // The rest of the project arrives while the paper is being written rather
    // than all at once at the start: a bibliography grows with the citations
    // that need it. Spread through the first half, so every one of them is in
    // place well before the end.
    let last = steps.len() - 1;
    for (index, path) in others.iter().enumerate() {
        let at = (1 + (index * last) / (others.len() * 2).max(1)).min(last);
        steps[at].arriving.push(path.clone());
    }
    steps
}

/// When each step happened.
///
/// Work happens in sittings -- an hour at a desk, once or twice on a working
/// day -- and the point of the calendar is that those are visible: a document
/// written one minute per day for a month is a flat grid that teaches nobody
/// what the view is for. So the days are chosen first (no weekends, and
/// roughly one weekday in six quiet), then one to three sittings on each, and
/// the steps are dealt into those sittings in order, a minute or three apart.
/// The document grows forwards, and the last step lands on the most recent
/// working day so the example does not look abandoned.
fn schedule(count: usize, days: u32, dice: &mut Dice, now: OffsetDateTime) -> Vec<OffsetDateTime> {
    const HOURS: [i64; 7] = [9, 10, 11, 14, 15, 16, 21];
    // The days somebody could have worked: weekdays, less roughly one in six
    // that went to something else.
    let mut working: Vec<OffsetDateTime> = Vec::new();
    for back in (0..days.clamp(1, MAX_DAYS) as i64).rev() {
        let day = now - Duration::days(back);
        if day.weekday().number_days_from_monday() >= 5 || dice.roll(6) == 0 {
            continue;
        }
        working.push(day.replace_time(time::Time::MIDNIGHT));
    }
    if working.is_empty() {
        working.push(now.replace_time(time::Time::MIDNIGHT));
    }
    // How many sittings there were is a fact about the writing, not about the
    // calendar: about four steps to a sitting. Deciding it from the days
    // instead spreads one step per day over a month, which is a flat grid and
    // the opposite of what the view is for.
    let wanted = (count.div_ceil(4)).clamp(2, working.len() * 2);
    let mut sittings: Vec<OffsetDateTime> = Vec::new();
    for index in 0..wanted {
        let day = working[(index * working.len()) / wanted];
        let hour = HOURS[dice.roll(HOURS.len() as u64) as usize];
        let sitting = day + Duration::hours(hour) + Duration::minutes(dice.roll(40) as i64);
        // Now and then somebody came back to it after dinner.
        if sittings.last() == Some(&sitting) {
            sittings.push(sitting + Duration::hours(3));
        } else {
            sittings.push(sitting);
        }
    }
    sittings.sort();
    sittings.dedup();
    // Deal the steps across the sittings in order: a sitting holds a run of
    // them, a minute or three apart, which is what makes a burst a burst.
    let mut when = Vec::with_capacity(count);
    let (mut previous, mut since) = (usize::MAX, 0i64);
    for index in 0..count {
        let at = ((index * sittings.len()) / count.max(1)).min(sittings.len() - 1);
        if at == previous {
            since += 1 + dice.roll(3) as i64;
        } else {
            (previous, since) = (at, 0);
        }
        when.push(sittings[at] + Duration::minutes(since));
    }
    when.sort();
    when
}

/// Which steps somebody stopped and saved.
///
/// Not all of them. A version a minute is a timeline nobody reads, and the
/// panel is at its most useful when a day holds a handful rather than a
/// column. So: the project arriving, the last step of every day, roughly one
/// step in three besides, and always the last -- which is what the document
/// is now. If that still comes to more than the store keeps, the earliest are
/// dropped, which is what retention would do to them anyway.
fn saved(when: &[OffsetDateTime], dice: &mut Dice) -> Vec<bool> {
    let mut take: Vec<bool> = when
        .iter()
        .enumerate()
        .map(|(index, at)| {
            let last_of_day =
                !matches!(when.get(index + 1), Some(next) if next.date() == at.date());
            last_of_day || dice.roll(3) == 0
        })
        .collect();
    if let Some(first) = take.first_mut() {
        *first = true;
    }
    if let Some(last) = take.last_mut() {
        *last = true;
    }
    let mut over = take
        .iter()
        .filter(|&&on| on)
        .count()
        .saturating_sub(MAX_VERSIONS);
    for (index, on) in take.iter_mut().enumerate() {
        if over == 0 {
            break;
        }
        if *on && index > 0 {
            *on = false;
            over -= 1;
        }
    }
    take
}

/// The two or three versions somebody bothered to name, by step.
///
/// A history where nothing is named shows only half of what the panel does --
/// the names are the one way into it that is not a date. They are placed by
/// proportion rather than by content, because this knows nothing about what
/// the example says.
fn names(take: &[bool]) -> BTreeMap<usize, String> {
    const NAMED: [(usize, &str); 3] = [
        (25, "First draft"),
        (60, "Sent to coauthors"),
        (90, "Ready to submit"),
    ];
    let chosen: Vec<usize> = take
        .iter()
        .enumerate()
        .filter_map(|(index, &on)| on.then_some(index))
        .collect();
    // With three or fewer versions there is nothing to navigate, so naming
    // them would be labelling the whole list.
    if chosen.len() < 4 {
        return BTreeMap::new();
    }
    NAMED
        .iter()
        .map(|(percent, name)| {
            let at = (chosen.len() - 1) * percent / 100;
            (chosen[at], (*name).to_string())
        })
        .collect()
}

/// What changed between one version and the next. The figures never move
/// after the project arrives, so this is the texts alone -- and a path the
/// step before did not have is a path that changed.
fn changed(before: &BTreeMap<String, String>, now: &BTreeMap<String, String>) -> Vec<String> {
    let mut paths: Vec<String> = now
        .iter()
        .filter(|(path, body)| before.get(*path) != Some(body))
        .map(|(path, _)| path.clone())
        .collect();
    paths.extend(
        before
            .keys()
            .filter(|path| !now.contains_key(*path))
            .cloned(),
    );
    paths.sort();
    paths.dedup();
    paths
}

/// Write `document_id`'s example as a history: real operations, invented
/// times. Returns what it wrote.
///
/// Refuses a document that already has a collaboration base: that is a
/// document somebody has opened and possibly typed in, and its history is not
/// ours to replace.
pub async fn simulate(
    catalog: Arc<PostgresCatalog>,
    blobs: Arc<dyn BlobStore>,
    document_id: Uuid,
    slug: &str,
    days: u32,
) -> Result<Simulated, String> {
    let state = catalog
        .collaboration_state(document_id)
        .await
        .map_err(|error| error.to_string())?;
    if state.base.is_some() || !state.updates.is_empty() {
        return Err(format!("{slug} already has a history of its own"));
    }
    let sources = SourceStorage::new(catalog.clone(), blobs.clone(), Default::default());
    let project = sources
        .read_current(document_id)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("{slug} has no published source"))?;
    let archive = project.archive;
    let main_path = archive.main_path.clone();

    // The three kinds of thing a project holds, told apart: the entrypoint,
    // which is written over the whole history; the other texts, which arrive
    // partway through; and the figures, which are there from the start
    // because that is how somebody works -- the plot before the paragraph
    // about it.
    let mut main_body = String::new();
    let mut texts: BTreeMap<String, String> = BTreeMap::new();
    let mut assets: Vec<SourceFile> = Vec::new();
    for file in &archive.files {
        match file {
            SourceFile::Inline { path, bytes } => {
                let body = String::from_utf8_lossy(bytes).to_string();
                if *path == main_path {
                    main_body = body;
                } else {
                    texts.insert(path.clone(), body);
                }
            }
            SourceFile::Asset { .. } => assets.push(file.clone()),
        }
    }

    let mut dice = Dice::new(slug);
    let now = OffsetDateTime::now_utc();
    let others: Vec<String> = texts.keys().cloned().collect();
    let steps = script(&main_body, &others, &mut dice);
    let when = schedule(steps.len(), days, &mut dice, now);
    let take = saved(&when, &mut dice);
    let mut named = names(&take);

    let document = catalog
        .document_by_slug(slug)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("{slug} disappeared while being written"))?;
    let author = catalog
        .account(document.owner_id)
        .await
        .map_err(|error| error.to_string())?
        .map_or_else(|| "Examples".to_string(), |account| account.display_name);

    let doc = session::new_doc();
    for file in &assets {
        if let SourceFile::Asset { path, digest, .. } = file {
            session::put_asset(&doc, path, &hex::encode(digest));
        }
    }
    // The entrypoint is named before anything is written into it, because
    // every moment recorded below is a document somebody can open, and a
    // document with no main file is not one -- that is what a reader would
    // have been shown for every moment but the last.
    let main_id = session::put_text(&doc, &main_path, "");
    session::set_main(&doc, &main_id);

    let mut marks: Vec<(OffsetDateTime, i64, Vec<u8>)> = Vec::new();
    let mut present: Vec<String> = Vec::new();
    let mut before: BTreeMap<String, String> = BTreeMap::new();
    let mut versions = 0usize;
    for (index, step) in steps.iter().enumerate() {
        for path in &step.arriving {
            let body = texts.get(path).map_or("", String::as_str);
            session::put_text(&doc, path, body);
            present.push(path.clone());
        }
        session::put_text(&doc, &main_path, &step.body);
        doc.commit();
        marks.push((when[index], encoded(&doc), frontier(&doc)));
        if !take[index] {
            continue;
        }
        let mut held: BTreeMap<String, String> = BTreeMap::new();
        held.insert(main_path.clone(), step.body.clone());
        for path in &present {
            held.insert(path.clone(), texts.get(path).cloned().unwrap_or_default());
        }
        let mut files: Vec<SourceFile> = held
            .iter()
            .map(|(path, body)| SourceFile::Inline {
                path: path.clone(),
                bytes: body.as_bytes().to_vec(),
            })
            .collect();
        files.extend(assets.iter().cloned());
        let moved = if index == 0 {
            files.iter().map(|file| file.path().to_string()).collect()
        } else {
            changed(&before, &held)
        };
        let stored = sources
            .commit_archive(CommitArchive {
                document_id,
                archive: SourceArchive {
                    source_format: document.source_format.clone(),
                    main_path: main_path.clone(),
                    files,
                },
                through_update_sequence: document.update_sequence,
                project_generation: document.project_generation,
                tree_digest: None,
                changed_paths: Some(moved),
                reason: match step.kind {
                    Kind::Arrive => "cli",
                    _ => "quiet",
                }
                .into(),
                label: named.remove(&index),
                author_account_id: Some(document.owner_id),
                author_label: author.clone(),
                make_current: true,
            })
            .await
            .map_err(|error| error.to_string())?;
        // The version is written now and happened then. This is the second of
        // the two invented clocks, and the only thing separating a simulated
        // history from one somebody typed.
        catalog
            .backdate_version(stored.version.id, when[index])
            .await
            .map_err(|error| error.to_string())?;
        before = held;
        versions += 1;
    }
    // What was written must be what was published; a simulation that drifts
    // from the archive would hand a reader a document nobody has.
    if steps.last().map(|step| step.body.as_str()) != Some(main_body.as_str()) {
        return Err(format!(
            "{slug} did not reassemble into its published source"
        ));
    }
    if present.len() != texts.len() {
        return Err(format!("{slug} lost a file while being written"));
    }

    // The history is the base; there are no update rows to compact, because
    // nobody was connected while this was written.
    CollaborationStorage::new(catalog.clone(), blobs)
        .compact(
            document_id,
            document.update_sequence,
            document.project_generation,
            &session::encode_state(&doc),
        )
        .await
        .map_err(|error| error.to_string())?;

    let buckets = catalog
        .record_activity(document_id, &marks)
        .await
        .map_err(|error| error.to_string())?;
    Ok(Simulated {
        steps: marks.len(),
        versions,
        buckets,
        first: marks.first().map_or(now, |mark| mark.0),
    })
}

fn frontier(doc: &loro::LoroDoc) -> Vec<u8> {
    doc.state_frontiers().encode()
}

fn encoded(doc: &loro::LoroDoc) -> i64 {
    session::encode_state(doc).len() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAPER: &str = "\\documentclass{article}\n\n\\begin{document}\n\nA first paragraph, long enough that it is cut into more than one piece by the splitting above, which is what makes a sitting several minutes of writing rather than one. It goes on for a while yet, saying very little.\n\nA second paragraph.\n\n\\end{document}\n";

    #[test]
    fn the_pieces_reassemble_into_the_document() {
        let parts = pieces(PAPER);
        assert!(
            parts.len() > 3,
            "a paper this long is more than one sitting: {parts:?}"
        );
        assert_eq!(
            parts.concat(),
            PAPER,
            "a simulation must not change the document"
        );
        // An empty document is still one piece, so the schedule below has
        // something to place.
        assert_eq!(pieces("").len(), 1);
    }

    #[test]
    fn the_writing_drafts_and_deletes_and_still_lands_on_the_paper() {
        let mut dice = Dice::new("learn-librepaper-with-latex");
        let others = vec![
            "references.bib".to_string(),
            "sections/intro.tex".to_string(),
        ];
        let steps = script(PAPER, &others, &mut dice);
        assert_eq!(steps[0].kind, Kind::Arrive);
        assert!(steps[0].body.is_empty(), "the entrypoint arrives empty");
        assert_eq!(
            steps.last().map(|step| step.body.as_str()),
            Some(PAPER),
            "however it was written, it ends as what was published"
        );
        assert!(
            steps.iter().any(|step| step.kind == Kind::Draft),
            "nothing was ever put down to be replaced: {steps:?}"
        );
        assert!(
            steps
                .iter()
                .zip(steps.iter().skip(1))
                .any(|(before, after)| after.body.len() < before.body.len()),
            "the document only ever grew, so the history has no deletions in it"
        );
        // Every other file is written exactly once, before the end.
        let arriving: Vec<&String> = steps.iter().flat_map(|step| &step.arriving).collect();
        assert_eq!(arriving.len(), others.len());
        assert!(steps.last().is_some_and(|step| step.arriving.is_empty()));
        // Deterministic: the same example is written the same way every time.
        let mut again = Dice::new("learn-librepaper-with-latex");
        let repeated = script(PAPER, &others, &mut again);
        assert_eq!(
            repeated
                .iter()
                .map(|step| step.body.clone())
                .collect::<Vec<_>>(),
            steps
                .iter()
                .map(|step| step.body.clone())
                .collect::<Vec<_>>(),
        );
    }

    #[test]
    fn the_work_arrives_in_bursts_on_working_days() {
        let mut dice = Dice::new("learn-librepaper-with-latex");
        // A Wednesday, so the month below contains whole weeks.
        let now = time::OffsetDateTime::from_unix_timestamp(1_789_000_000).unwrap();
        let when = schedule(24, 30, &mut dice, now);
        assert_eq!(when.len(), 24);
        assert!(
            when.windows(2).all(|pair| pair[0] <= pair[1]),
            "the document grows forwards"
        );

        let mut by_day: std::collections::BTreeMap<time::Date, usize> = Default::default();
        for at in &when {
            *by_day.entry(at.date()).or_default() += 1;
        }
        assert!(
            by_day.len() <= 12,
            "twenty-four writes spread over {} days is not bursts",
            by_day.len()
        );
        assert!(
            by_day.values().any(|&count| count >= 3),
            "no day holds a sitting: {by_day:?}"
        );
        assert!(
            by_day
                .keys()
                .all(|day| day.weekday().number_days_from_monday() < 5),
            "somebody worked the weekend: {by_day:?}"
        );
        assert!(
            when.iter().all(|at| (9..=22).contains(&at.hour())),
            "somebody wrote at an implausible hour"
        );
        // Never further back than retention keeps, however many days are
        // asked for: a version older than that deletes itself on the next
        // write.
        let far = schedule(24, 365, &mut dice, now);
        assert!(
            far.first()
                .is_some_and(|at| *at >= now - Duration::days(MAX_DAYS as i64)),
            "a simulation reached back past what the store keeps"
        );
        // Deterministic: the same example seeds the same month every time.
        let mut again = Dice::new("learn-librepaper-with-latex");
        assert_eq!(schedule(24, 30, &mut again, now), when);
    }

    #[test]
    fn some_steps_are_saved_and_a_few_of_those_are_named() {
        let mut dice = Dice::new("learn-librepaper-with-latex");
        let now = time::OffsetDateTime::from_unix_timestamp(1_789_000_000).unwrap();
        let when = schedule(40, 28, &mut dice, now);
        let take = saved(&when, &mut dice);
        assert_eq!(take.len(), when.len());
        assert!(take[0], "the project arriving is a version");
        assert!(take[take.len() - 1], "and so is what the document is now");
        let count = take.iter().filter(|&&on| on).count();
        assert!(
            count <= MAX_VERSIONS,
            "{count} versions is more than the store keeps"
        );
        assert!(
            count < when.len(),
            "every step became a version, which is not a timeline"
        );
        // The last step of a day is always one, so no day of work goes
        // unrepresented on the calendar.
        for (index, at) in when.iter().enumerate() {
            let last_of_day =
                !matches!(when.get(index + 1), Some(next) if next.date() == at.date());
            assert!(!last_of_day || take[index], "a day ended without a version");
        }
        let named = names(&take);
        assert_eq!(named.len(), 3, "a history with no names is half a panel");
        assert!(
            named.keys().all(|index| take[*index]),
            "a name was hung on a step nobody saved"
        );
        // Too short to navigate is too short to label.
        assert!(names(&[true, true, true]).is_empty());
    }

    #[test]
    fn what_changed_is_the_texts_that_differ() {
        let before: BTreeMap<String, String> = [("main.tex".to_string(), "one".to_string())]
            .into_iter()
            .collect();
        let now: BTreeMap<String, String> = [
            ("main.tex".to_string(), "two".to_string()),
            ("references.bib".to_string(), String::new()),
        ]
        .into_iter()
        .collect();
        assert_eq!(changed(&before, &now), vec!["main.tex", "references.bib"]);
        assert!(
            changed(&now, &now).is_empty(),
            "a version that moved nothing must say so rather than claiming a file"
        );
    }
}
