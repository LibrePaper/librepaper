//! `pull`: take the web project's named versions into a directory.
//!
//! In a git repository the versions become commits on `refs/librepaper/main`
//! and git merges them; the working tree is written by git alone. In a plain
//! folder the newest version is written into the folder, with a file changed
//! on both sides left for the person to settle.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

use crate::chain::{Chain, Meta, MAIN_REF};
use crate::client::{Client, Label, Snapshot};
use crate::files::{inside, line_ending, sha256_hex, with_ending, FileRecord, Kind};
use crate::git::Git;
use crate::rules::Rules;
use crate::state::{Fetched, State, StateStore};
use crate::walk::{scan, Local, Scan};
use crate::Error;

/// How to pull.
#[derive(Clone, Debug)]
pub struct PullOptions {
    /// In a repository, merge what was fetched. Off, the fetch stops at
    /// `refs/librepaper/main`.
    pub merge: bool,
    /// Outside a repository, files changed on both sides that stay as they
    /// are here.
    pub mine: Vec<String>,
    /// Outside a repository, files changed on both sides that take the web
    /// version.
    pub theirs: Vec<String>,
}

impl Default for PullOptions {
    fn default() -> PullOptions {
        PullOptions {
            merge: true,
            mine: Vec::new(),
            theirs: Vec::new(),
        }
    }
}

/// A web version and the commit that stands for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VersionCommit {
    /// The version's label sha; empty for the current head taken as a base.
    pub sha: String,
    pub label: String,
    pub by: String,
    pub commit: String,
}

/// What git did with `refs/librepaper/main`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MergeOutcome {
    /// The pull was asked not to merge, or has nothing to merge.
    Skipped,
    FastForward,
    Merged,
    /// The branch already held everything fetched.
    AlreadyMerged,
    /// The merge is left in progress for the person, as `git pull` leaves it.
    Conflicts(Vec<String>),
    /// Tracked project files with uncommitted changes. Nothing was fetched.
    DirtyTree(Vec<String>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PullOutcome {
    UpToDate,
    Repository {
        versions: Vec<VersionCommit>,
        merge: MergeOutcome,
        /// History before the project's trim point is gone, so the newest
        /// version was taken as a fresh base.
        trimmed: bool,
    },
    Folder {
        /// Files written from the web version.
        written: Vec<String>,
        /// Files the web deleted, moved to the trash.
        trashed: Vec<String>,
        /// Files changed only here, left as they are.
        kept: Vec<String>,
        /// Files changed on both sides and not settled by `mine` or `theirs`.
        conflicts: Vec<String>,
        /// Files the web holds that the folder also has but the scan leaves
        /// out (ignored, hidden, not UTF-8): neither written nor trashed.
        skipped: Vec<String>,
        trimmed: bool,
    },
}

pub async fn pull(
    client: &Client,
    dir: &Path,
    store: &dyn StateStore,
    options: &PullOptions,
) -> Result<PullOutcome, Error> {
    let state = store.load()?.ok_or(Error::NotLinked)?;
    let rules = client.rules().await?;
    match store.repository() {
        Some(root) => pull_repository(client, root, store, options, state, &rules).await,
        None => pull_folder(client, dir, store, options, state, &rules).await,
    }
}

// ---------------------------------------------------------------- versions

/// What to take from the project.
#[derive(Debug)]
pub(crate) struct Discovery {
    /// Oldest first.
    pub versions: Vec<Label>,
    pub trimmed: bool,
    /// There is no named version to take, so the project as it stands is the
    /// base.
    pub from_head: bool,
}

/// The versions newer than `pulled` in a list that runs newest first, oldest
/// first, and whether the list reached `pulled`.
pub(crate) fn select(newest_first: &[Label], pulled: Option<i64>) -> (Vec<Label>, bool) {
    let mut take = Vec::new();
    let mut reached = false;
    for label in newest_first {
        if pulled.is_some_and(|pulled| label.sequence <= pulled) {
            reached = true;
            break;
        }
        take.push(label.clone());
    }
    take.reverse();
    (take, reached)
}

/// Decides what to take. History is trimmed when something was pulled before
/// and nothing at or below it is left to show for it.
pub(crate) fn plan(mut take: Vec<Label>, pulled: Option<i64>, base_found: bool) -> Discovery {
    let trimmed = pulled.is_some() && !base_found;
    if trimmed {
        // What came between is gone or cannot be told apart from what is not,
        // so only the newest version is taken.
        take = take.pop().into_iter().collect();
    }
    let from_head = take.is_empty() && (pulled.is_none() || trimmed);
    Discovery {
        versions: take,
        trimmed,
        from_head,
    }
}

async fn discover(client: &Client, slug: &str, pulled: Option<i64>) -> Result<Discovery, Error> {
    let mut seen: Vec<Label> = Vec::new();
    let mut after: Option<i64> = None;
    loop {
        let (page, next) = client.labels(slug, true, 200, after).await?;
        let reached =
            pulled.is_some_and(|pulled| page.iter().any(|label| label.sequence <= pulled));
        seen.extend(page);
        match next {
            Some(cursor) if !reached => {
                if Some(cursor) == after {
                    return Err(Error::Invalid(
                        "the server repeated a page of versions".into(),
                    ));
                }
                after = Some(cursor);
            }
            _ => break,
        }
    }
    let (take, mut base_found) = select(&seen, pulled);
    if !base_found {
        if let Some(pulled) = pulled {
            // The version taken last may be one nobody named, which the named
            // list does not show.
            let (older, _) = client.labels(slug, false, 1, Some(pulled + 1)).await?;
            base_found = !older.is_empty();
        }
    }
    Ok(plan(take, pulled, base_found))
}

// ---------------------------------------------------------------- repository

async fn pull_repository(
    client: &Client,
    root: &Path,
    store: &dyn StateStore,
    options: &PullOptions,
    mut state: State,
    rules: &Rules,
) -> Result<PullOutcome, Error> {
    let git = Git::at(root);
    let settled = settle(&git, &mut state);
    let slug = state.link.slug.clone();
    let found = discover(client, &slug, state.pulled).await?;
    let nothing_new = found.versions.is_empty() && !found.from_head;
    let pending = state.fetched.is_some();
    // A trim is said even when the project holds what the chain already has,
    // so the person knows the versions between are gone.
    let unchanged = |trimmed: bool| {
        if trimmed {
            PullOutcome::Repository {
                versions: Vec::new(),
                merge: MergeOutcome::Skipped,
                trimmed: true,
            }
        } else {
            PullOutcome::UpToDate
        }
    };
    if nothing_new && !(pending && options.merge) {
        if settled {
            store.save(&state)?;
        }
        return Ok(unchanged(found.trimmed));
    }

    // A tree with uncommitted changes refuses before anything is fetched.
    if options.merge {
        let blocked = git.dirty(|path| rules.check(path).is_ok())?;
        if !blocked.is_empty() {
            if settled {
                store.save(&state)?;
            }
            return Ok(PullOutcome::Repository {
                versions: Vec::new(),
                merge: MergeOutcome::DirtyTree(blocked),
                trimmed: found.trimmed,
            });
        }
    }

    let mut versions = Vec::new();
    if !nothing_new {
        versions = fetch(client, &git, &mut state, &found, rules).await?;
        if versions.is_empty() && state.fetched.is_none() {
            // The project as it stands is what the branch already has.
            if settled {
                store.save(&state)?;
            }
            return Ok(unchanged(found.trimmed));
        }
    }
    // Saved before git merges, so a merge that stops for a person is not
    // fetched a second time.
    store.save(&state)?;

    let merge = if options.merge {
        merge_fetched(&git)?
    } else {
        MergeOutcome::Skipped
    };
    if matches!(
        merge,
        MergeOutcome::FastForward | MergeOutcome::Merged | MergeOutcome::AlreadyMerged
    ) {
        promote(&git, &mut state);
        store.save(&state)?;
    }
    Ok(PullOutcome::Repository {
        versions,
        merge,
        trimmed: found.trimmed,
    })
}

/// Chains each version onto `refs/librepaper/main` and records the newest as
/// fetched. Answers the commits it made.
async fn fetch(
    client: &Client,
    git: &Git,
    state: &mut State,
    found: &Discovery,
    rules: &Rules,
) -> Result<Vec<VersionCommit>, Error> {
    let slug = state.link.slug.clone();
    let host = state
        .link
        .site
        .host_str()
        .unwrap_or("librepaper.org")
        .to_string();
    let local = scan(git.dir(), rules, Some(git))?;
    let mut known: BTreeSet<String> = state.files.keys().cloned().collect();
    known.extend(state.synced.iter().cloned());
    if let Some(fetched) = &state.fetched {
        known.extend(fetched.files.keys().cloned());
    }
    // The chain goes on from what was fetched last; a repository that has
    // fetched nothing yet starts from the commit it was published at.
    let before = git.rev(MAIN_REF).or_else(|| {
        state
            .head
            .clone()
            .filter(|head| git.has_commit(head.as_str()))
    });
    let mut chain = Chain::new(git, &host, before.clone(), known)?;

    let mut commits = Vec::new();
    let mut taken: Vec<String> = Vec::new();
    let mut newest: Option<(Option<i64>, BTreeMap<String, FileRecord>)> = None;

    if found.from_head {
        let mut snapshot = client.head(&slug).await?;
        snapshot.retain_synced(rules);
        let assets = asset_bytes(client, &slug, &local, &snapshot, &chain).await?;
        let meta = Meta {
            by: "LibrePaper",
            at: "",
            message: "Current version",
            version: None,
            skip_if_unchanged: true,
        };
        let commit = chain.commit(&snapshot.files, &snapshot.texts, &assets, &meta, None)?;
        if Some(commit.as_str()) != before.as_deref() {
            commits.push(VersionCommit {
                sha: String::new(),
                label: meta.message.to_string(),
                by: meta.by.to_string(),
                commit,
            });
        }
        state.synced.extend(snapshot.files.keys().cloned());
        newest = Some((None, snapshot.files));
    }
    for label in &found.versions {
        let mut snapshot = client.version(&slug, &label.sha).await?;
        // A path the rules refuse never enters the chain, whatever the web holds.
        snapshot.retain_synced(rules);
        let assets = asset_bytes(client, &slug, &local, &snapshot, &chain).await?;
        let name = label.name();
        let meta = Meta {
            by: &label.by,
            at: &label.at,
            message: &name,
            version: Some(&label.sha),
            skip_if_unchanged: false,
        };
        let own = state
            .pushed
            .iter()
            .find(|pushed| pushed.label == label.sha)
            .and_then(|pushed| pushed.commit.as_deref());
        let commit = chain.commit(&snapshot.files, &snapshot.texts, &assets, &meta, own)?;
        commits.push(VersionCommit {
            sha: label.sha.clone(),
            label: name.clone(),
            by: label.by.clone(),
            commit,
        });
        taken.push(label.sha.clone());
        state.synced.extend(snapshot.files.keys().cloned());
        newest = Some((Some(label.sequence), snapshot.files));
    }

    let Some(tip) = chain.tip().map(str::to_string) else {
        return Ok(commits);
    };
    if commits.is_empty() {
        return Ok(commits);
    }
    git.run(&["update-ref", MAIN_REF, tip.as_str()], None, &[])?;
    if let Some((pulled, files)) = newest {
        state.pulled = pulled;
        state.fetched = Some(Fetched { commit: tip, files });
    }
    state.pushed.retain(|pushed| !taken.contains(&pushed.label));
    Ok(commits)
}

/// The bytes of every asset in the snapshot that git does not have yet, from
/// the working tree when it holds them and from the project otherwise.
async fn asset_bytes(
    client: &Client,
    slug: &str,
    local: &Scan,
    snapshot: &Snapshot,
    chain: &Chain<'_>,
) -> Result<HashMap<String, Vec<u8>>, Error> {
    let mut bytes = HashMap::new();
    for record in snapshot.files.values() {
        if record.kind != Kind::Asset
            || chain.has_blob(&record.sha)
            || bytes.contains_key(&record.sha)
        {
            continue;
        }
        let held = local
            .files
            .values()
            .find(|file| file.kind == Kind::Asset && file.sha == record.sha)
            .and_then(|file| std::fs::read(&file.file).ok())
            .filter(|data| sha256_hex(data) == record.sha);
        let data = match held {
            Some(data) => data,
            None => client.asset(slug, &record.sha).await?,
        };
        bytes.insert(record.sha.clone(), data);
    }
    Ok(bytes)
}

fn merge_fetched(git: &Git) -> Result<MergeOutcome, Error> {
    let before = git.rev("HEAD");
    let output = git.output(&["merge", "--no-edit", MAIN_REF], None, &[])?;
    if output.status.success() {
        let after = git.rev("HEAD");
        return Ok(if after == before {
            MergeOutcome::AlreadyMerged
        } else if after == git.rev(MAIN_REF) {
            MergeOutcome::FastForward
        } else {
            MergeOutcome::Merged
        });
    }
    let unmerged = git.unmerged()?;
    if !unmerged.is_empty() {
        return Ok(MergeOutcome::Conflicts(unmerged));
    }
    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let said = said.trim();
    if said.contains("unrelated histories") {
        return Err(Error::Git(format!(
            "this repository and the project share no history; to join them, run: git merge --allow-unrelated-histories {MAIN_REF}"
        )));
    }
    Err(Error::Git(format!("merge: {said}")))
}

/// Makes a fetched version the one the directory is based on.
fn promote(git: &Git, state: &mut State) {
    let Some(fetched) = state.fetched.take() else {
        return;
    };
    // A push made since is newer than the fetch it was merged after.
    let newer = state
        .head
        .as_deref()
        .is_none_or(|head| git.is_ancestor(head, &fetched.commit));
    if newer {
        state.head = Some(fetched.commit);
    }
    state.files = fetched.files;
}

/// Promotes a fetch that git has merged since, perhaps by hand. Answers
/// whether the state changed.
pub(crate) fn settle(git: &Git, state: &mut State) -> bool {
    let merged = state
        .fetched
        .as_ref()
        .is_some_and(|fetched| git.is_ancestor(&fetched.commit, "HEAD"));
    if merged {
        promote(git, state);
    }
    merged
}

// ---------------------------------------------------------------- folder

/// What a folder pull does with each path.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Reconciliation {
    /// Paths to write from the web version, or to trash when the web has no
    /// such file.
    pub apply: Vec<String>,
    /// Paths changed only here, or on both sides and settled as mine.
    pub kept: Vec<String>,
    pub conflicts: Vec<String>,
    /// Paths the web changed that the folder holds but the scan leaves out.
    /// Their records stay as they were.
    pub skipped: Vec<String>,
    /// The recorded files once `apply` is done.
    pub records: BTreeMap<String, FileRecord>,
}

/// Compares the recorded files, the folder and the web version, path by path.
/// `local` is the hash of each file in the folder, and `excluded` the paths
/// that are in the folder although `local` does not list them.
pub(crate) fn reconcile(
    recorded: &BTreeMap<String, FileRecord>,
    local: &BTreeMap<String, String>,
    web: &BTreeMap<String, FileRecord>,
    excluded: &BTreeSet<String>,
    mine: &[String],
    theirs: &[String],
) -> Reconciliation {
    let mut outcome = Reconciliation {
        records: recorded.clone(),
        ..Reconciliation::default()
    };
    let paths: BTreeSet<&String> = recorded
        .keys()
        .chain(local.keys())
        .chain(web.keys())
        .collect();
    for path in paths {
        let base = recorded.get(path).map(|record| record.sha.as_str());
        let here = local.get(path).map(String::as_str);
        let there = web.get(path).map(|record| record.sha.as_str());
        let adopt = |records: &mut BTreeMap<String, FileRecord>| match web.get(path) {
            Some(record) => {
                records.insert(path.clone(), record.clone());
            }
            None => {
                records.remove(path);
            }
        };
        if excluded.contains(path) && there.is_some() && there != base {
            // Absent from `local` is not absent from the folder. Writing over
            // a file nobody compared would lose it, so it stays as it is. The
            // record takes the web version, so the folder is measured against
            // it from now on and the version counts as taken in.
            adopt(&mut outcome.records);
            outcome.skipped.push(path.clone());
        } else if here == there {
            adopt(&mut outcome.records);
        } else if here == base {
            adopt(&mut outcome.records);
            outcome.apply.push(path.clone());
        } else if there == base {
            outcome.kept.push(path.clone());
        } else if theirs.contains(path) {
            adopt(&mut outcome.records);
            outcome.apply.push(path.clone());
        } else if mine.contains(path) {
            // Kept here, and from now on measured against the web version, so
            // the next push replaces it.
            adopt(&mut outcome.records);
            outcome.kept.push(path.clone());
        } else {
            outcome.conflicts.push(path.clone());
        }
    }
    outcome
}

async fn pull_folder(
    client: &Client,
    dir: &Path,
    store: &dyn StateStore,
    options: &PullOptions,
    mut state: State,
    rules: &Rules,
) -> Result<PullOutcome, Error> {
    let slug = state.link.slug.clone();
    let found = discover(client, &slug, state.pulled).await?;
    if found.versions.is_empty() && !found.from_head {
        return Ok(PullOutcome::UpToDate);
    }
    let (mut web, sequence) = match found.versions.last() {
        Some(label) => (
            client.version(&slug, &label.sha).await?,
            Some(label.sequence),
        ),
        None => (client.head(&slug).await?, None),
    };
    // The rules apply to what comes in as to what goes out: a path they refuse
    // is dropped here, so it is never written.
    web.retain_synced(rules);
    let here = scan(dir, rules, None)?;
    let excluded = here.left_out(dir, web.files.keys());
    let hashes: BTreeMap<String, String> = here
        .files
        .iter()
        .map(|(path, file)| (path.clone(), file.sha.clone()))
        .collect();
    let plan = reconcile(
        &state.files,
        &hashes,
        &web.files,
        &excluded,
        &options.mine,
        &options.theirs,
    );

    let mut written = Vec::new();
    let mut trashed = Vec::new();
    for path in &plan.apply {
        match web.files.get(path) {
            Some(record) => {
                write_remote(client, &slug, dir, path, record, &web, here.files.get(path)).await?;
                written.push(path.clone());
            }
            None => {
                // `file.file` came from the walk of `dir`, which follows no
                // link, so it is inside the folder.
                if let Some(file) = here.files.get(path) {
                    move_to_trash(&file.file)?;
                    trashed.push(path.clone());
                }
            }
        }
    }

    state.files = plan.records;
    let paths: Vec<String> = state.files.keys().cloned().collect();
    state.synced.extend(paths);
    // With a file unsettled the same version is met again, so the next call
    // can settle it.
    if plan.conflicts.is_empty() {
        state.pulled = sequence;
    }
    store.save(&state)?;
    Ok(PullOutcome::Folder {
        written,
        trashed,
        kept: plan.kept,
        conflicts: plan.conflicts,
        skipped: plan.skipped,
        trimmed: found.trimmed,
    })
}

/// Writes one file of the web version. A text file keeps the line ending it
/// has; a new one gets LF.
async fn write_remote(
    client: &Client,
    slug: &str,
    dir: &Path,
    path: &str,
    record: &FileRecord,
    web: &Snapshot,
    existing: Option<&Local>,
) -> Result<(), Error> {
    let bytes = match record.kind {
        Kind::Text => {
            let text = web
                .texts
                .get(path)
                .ok_or_else(|| Error::Invalid(format!("the version has no text for {path}")))?;
            let ending = existing
                .and_then(|file| std::fs::read(&file.file).ok())
                .map_or("\n", |current| line_ending(&current));
            with_ending(text, ending).into_bytes()
        }
        Kind::Asset => client.asset(slug, &record.sha).await?,
    };
    write_file(dir, path, &bytes)
}

/// Replaces a file in one step, so an editor watching the folder never sees
/// half of it.
fn write_file(dir: &Path, relative: &str, bytes: &[u8]) -> Result<(), Error> {
    // Before anything is created: a path that is not plain names below `dir`
    // could land anywhere, as `C:/x` does on Windows.
    let target = inside(dir, relative)?;
    let parent = target.parent().unwrap_or(dir);
    std::fs::create_dir_all(parent).map_err(|error| Error::io(parent, error))?;
    // A link anywhere on the way in could lead out of the directory.
    for ancestor in target.ancestors().skip(1) {
        if ancestor == dir || !ancestor.starts_with(dir) {
            break;
        }
        if std::fs::symlink_metadata(ancestor).is_ok_and(|meta| meta.file_type().is_symlink()) {
            return Err(Error::Invalid(format!(
                "{relative} is behind a symbolic link"
            )));
        }
    }
    let temporary = parent.join(format!(
        ".librepaper-{}.tmp",
        librepaper_base::util::random_token()
    ));
    std::fs::write(&temporary, bytes).map_err(|error| Error::io(&temporary, error))?;
    if let Ok(meta) = std::fs::metadata(&target) {
        let _ = std::fs::set_permissions(&temporary, meta.permissions());
    }
    std::fs::rename(&temporary, &target).map_err(|error| {
        let _ = std::fs::remove_file(&temporary);
        Error::io(&target, error)
    })
}

/// Moves a file to the operating system trash, which the person can undo.
#[cfg(not(test))]
fn move_to_trash(path: &Path) -> Result<(), Error> {
    trash::delete(path).map_err(|error| Error::io(path, std::io::Error::other(error.to_string())))
}

/// Tests cannot rely on a desktop trash being present.
#[cfg(test)]
fn move_to_trash(path: &Path) -> Result<(), Error> {
    std::fs::remove_file(path).map_err(|error| Error::io(path, error))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn label(sequence: i64) -> Label {
        Label {
            sha: format!("sha-{sequence}"),
            sequence,
            at: String::new(),
            by: "Bob".into(),
            label: Some(format!("Version {sequence}")),
        }
    }

    fn sequences(labels: &[Label]) -> Vec<i64> {
        labels.iter().map(|label| label.sequence).collect()
    }

    fn text(sha: &str) -> FileRecord {
        FileRecord {
            kind: Kind::Text,
            sha: sha.into(),
        }
    }

    #[test]
    fn versions_after_the_last_pull_come_oldest_first() {
        let list = [label(9), label(7), label(5), label(3)];
        let (take, reached) = select(&list, Some(5));
        assert_eq!(sequences(&take), [7, 9]);
        assert!(reached);
        let (take, reached) = select(&list, Some(9));
        assert!(take.is_empty());
        assert!(reached);
        let (take, reached) = select(&list, None);
        assert_eq!(sequences(&take), [3, 5, 7, 9]);
        assert!(!reached, "a first pull has no base to reach");
    }

    #[test]
    fn a_list_that_never_reaches_the_last_pull_means_the_history_was_trimmed() {
        // Pulled at 5, but the project now lists nothing at or below 5.
        let list = [label(20), label(12)];
        let (take, reached) = select(&list, Some(5));
        assert!(!reached);
        let found = plan(take, Some(5), reached);
        assert!(found.trimmed);
        assert_eq!(sequences(&found.versions), [20], "only the newest is taken");
        assert!(!found.from_head);
    }

    #[test]
    fn an_unnamed_base_label_is_not_a_trim() {
        let list = [label(20), label(12)];
        let (take, reached) = select(&list, Some(5));
        assert!(!reached);
        // The base label exists, but nobody named it, so the named list does
        // not show it.
        let found = plan(take, Some(5), true);
        assert!(!found.trimmed);
        assert_eq!(sequences(&found.versions), [12, 20]);
    }

    #[test]
    fn a_trim_that_left_no_version_takes_the_project_as_it_stands() {
        let found = plan(Vec::new(), Some(5), false);
        assert!(found.trimmed);
        assert!(found.versions.is_empty());
        assert!(found.from_head);
    }

    #[test]
    fn nothing_new_is_up_to_date_except_on_a_first_pull() {
        let found = plan(Vec::new(), Some(5), true);
        assert!(!found.trimmed && !found.from_head && found.versions.is_empty());
        let first = plan(Vec::new(), None, false);
        assert!(!first.trimmed);
        assert!(first.from_head);
        let first = plan(vec![label(1), label(2)], None, false);
        assert_eq!(sequences(&first.versions), [1, 2]);
        assert!(!first.from_head);
    }

    fn files(entries: &[(&str, &str)]) -> BTreeMap<String, FileRecord> {
        entries
            .iter()
            .map(|(path, sha)| (path.to_string(), text(sha)))
            .collect()
    }

    fn none() -> BTreeSet<String> {
        BTreeSet::new()
    }

    fn hashes(entries: &[(&str, &str)]) -> BTreeMap<String, String> {
        entries
            .iter()
            .map(|(path, sha)| (path.to_string(), sha.to_string()))
            .collect()
    }

    #[test]
    fn a_folder_takes_web_changes_and_keeps_its_own() {
        let recorded = files(&[
            ("web.tex", "1"),
            ("here.tex", "1"),
            ("same.tex", "1"),
            ("gone.tex", "1"),
        ]);
        let local = hashes(&[
            ("web.tex", "1"),
            ("here.tex", "2"),
            ("same.tex", "1"),
            ("gone.tex", "1"),
        ]);
        let web = files(&[
            ("web.tex", "2"),
            ("here.tex", "1"),
            ("same.tex", "1"),
            ("new.tex", "1"),
        ]);
        let plan = reconcile(&recorded, &local, &web, &none(), &[], &[]);
        assert_eq!(plan.apply, ["gone.tex", "new.tex", "web.tex"]);
        assert_eq!(plan.kept, ["here.tex"]);
        assert!(plan.conflicts.is_empty());
        assert_eq!(plan.records["web.tex"].sha, "2");
        assert_eq!(
            plan.records["here.tex"].sha, "1",
            "still measured against the base"
        );
        assert!(!plan.records.contains_key("gone.tex"));
        assert_eq!(plan.records["new.tex"].sha, "1");
    }

    #[test]
    fn a_file_changed_on_both_sides_waits_for_an_answer() {
        let recorded = files(&[("a.tex", "1"), ("b.tex", "1"), ("c.tex", "1")]);
        let local = hashes(&[("a.tex", "2"), ("b.tex", "2"), ("c.tex", "2")]);
        let web = files(&[("a.tex", "3"), ("b.tex", "3"), ("c.tex", "3")]);

        let open = reconcile(&recorded, &local, &web, &none(), &[], &[]);
        assert_eq!(open.conflicts, ["a.tex", "b.tex", "c.tex"]);
        assert!(open.apply.is_empty());
        assert_eq!(open.records, recorded, "nothing is confirmed");

        let settled = reconcile(
            &recorded,
            &local,
            &web,
            &none(),
            &["b.tex".to_string()],
            &["a.tex".to_string()],
        );
        assert_eq!(settled.conflicts, ["c.tex"]);
        assert_eq!(settled.apply, ["a.tex"]);
        assert_eq!(settled.kept, ["b.tex"]);
        assert_eq!(settled.records["a.tex"].sha, "3");
        assert_eq!(
            settled.records["b.tex"].sha, "3",
            "the next push replaces the web version"
        );
        assert_eq!(settled.records["c.tex"].sha, "1");
    }

    #[test]
    fn deletions_follow_the_same_rule() {
        let recorded = files(&[("web.tex", "1"), ("here.tex", "1"), ("both.tex", "1")]);
        // Deleted on the web only, here only, and here with an edit on the web.
        let local = hashes(&[("web.tex", "1"), ("both.tex", "2")]);
        let web = files(&[("here.tex", "1")]);
        let plan = reconcile(&recorded, &local, &web, &none(), &[], &[]);
        assert_eq!(plan.apply, ["web.tex"]);
        assert_eq!(plan.kept, ["here.tex"]);
        assert_eq!(plan.conflicts, ["both.tex"]);
        assert!(!plan.records.contains_key("web.tex"));

        // Mine keeps the edited file and drops the record, so a push creates it.
        let plan = reconcile(
            &recorded,
            &local,
            &web,
            &none(),
            &["both.tex".to_string()],
            &[],
        );
        assert!(plan.conflicts.is_empty());
        assert!(!plan.records.contains_key("both.tex"));
        assert!(plan.kept.contains(&"both.tex".to_string()));
    }

    #[test]
    fn a_first_pull_into_a_folder_with_the_same_files_confirms_them() {
        let local = hashes(&[("a.tex", "1"), ("b.tex", "2")]);
        let web = files(&[("a.tex", "1"), ("b.tex", "3"), ("c.tex", "1")]);
        let plan = reconcile(&BTreeMap::new(), &local, &web, &none(), &[], &[]);
        assert_eq!(plan.conflicts, ["b.tex"]);
        assert_eq!(plan.apply, ["c.tex"]);
        assert_eq!(plan.records["a.tex"].sha, "1");
    }

    #[test]
    fn an_excluded_path_takes_the_web_record_and_is_never_written() {
        let recorded = files(&[("changed.tex", "1"), ("same.tex", "1"), ("gone.tex", "1")]);
        let web = files(&[("changed.tex", "2"), ("same.tex", "1"), ("fresh.tex", "1")]);
        let excluded: BTreeSet<String> =
            BTreeSet::from(["changed.tex", "same.tex", "gone.tex", "fresh.tex"].map(String::from));
        let plan = reconcile(&recorded, &hashes(&[]), &web, &excluded, &[], &[]);
        assert_eq!(plan.skipped, ["changed.tex", "fresh.tex"]);
        assert!(plan.apply.is_empty());
        assert!(plan.conflicts.is_empty());
        assert_eq!(
            plan.kept,
            ["same.tex"],
            "the web did not change it, so nothing is at stake"
        );
        assert_eq!(
            plan.records["changed.tex"].sha, "2",
            "measured against the web from now on, though the file was not written"
        );
        assert_eq!(plan.records["fresh.tex"].sha, "1", "adopted, not written");
        assert!(!plan.records.contains_key("gone.tex"), "the web dropped it");
    }

    #[test]
    fn a_folder_file_the_scan_skips_is_not_written_over_by_the_web() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join(".gitignore"), "build/\n").unwrap();
        std::fs::create_dir_all(root.join("build")).unwrap();
        std::fs::write(root.join("build/out.tex"), "mine\n").unwrap();
        std::fs::write(root.join("latin.tex"), [b'c', b'a', b'f', 0xe9]).unwrap();
        std::fs::write(root.join("main.tex"), "same\n").unwrap();

        let same = sha256_hex(b"same\n");
        let web = files(&[
            ("build/out.tex", "1"),
            ("latin.tex", "1"),
            ("main.tex", same.as_str()),
            ("new.tex", "1"),
        ]);
        let here = scan(root, &Rules::default(), None).unwrap();
        let excluded = here.left_out(root, web.keys());
        assert_eq!(
            excluded.iter().map(String::as_str).collect::<Vec<_>>(),
            ["build/out.tex", "latin.tex"]
        );
        let local: BTreeMap<String, String> = here
            .files
            .iter()
            .map(|(path, file)| (path.clone(), file.sha.clone()))
            .collect();

        let plan = reconcile(&BTreeMap::new(), &local, &web, &excluded, &[], &[]);
        assert_eq!(plan.apply, ["new.tex"]);
        assert_eq!(plan.skipped, ["build/out.tex", "latin.tex"]);
        assert!(plan.conflicts.is_empty());
        assert_eq!(plan.records["build/out.tex"].sha, web["build/out.tex"].sha);
        assert_eq!(plan.records["latin.tex"].sha, web["latin.tex"].sha);
        assert_eq!(plan.records["main.tex"].sha, same);

        // Taking the web version for the path does not write it either.
        let theirs = ["build/out.tex".to_string()];
        let plan = reconcile(&BTreeMap::new(), &local, &web, &excluded, &[], &theirs);
        assert_eq!(plan.apply, ["new.tex"]);
        assert_eq!(plan.skipped, ["build/out.tex", "latin.tex"]);
    }

    #[test]
    fn a_path_that_is_not_plain_names_is_refused_before_anything_is_made() {
        let outer = tempfile::tempdir().unwrap();
        let dir = outer.path().join("folder");
        std::fs::create_dir(&dir).unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        let absolute = elsewhere.path().join("abs.tex");
        for bad in [
            "../escape.tex",
            "chapters/../../escape.tex",
            "./one.tex",
            absolute.to_str().unwrap(),
        ] {
            let error = write_file(&dir, bad, b"x").unwrap_err().to_string();
            assert!(error.ends_with("leaves the folder"), "{bad}: {error}");
        }
        assert!(!outer.path().join("escape.tex").exists());
        assert!(!absolute.exists());
        assert_eq!(
            std::fs::read_dir(&dir).unwrap().count(),
            0,
            "nothing was created"
        );

        write_file(&dir, "chapters/part/one.tex", b"x").unwrap();
        assert_eq!(
            std::fs::read(dir.join("chapters/part/one.tex")).unwrap(),
            b"x"
        );
    }

    #[cfg(windows)]
    #[test]
    fn a_drive_letter_path_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        for bad in ["C:/outside/paper.tex", "C:paper.tex"] {
            let error = write_file(dir.path(), bad, b"x").unwrap_err().to_string();
            assert!(error.ends_with("leaves the folder"), "{bad}: {error}");
        }
    }

    #[test]
    fn written_files_keep_their_line_endings_and_replace_in_one_step() {
        let dir = tempfile::tempdir().unwrap();
        write_file(dir.path(), "chapters/one.tex", b"first\n").unwrap();
        assert_eq!(
            std::fs::read(dir.path().join("chapters/one.tex")).unwrap(),
            b"first\n"
        );
        write_file(dir.path(), "chapters/one.tex", b"second\r\n").unwrap();
        assert_eq!(
            std::fs::read(dir.path().join("chapters/one.tex")).unwrap(),
            b"second\r\n"
        );
        let names: Vec<_> = std::fs::read_dir(dir.path().join("chapters"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(names.len(), 1, "no temporary file is left behind");
    }

    #[cfg(unix)]
    #[test]
    fn a_symbolic_link_on_the_way_in_is_not_followed() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), dir.path().join("linked")).unwrap();
        assert!(write_file(dir.path(), "linked/escape.tex", b"x").is_err());
        assert!(!outside.path().join("escape.tex").exists());
    }
}
