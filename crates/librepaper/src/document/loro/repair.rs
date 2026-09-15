//! Repair of malformed documents, using Loro.
//!
//! A document can arrive with paths the rules refuse, duplicate paths, assets
//! the rules refuse, files with no path, or a main pointer that does not exist.
//! Each case is caught and fixed in one transaction, so the document is either
//! wholly right or wholly as it was.
//!
//! A restore applies a checkpoint, preserving file identity and blame by
//! keeping texts that exist and applying word-level diffs rather than
//! wholesale replacement.

use std::collections::{BTreeMap, HashMap, HashSet};

use loro::LoroDoc;

use crate::document::paths::{self, Rules};

/// What the repair did, so the room can say it and the tests can see it.
#[derive(Clone, Debug, PartialEq)]
pub enum Repair {
    /// A path the rules refuse, renamed to something a person can see.
    Renamed { id: String, to: String },
    /// Two files at one path; the second one seen was moved aside.
    Collided { id: String, to: String },
    /// A path a person can already see, rewritten by trimming or Unicode
    /// normalisation alone -- no rename and no collision, just the same name
    /// spelled the way this document stores names. Reported on its own,
    /// because the room only relays a repair when it has something to say,
    /// and a peer whose path silently changed underneath it would otherwise
    /// diverge from the copy the server now holds.
    Normalised { id: String, to: String },
    /// An asset key the rules refuse. There is nothing to rename it to -- the
    /// key *is* the name -- so the entry goes.
    DroppedAsset { path: String },
    /// `meta.main` named nothing, or named a file that is not there.
    Remained { id: String },
}

/// Puts right whatever a peer wrote that the document cannot hold, in one
/// transaction, after the update has been applied and before it is relayed.
///
/// This is the counterpart of the size ceiling, and it is deliberately not the
/// same kind of answer. A peer that writes past the size ceiling is closed,
/// because bytes are the bill; a peer that writes a path the rules refuse is
/// corrected, because a bad name is a mistake a person can see and fix and
/// closing their socket would lose the rest of what they typed. Every key in
/// the shared document is a string any editor can set, so this is the place
/// that says what those strings may be.
pub fn repair(doc: &LoroDoc, rules: &Rules) -> Vec<Repair> {
    let mut done = Vec::new();

    // Get current state: paths and assets
    let here = super::shape::paths_of(doc);
    let assets = super::shape::assets_of(doc);
    let files = super::shape::texts_of(doc);

    // Paths, in a fixed order, so that two servers repairing the same document
    // make the same corrections: which of two colliding files is the one moved
    // aside cannot depend on the order a hash map happened to iterate in.
    let mut named: Vec<(String, String)> = here.into_iter().collect();
    named.sort();

    let mut taken: HashSet<String> = HashSet::new();
    for (id, path) in named {
        // Whether this id has already had a `Renamed` or `Collided` pushed
        // for it below -- the two repairs that already say a path changed.
        // A `Normalised` at the end is only for a path that changed and had
        // neither said so yet.
        let mut reported = false;
        let mut wanted = match paths::check(rules, &path) {
            Ok(paths::Kind::Text) => paths::normalise(&path),
            // A text at an asset's name, or at no known name at all: the file
            // holds words, whatever it is called, so it is renamed rather than
            // dropped.
            _ => {
                let placeholder = paths::placeholder(&id);
                done.push(Repair::Renamed {
                    id: id.clone(),
                    to: placeholder.clone(),
                });
                reported = true;
                placeholder
            }
        };
        if taken.contains(&paths::collision_key(&wanted)) {
            let mut base = wanted.clone();
            let mut nth = 2;
            let mut fell_back = false;
            let mut candidate = paths::suffixed(&base, nth);
            loop {
                // `suffixed` only ever makes a name longer, so a base already
                // close to the rules' length ceiling can be pushed over it by
                // its own suffix. Inserting that candidate unchecked would
                // write a path the rules refuse right back into the document
                // the rules are supposed to hold -- the next repair pass
                // would then see the same refused path forever. The
                // placeholder is short enough that, suffixed the same way, it
                // fits under any ceiling a deployment would sensibly set; it
                // is tried once, so a ceiling shorter even than that cannot
                // turn this into a loop that never ends.
                if !fell_back && !matches!(paths::check(rules, &candidate), Ok(paths::Kind::Text)) {
                    base = paths::placeholder(&id);
                    nth = 2;
                    fell_back = true;
                    candidate = paths::suffixed(&base, nth);
                    continue;
                }
                if !taken.contains(&paths::collision_key(&candidate)) {
                    break;
                }
                nth += 1;
                candidate = paths::suffixed(&base, nth);
            }
            wanted = candidate;
            done.push(Repair::Collided {
                id: id.clone(),
                to: wanted.clone(),
            });
            reported = true;
        }
        taken.insert(paths::collision_key(&wanted));
        if wanted != path {
            super::edits::rename_path(doc, &path, &wanted);
            if !reported {
                done.push(Repair::Normalised {
                    id: id.clone(),
                    to: wanted,
                });
            }
        }
    }

    // A text with no path at all -- a `files` entry somebody set without one --
    // is named, so that it is a file rather than an orphan.
    let ids: Vec<String> = files.keys().cloned().collect();
    for id in &ids {
        if super::shape::paths_of(doc).contains_key(id) {
            continue;
        }
        let mut placeholder = paths::placeholder(id);
        let mut nth = 2;
        while taken.contains(&paths::collision_key(&placeholder)) {
            placeholder = paths::suffixed(&paths::placeholder(id), nth);
            nth += 1;
        }
        taken.insert(paths::collision_key(&placeholder));
        // An orphaned file (in files map but not in paths map) is given a path.
        // This assumes the edits module can assign a path to an existing id
        // without creating a duplicate file or losing its content.
        super::edits::put_text(doc, &placeholder, &files[&id]);
        done.push(Repair::Renamed {
            id: id.clone(),
            to: placeholder,
        });
    }

    // An asset is named by its key, so a key the rules refuse has nothing to
    // be renamed to: the bytes are still in the store, and setting the key
    // again with a name that passes is what puts them back.
    // Sorted for the same reason `named` is above: two assets that collide
    // only once case is folded (`Fig.png` and `fig.png`) must move the same
    // one of the two aside on every server, and a hash map's iteration order
    // is not that.
    let mut asset_paths: Vec<String> = assets.keys().cloned().collect();
    asset_paths.sort();
    for path in asset_paths {
        let refused = !matches!(paths::check(rules, &path), Ok(paths::Kind::Asset));
        if refused || taken.contains(&paths::collision_key(&path)) {
            super::edits::remove_asset(doc, &path);
            done.push(Repair::DroppedAsset { path });
            continue;
        }
        taken.insert(paths::collision_key(&path));
    }

    // The main file. A document whose `meta.main` names nothing takes the
    // file whose path sorts first, which is a choice somebody can change and
    // never a document that cannot be rendered.
    let names_a_file = {
        let paths = super::shape::paths_of(doc);
        let main_id = super::shape::main_id(doc);
        !main_id.is_empty() && paths.contains_key(&main_id)
    };
    if !names_a_file {
        let mut candidates: Vec<(String, String)> = super::shape::paths_of(doc)
            .into_iter()
            .map(|(id, path)| (path, id))
            .collect();
        candidates.sort();
        if let Some((_, id)) = candidates.first() {
            super::shape::set_main(doc, id);
            done.push(Repair::Remained { id: id.clone() });
        }
    }

    done
}

/// Sets the document to a tree that was checkpointed, in one transaction: a
/// restore is one moment, and a chapter and the file that includes it can
/// never come back out of step.
///
/// A file present on both sides keeps its id and takes word-level edits rather
/// than being deleted and made again, so an editor watching a restore sees the
/// words change under their caret instead of their file disappearing and a new
/// one arriving in its place.
pub fn restore(
    doc: &LoroDoc,
    tree: &crate::document::history::Tree,
    bodies: &HashMap<String, String>,
) {
    restore_with(doc, tree, |_, entry| {
        bodies.get(&entry.sha).cloned().unwrap_or_default()
    });
}

fn restore_with(
    doc: &LoroDoc,
    tree: &crate::document::history::Tree,
    body_for: impl FnMut(&str, &crate::document::history::TreeEntry) -> String,
) {
    let here = super::shape::paths_of(doc);
    let mut by_path: HashMap<String, String> = HashMap::new();
    let mut by_id: HashSet<String> = HashSet::new();
    let mut path_by_id: HashMap<String, String> = HashMap::new();
    for (id, path) in &here {
        by_path.insert(path.clone(), id.clone());
        by_id.insert(id.clone());
        path_by_id.insert(id.clone(), path.clone());
    }

    // Two different things to not do twice, kept apart on purpose. A
    // checkpoint's LoroText -- the physical thing edited or created below --
    // must not be touched a second time for a second path that resolves to
    // it; a checkpoint *entry*'s id must not be reused for a second entry
    // that names it, which is what catches a malformed or legacy tree with
    // one id under two paths. They used to share one set, which is sound
    // only when the id a path resolves to and the id the entry itself
    // carries are the same id. A path swap breaks exactly that: restoring
    // `a.md` can resolve, by live path, to the LoroText a swap put there for
    // `b.md`'s id, so what gets kept and what an entry carries are two
    // different ids for the rest of this pass. Sharing one set then made the
    // second entry's own id look already "kept" -- by the first entry's
    // unrelated LoroText -- and skip, so only one of the two swapped files
    // survived the cleanup below.
    let mut kept_texts: HashSet<String> = HashSet::new();
    let mut seen_entries: HashSet<String> = HashSet::new();
    let mut main = String::new();
    let mut body_for = body_for;

    for (path, entry) in &tree.files {
        if entry.kind == "asset" {
            super::edits::put_asset(doc, path, &entry.sha);
            continue;
        }
        // A concurrent rename can leave an effective tree with both the
        // checkpoint's old path and the live path for one LoroText id. Keep the
        // live path when it is present; assigning the id to both paths would
        // make the path map lose one of them nondeterministically.
        //
        // "Present" has to mean this id's own entry sits at the live path in
        // the checkpoint being restored, not merely that some entry does.
        // Two files can trade paths between the checkpoint and now -- `a.md`
        // and `b.md` swapped -- and then each one's live path is a key the
        // tree happens to have, but for the *other* file. Checking only
        // `contains_key` treated that as "this id's live path is covered
        // too" for both of them at once, so neither ever fell through to be
        // kept below and the cleanup pass at the end deleted both.
        if let Some(live_path) = path_by_id.get(&entry.id) {
            if live_path != path
                && tree
                    .files
                    .get(live_path)
                    .is_some_and(|other| other.id == entry.id)
            {
                if *path == tree.main {
                    if let Some(id) = by_path.get(live_path) {
                        main = id.clone();
                    }
                }
                continue;
            }
        }
        if by_path.get(path).is_some_and(|id| kept_texts.contains(id))
            || (!entry.id.is_empty() && seen_entries.contains(&entry.id))
        {
            continue;
        }
        if !entry.id.is_empty() {
            seen_entries.insert(entry.id.clone());
        }
        let body = body_for(path, entry);
        let id = if let Some(id) = by_path.get(path).cloned() {
            // File exists at this path. Apply word-level edits to preserve blame.
            apply_text_edits(doc, &id, &body);
            id
        } else if !entry.id.is_empty() && by_id.contains(&entry.id) {
            // A file may have been renamed since this checkpoint. Reuse the
            // existing LoroText by its recorded id, then move its path. Replacing
            // the map value with a new LoroText at the same key would sever
            // the identity that concurrent peers and their carets still hold.
            let id = entry.id.clone();
            apply_text_edits(doc, &id, &body);
            super::edits::rename_path(doc, &path_by_id[&id], path);
            id
        } else {
            // The id the tree recorded, so that restoring twice does not
            // make two files. A missing id is from a malformed/legacy tree,
            // and gets a fresh one rather than colliding with a live file.
            let id = if entry.id.is_empty() {
                super::shape::mint_id()
            } else {
                entry.id.clone()
            };
            super::edits::put_text(doc, path, &body);
            id
        };
        if *path == tree.main {
            main = id.clone();
        }
        kept_texts.insert(id);
    }

    // What the tree does not have is not in the document any more. A restore
    // is the tree, whole.
    for (id, _) in here {
        if !kept_texts.contains(&id) {
            super::edits::remove_path(doc, &path_by_id.get(&id).cloned().unwrap_or_default());
        }
    }

    let assets = super::shape::assets_of(doc);
    let stale: Vec<String> = assets
        .into_iter()
        .filter_map(|(path, _)| {
            if !matches!(tree.files.get(&path), Some(entry) if entry.kind == "asset") {
                Some(path)
            } else {
                None
            }
        })
        .collect();
    for path in stale {
        super::edits::remove_asset(doc, &path);
    }

    if !main.is_empty() {
        super::shape::set_main(doc, &main);
    }
}

/// Apply word-level edits to preserve blame when restoring a file.
/// The text is identified by its id. This uses character boundaries and
/// UTF-16 offsets to avoid splitting surrogate pairs, preserving concurrent
/// edits and attribution better than wholesale replacement.
fn apply_text_edits(doc: &LoroDoc, id: &str, wanted: &str) {
    // Look up the path for this id to get the current text
    let paths = super::shape::paths_of(doc);
    let Some(path) = paths.get(id) else {
        return;
    };

    let texts = super::shape::texts_of(doc);
    let Some(current) = texts.get(path) else {
        return;
    };

    if current == wanted {
        return;
    }

    // Compute the diff between current and wanted text, preserving blame
    // by only changing what is different, using character boundaries.
    let current_chars: Vec<char> = current.chars().collect();
    let wanted_chars: Vec<char> = wanted.chars().collect();

    // Find common prefix
    let mut head = 0;
    while head < current_chars.len()
        && head < wanted_chars.len()
        && current_chars[head] == wanted_chars[head]
    {
        head += 1;
    }

    // Find common suffix
    let mut tail = 0;
    while tail < current_chars.len() - head
        && tail < wanted_chars.len() - head
        && current_chars[current_chars.len() - 1 - tail] == wanted_chars[wanted_chars.len() - 1 - tail]
    {
        tail += 1;
    }

    // Translate the retained prefix/suffix, counted in characters, into the
    // UTF-16 offsets Loro counts in. Each is a sum of `char::len_utf16` rather
    // than a slice of the code-unit vector, so a boundary chosen above can
    // never land inside a surrogate pair.
    let head_units: usize = current_chars[..head].iter().map(|c| c.len_utf16()).sum();
    let tail_units: usize = current_chars[current_chars.len() - tail..]
        .iter()
        .map(|c| c.len_utf16())
        .sum();
    let current_units: usize = current_chars.iter().map(|c| c.len_utf16()).sum();
    let removed = current_units - head_units - tail_units;
    let inserted: String = wanted_chars[head..wanted_chars.len() - tail]
        .iter()
        .collect();

    if removed > 0 || !inserted.is_empty() {
        super::edits::replace_text(doc, path, head_units, removed, &inserted);
    }
}
