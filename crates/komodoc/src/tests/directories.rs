//! A document is a directory.
//!
//! Nothing a reader can see changes in this step: every document is a
//! directory of one file, and every document that existed before becomes one.
//! What is checked here is therefore mostly what must *not* have changed --
//! the migration is lossless, the old checkpoints still read, the ceiling
//! still refuses the same updates -- plus the two things that are new: the
//! rules a path has to pass, and the tree a checkpoint now records.

use std::collections::HashMap;

use yrs::{GetString, Map, Text, Transact};

use crate::config::Configuration;
use crate::history::{Checkpoint, Tree};
use crate::paths;
use crate::session::{self, Admission};

/// A document with one file in it, the way a publish leaves one.
fn one_file(path: &str, body: &str) -> yrs::Doc {
    let doc = session::new_doc();
    let id = session::put_text(&doc, path, body);
    session::set_main(&doc, &id);
    doc
}

/// What the retired `source` text still holds. Named before the transaction
/// is taken: asking a document for a type it may not have needs a write
/// transaction, and taking one while a read transaction is open is a deadlock.
fn retired(doc: &yrs::Doc) -> String {
    let source = doc.get_or_insert_text(session::SOURCE);
    let txn = doc.transact();
    source.get_string(&txn)
}

/// A session written the old way: one `source` text and nothing else. What
/// every document in storage looks like before this step runs.
fn old_session(body: &str) -> yrs::Doc {
    let doc = session::new_doc();
    let text = doc.get_or_insert_text(session::SOURCE);
    let mut txn = doc.transact_mut();
    text.insert(&mut txn, 0, body);
    drop(txn);
    doc
}

/* ------------------------------------------------------------- the migration */

#[test]
fn a_document_that_was_one_text_becomes_a_directory_of_one_file() {
    let doc = old_session("= Title\n\nA paragraph.\n");
    assert!(session::migrate(&doc, "main.typ"));

    // The words are where they were, under a name.
    assert_eq!(session::text_of(&doc), "= Title\n\nA paragraph.\n");
    assert_eq!(session::main_path(&doc), "main.typ");
    let texts = session::texts_of(&doc);
    assert_eq!(texts.len(), 1);
    assert_eq!(texts["main.typ"], "= Title\n\nA paragraph.\n");

    // And the text it came out of is empty, so nothing is stored twice and
    // nothing measures it twice.
    assert_eq!(retired(&doc), "");
}

#[test]
fn migrating_twice_is_migrating_once() {
    let doc = old_session("one file\n");
    assert!(session::migrate(&doc, "main.md"));
    let after = session::texts_of(&doc);
    let id = session::main_id(&doc);

    // The second call has nothing to do, and says so: a session loaded twice
    // must not grow a second copy of itself.
    assert!(!session::migrate(&doc, "main.md"));
    assert_eq!(session::texts_of(&doc), after);
    assert_eq!(session::main_id(&doc), id, "the file was made again");
}

#[test]
fn a_migrated_session_still_reads_as_the_old_shape_would_leave_it() {
    // The rollback caveat: a deployment that goes back to the old code reads
    // `sessions/<slug>` with a `source` text in it. After the migration that
    // text is empty -- the words are in the directory -- so what an old server
    // would show is an empty document rather than a broken one. That is the
    // bargain the migration makes, and it is worth stating as a test rather
    // than as a sentence in a file nobody opens: the state stays *readable*,
    // which is what the caveat in TODO.md promises, and not more than that.
    let doc = old_session("words\n");
    session::migrate(&doc, "main.md");
    let state = session::encode_state(&doc);

    let old = session::new_doc();
    session::apply_update(&old, &state).expect("an old server still applies it");
    assert_eq!(
        retired(&old),
        "",
        "the retired text should be empty, not unreadable"
    );
}

#[test]
fn an_empty_old_session_is_left_alone() {
    // A document created and never typed into has nothing to move, and making
    // it a file would write a state for a session that has none.
    let doc = session::new_doc();
    assert!(!session::migrate(&doc, "main.typ"));
    assert!(session::texts_of(&doc).is_empty());
}

/* ------------------------------------------------------------------ the rules */

fn rules() -> Configuration {
    Configuration::default()
}

#[test]
fn a_path_has_to_be_a_path() {
    let config = rules();
    let rules = config.paths();
    let refused = [
        "/main.typ",
        "../main.typ",
        "chapters/../main.typ",
        "chapters//03.tex",
        ".hidden.typ",
        "fig/.DS_Store",
        "a/b/c/d/e/f/g/h/i.typ",
        "main.aux",
        "main.log",
        "paper.synctex.gz",
        "notes.rtf",
        "main.typ\u{7}",
        "",
    ];
    for path in refused {
        assert!(
            paths::check(&rules, path).is_err(),
            "{path:?} should have been refused"
        );
    }
    assert_eq!(paths::check(&rules, "main.typ"), Ok(paths::Kind::Text));
    assert_eq!(
        paths::check(&rules, "chapters/03.tex"),
        Ok(paths::Kind::Text)
    );
    assert_eq!(paths::check(&rules, "refs.bib"), Ok(paths::Kind::Text));
    assert_eq!(paths::check(&rules, "fig/one.png"), Ok(paths::Kind::Asset));
    assert_eq!(paths::check(&rules, "fig/plot.pdf"), Ok(paths::Kind::Asset));

    // A path at the length limit passes and one past it does not, so the
    // boundary is the boundary rather than approximately it.
    let long = format!("{}.typ", "n".repeat(config.max_path - 4));
    assert_eq!(long.len(), config.max_path);
    assert!(paths::check(&rules, &long).is_ok());
    assert!(paths::check(&rules, &format!("x{long}")).is_err());
}

#[test]
fn a_refused_path_is_renamed_rather_than_dropped() {
    // A peer can set any string as a path. What it wrote is put right, and the
    // file keeps its words: closing the socket would lose the rest of what
    // that person typed, and the name is a mistake they can see and fix.
    let doc = session::new_doc();
    let id = session::put_text(&doc, "../escape.typ", "words worth keeping\n");
    session::set_main(&doc, &id);
    let config = rules();

    let done = session::repair(&doc, &config.paths());
    assert_eq!(
        done,
        vec![session::Repair::Renamed {
            id: id.clone(),
            to: paths::placeholder(&id),
        }]
    );
    let texts = session::texts_of(&doc);
    assert_eq!(texts[&paths::placeholder(&id)], "words worth keeping\n");
}

#[test]
fn two_files_at_one_path_both_survive_and_one_is_moved_aside() {
    // Two people create `notes.md` at the same instant. Neither loses their
    // file: a CRDT has no way to refuse the second, so the repair names it
    // rather than dropping it.
    let doc = session::new_doc();
    let first = session::put_text(&doc, "notes.md", "mine\n");
    let second = {
        let (files, path_map) = (
            doc.get_or_insert_map("files"),
            doc.get_or_insert_map("paths"),
        );
        let id = "zzzzzzzzzzzz".to_string();
        let mut txn = doc.transact_mut();
        files.insert(
            &mut txn,
            id.clone(),
            yrs::types::text::TextPrelim::new("theirs\n"),
        );
        path_map.insert(&mut txn, id.clone(), "notes.md".to_string());
        id
    };
    session::set_main(&doc, &first);

    let done = session::repair(&doc, &rules().paths());
    assert_eq!(
        done,
        vec![session::Repair::Collided {
            id: second,
            to: "notes (2).md".to_string(),
        }],
        "the file seen second should be the one moved aside"
    );
    let texts = session::texts_of(&doc);
    assert_eq!(texts["notes.md"], "mine\n");
    assert_eq!(texts["notes (2).md"], "theirs\n");
    assert_eq!(session::main_id(&doc), first, "the main file did not move");
}

#[test]
fn two_paths_that_a_disk_cannot_tell_apart_are_moved_apart() {
    // `Fig.png` and `fig.png` are one file on macOS and on Windows. Refusing
    // the collision here is what stops a sync client writing one over the
    // other on a laptop nobody is watching.
    let doc = session::new_doc();
    session::put_text(&doc, "Notes.md", "upper\n");
    session::put_text(&doc, "notes.md", "lower\n");
    session::repair(&doc, &rules().paths());

    let texts = session::texts_of(&doc);
    assert_eq!(texts.len(), 2);
    let names: Vec<&String> = texts.keys().collect();
    let folded: std::collections::HashSet<String> = names
        .iter()
        .map(|name| paths::collision_key(name))
        .collect();
    assert_eq!(folded.len(), 2, "two files still share one name: {names:?}");
}

#[test]
fn an_asset_at_a_name_the_rules_refuse_is_dropped() {
    // An asset is named by its key, so there is nothing to rename it to. The
    // bytes stay in the store; setting the key again with a name that passes
    // is what puts them back.
    let doc = session::new_doc();
    session::put_asset(&doc, "fig/../secret.png", "abc123");
    session::put_asset(&doc, "fig/one.png", "def456");

    let done = session::repair(&doc, &rules().paths());
    assert_eq!(
        done,
        vec![session::Repair::DroppedAsset {
            path: "fig/../secret.png".to_string()
        }]
    );
    assert_eq!(session::assets_of(&doc).len(), 1);
}

#[test]
fn a_document_whose_main_file_is_missing_takes_the_first_one() {
    // `meta.main` is a string a peer can set to anything, including the id of
    // a file it then deleted. A document that cannot say which file it is is
    // a document that cannot be rendered, so one is chosen -- the path that
    // sorts first, which is a decision somebody can change.
    let doc = session::new_doc();
    session::put_text(&doc, "zeta.typ", "last\n");
    session::put_text(&doc, "alpha.typ", "first\n");
    session::set_main(&doc, "a-file-that-is-not-here");

    let done = session::repair(&doc, &rules().paths());
    assert!(matches!(
        done.as_slice(),
        [session::Repair::Remained { .. }]
    ));
    assert_eq!(session::main_path(&doc), "alpha.typ");
}

#[test]
fn what_the_old_bundle_writes_is_folded_in_rather_than_lost() {
    // During a deploy a tab loaded before it is still writing to `source`.
    // What it wrote is the document as that tab understands it, so it becomes
    // the main file's text; the alternative is a person watching their words
    // go nowhere.
    let doc = one_file("main.md", "before\n");
    {
        let text = doc.get_or_insert_text(session::SOURCE);
        let mut txn = doc.transact_mut();
        text.insert(&mut txn, 0, "what the old tab typed\n");
    }

    let done = session::repair(&doc, &rules().paths());
    assert!(done.contains(&session::Repair::Folded));
    assert_eq!(session::text_of(&doc), "what the old tab typed\n");
    assert_eq!(
        retired(&doc),
        "",
        "the retired text should be emptied once it is folded in"
    );
}

/* ---------------------------------------------------------------- the ceiling */

#[test]
fn the_ceiling_is_on_the_sum_of_every_text() {
    // A paper split into thirty files is allowed exactly what a paper in one
    // file is allowed. The bound is over the whole directory, so no number of
    // files can talk their way past it between them.
    let ceiling = 4096;
    let doc = one_file("main.typ", "");
    let filler = "x".repeat(1000);
    for nth in 0..3 {
        session::put_text(&doc, &format!("chapters/{nth}.typ"), &filler);
    }

    let over = {
        let scratch = session::new_doc();
        session::apply_update(&scratch, &session::encode_state(&doc)).unwrap();
        let before = session::encode_vector(&scratch);
        session::put_text(&scratch, "chapters/4.typ", &"y".repeat(2000));
        session::encode_diff(&scratch, &before).unwrap()
    };
    assert_eq!(
        session::admit_update(&doc, &over, ceiling, 200),
        Admission::TooLarge,
        "three files of a thousand bytes plus two thousand more is over four \
         thousand, whatever it is split into"
    );

    // And nothing was applied: the document a reader is looking at does not
    // move, not even for the instant a trim afterwards would have taken.
    assert_eq!(session::texts_of(&doc).len(), 4);
}

#[test]
fn a_document_may_hold_only_so_many_files() {
    let doc = one_file("main.typ", "");
    // A limit counts files, not keys: a text and its path are one file.
    let max_files = 2;

    // The boundary: the update that reaches the limit is taken.
    let to_the_limit = {
        let scratch = session::new_doc();
        session::apply_update(&scratch, &session::encode_state(&doc)).unwrap();
        let before = session::encode_vector(&scratch);
        session::put_text(&scratch, "two.typ", "");
        session::encode_diff(&scratch, &before).unwrap()
    };
    assert_eq!(
        session::admit_update(&doc, &to_the_limit, 1 << 20, max_files),
        Admission::Fits,
        "two files are exactly the limit"
    );
    session::apply_update(&doc, &to_the_limit).unwrap();

    // And the one past it is refused, with a reason of its own rather than
    // the size ceiling's, because the person is told which limit they met.
    let past_it = {
        let scratch = session::new_doc();
        session::apply_update(&scratch, &session::encode_state(&doc)).unwrap();
        let before = session::encode_vector(&scratch);
        session::put_text(&scratch, "three.typ", "");
        session::encode_diff(&scratch, &before).unwrap()
    };
    assert_eq!(
        session::admit_update(&doc, &past_it, 1 << 20, max_files),
        Admission::TooMany
    );
    assert_eq!(session::texts_of(&doc).len(), 2);
}

/* ------------------------------------------------------------------ the tree */

#[test]
fn a_checkpoint_is_the_whole_directory() {
    let doc = one_file("main.tex", "\\input{chapters/03}\n");
    session::put_text(&doc, "chapters/03.tex", "The third chapter.\n");
    session::put_text(&doc, "refs.bib", "@book{a,title={A}}\n");

    let (tree, bodies) = crate::room::tree_of(&doc, &HashMap::new());
    assert_eq!(tree.main, "main.tex");
    assert_eq!(
        tree.files.keys().collect::<Vec<_>>(),
        vec!["chapters/03.tex", "main.tex", "refs.bib"],
        "the keys are sorted, because the tree is named by its own bytes"
    );
    assert_eq!(tree.files["refs.bib"].kind, "text");
    assert_eq!(
        tree.size(),
        tree.files.values().map(|f| f.size).sum::<i64>()
    );

    // Every text is there to be written, once per distinct digest.
    for entry in tree.files.values() {
        assert!(bodies.contains_key(&entry.sha), "no bytes for {entry:?}");
    }

    // The name of a tree is the digest of what is stored, so naming it and
    // writing it cannot disagree, and the same directory always names itself
    // the same way.
    let (again, _) = crate::room::tree_of(&doc, &HashMap::new());
    assert_eq!(tree.digest(), again.digest());
    assert_eq!(
        tree.digest(),
        hex::encode(<sha2::Sha256 as sha2::Digest>::digest(tree.to_bytes()))
    );
}

#[test]
fn two_files_with_the_same_words_are_one_object() {
    let doc = one_file("main.typ", "same\n");
    session::put_text(&doc, "copy.typ", "same\n");
    let (tree, bodies) = crate::room::tree_of(&doc, &HashMap::new());
    assert_eq!(tree.files["main.typ"].sha, tree.files["copy.typ"].sha);
    assert_eq!(bodies.len(), 1, "the same bytes should be written once");
}

#[test]
fn the_timeline_says_which_paths_moved() {
    let doc = one_file("main.tex", "one\n");
    session::put_text(&doc, "refs.bib", "@book{a}\n");
    let (before, _) = crate::room::tree_of(&doc, &HashMap::new());

    session::put_text(&doc, "refs.bib", "@book{b}\n");
    session::put_text(&doc, "chapters/03.tex", "new\n");
    let (after, _) = crate::room::tree_of(&doc, &HashMap::new());

    assert_eq!(
        after.changed_from(Some(&before)),
        vec!["chapters/03.tex", "refs.bib"],
        "the file that did not change should not be listed"
    );

    // A file that went away is a change too, and is listed under the name it
    // had rather than not at all.
    let doc2 = one_file("main.tex", "one\n");
    let (fewer, _) = crate::room::tree_of(&doc2, &HashMap::new());
    assert_eq!(fewer.changed_from(Some(&before)), vec!["refs.bib"]);

    // With no parent, everything is new.
    assert_eq!(before.changed_from(None), vec!["main.tex", "refs.bib"]);
}

#[test]
fn a_checkpoint_from_before_directories_reads_as_one_file() {
    // The old entries are never rewritten. An entry with no `tree` is read as
    // the one file it is, which is what makes the timeline continuous across
    // the change and a rollback lossless.
    let old = Checkpoint {
        sha: "abc".to_string(),
        why: "quiet".to_string(),
        size: 12,
        ..Checkpoint::default()
    };
    assert!(!old.tree);
    let read = Tree::of_one_file("main.typ", "0123456789ab", &old.sha, old.size);
    assert_eq!(read.main, "main.typ");
    assert_eq!(read.files["main.typ"].sha, "abc");
    assert_eq!(read.files["main.typ"].id, "0123456789ab");
    assert_eq!(read.size(), 12);
}

#[test]
fn a_restore_puts_every_file_back_at_one_moment() {
    let doc = one_file("main.tex", "first\n");
    session::put_text(&doc, "refs.bib", "@book{a}\n");
    let main_id = session::main_id(&doc);
    let (recorded, bodies) = crate::room::tree_of(&doc, &HashMap::new());

    // The document moves on: one file edited, one added, one removed.
    session::put_text(&doc, "main.tex", "second\n");
    session::put_text(&doc, "chapters/03.tex", "later\n");
    {
        let (files, path_map) = (
            doc.get_or_insert_map("files"),
            doc.get_or_insert_map("paths"),
        );
        let gone = session::paths_of(&doc)
            .into_iter()
            .find(|(_, path)| path == "refs.bib")
            .map(|(id, _)| id)
            .unwrap();
        let mut txn = doc.transact_mut();
        files.remove(&mut txn, &gone);
        path_map.remove(&mut txn, &gone);
    }

    session::restore(&doc, &recorded, &bodies);

    let texts = session::texts_of(&doc);
    assert_eq!(texts.len(), 2, "the tree is the whole of what comes back");
    assert_eq!(texts["main.tex"], "first\n");
    assert_eq!(texts["refs.bib"], "@book{a}\n");
    assert!(!texts.contains_key("chapters/03.tex"));
    assert_eq!(
        session::main_id(&doc),
        main_id,
        "a file present on both sides keeps its id, so an editor watching a \
         restore sees the words change rather than their file disappear"
    );
}

#[test]
fn a_restore_of_a_one_file_tree_reaches_a_directory() {
    // Restoring a checkpoint taken before directories, into a document that is
    // one now: the shapes meet, because the old entry is read as a tree.
    let doc = one_file("main.typ", "now\n");
    let old = Tree::of_one_file("main.typ", &session::main_id(&doc), "sha-of-then", 5);
    let mut bodies = HashMap::new();
    bodies.insert("sha-of-then".to_string(), "then\n".to_string());

    session::restore(&doc, &old, &bodies);
    assert_eq!(session::text_of(&doc), "then\n");
    assert_eq!(session::main_path(&doc), "main.typ");
}

/* ------------------------------------------------------- publishing a directory */

/// A directory on disk, for the walk to find things in.
fn scratch(files: &[(&str, &[u8])]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    for (at, bytes) in files {
        let path = dir.path().join(at);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("mkdir");
        }
        std::fs::write(&path, bytes).expect("write");
    }
    dir
}

#[test]
fn the_walk_leaves_behind_what_is_not_part_of_a_document() {
    let dir = scratch(&[
        ("main.typ", b"= A paper\n"),
        ("chapters/03.typ", b"third\n"),
        ("refs.bib", b"@book{a}\n"),
        ("fig/one.png", b"\x89PNG"),
        // The output of the document itself, which a compiler wrote.
        ("main.pdf", b"%PDF-1.4"),
        // A dotfile, and a file inside a dot directory.
        (".env", b"SECRET=1\n"),
        (".git/config", b"[core]\n"),
    ]);
    let found = crate::cli::files_under(dir.path(), "main", &|_| false);
    assert_eq!(
        found,
        vec![
            "chapters/03.typ".to_string(),
            "fig/one.png".to_string(),
            "main.typ".to_string(),
            "refs.bib".to_string(),
        ],
        "the walk kept something it should not have, or dropped something it should not"
    );
}

#[test]
fn the_walk_skips_what_git_ignores() {
    let dir = scratch(&[
        ("main.typ", b"= A paper\n"),
        ("scratch.txt", b"working notes\n"),
    ]);
    // A real working tree, because what is ignored is asked of git rather than
    // worked out from the file: precedence, nesting and the global config are
    // git's own, and reimplementing them is a way to disagree with it.
    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .arg("-C")
            .arg(dir.path())
            .args(args)
            .output()
    };
    if git(&["init", "-q"]).is_err() {
        eprintln!("skipping: no git on this machine");
        return;
    }
    std::fs::write(dir.path().join(".gitignore"), "scratch.txt\n").unwrap();

    let found = crate::cli::files_under(dir.path(), "main", &crate::cli::git_ignores(dir.path()));
    assert_eq!(found, vec!["main.typ".to_string()], "{found:?}");
}

#[test]
fn which_file_is_the_document() {
    // The only text at the top level whose extension is a format this renders.
    let one = vec![
        "chapters/03.typ".to_string(),
        "main.typ".to_string(),
        "refs.bib".to_string(),
    ];
    assert_eq!(crate::cli::main_file(&one, "").unwrap(), "main.typ");

    // Two candidates and neither called `main`: a refusal that lists them,
    // because guessing wrong here publishes the wrong document.
    let several = vec!["paper.typ".to_string(), "notes.md".to_string()];
    let refused = crate::cli::main_file(&several, "").unwrap_err();
    assert!(refused.contains("--main"), "{refused}");
    assert!(
        refused.contains("paper.typ") && refused.contains("notes.md"),
        "{refused}"
    );

    // Unless one of them is called `main`, which is what an author means by it.
    let named = vec!["paper.typ".to_string(), "main.typ".to_string()];
    assert_eq!(crate::cli::main_file(&named, "").unwrap(), "main.typ");

    // Asked for by name, and refused when it is not there.
    assert_eq!(
        crate::cli::main_file(&several, "notes.md").unwrap(),
        "notes.md"
    );
    assert!(crate::cli::main_file(&several, "absent.typ").is_err());

    // A directory with nothing that could be a document.
    let none = vec!["refs.bib".to_string(), "fig/one.png".to_string()];
    assert!(crate::cli::main_file(&none, "")
        .unwrap_err()
        .contains("no document"));
}

#[test]
fn a_compile_says_which_siblings_it_read() {
    // This is what lets `publish <file>` warn that the document it just
    // compiled will not compile for a reader: it read files that are not
    // being sent.
    let dir = scratch(&[
        ("main.typ", b"#import \"lib.typ\": word\n= T\n#word\n"),
        ("lib.typ", b"#let word = \"sibling\"\n"),
    ]);
    let main = dir.path().join("main.typ");
    let source = std::fs::read_to_string(&main).unwrap();
    let (compiled, read) = crate::render::read_and_note(&main, &source, "T");
    assert!(compiled.page.is_some(), "the fixture does not compile");
    assert_eq!(read, vec!["lib.typ".to_string()]);

    // And a document that reads nothing says so, which is what stops the hint
    // appearing on every ordinary publish.
    let alone = scratch(&[("main.typ", b"= T\n\nJust prose.\n")]);
    let path = alone.path().join("main.typ");
    let (_, none) =
        crate::render::read_and_note(&path, &std::fs::read_to_string(&path).unwrap(), "T");
    assert!(none.is_empty(), "{none:?}");
}

#[test]
fn a_document_named_without_a_directory_compiles() {
    // `Path::new("paper.typ").parent()` is `Some("")`, not `None`, and an
    // empty path cannot be made absolute. So `komodoc publish paper.typ`, run
    // from the directory the file is in -- which is how anybody would run it
    // -- failed with "cannot make an empty path absolute" and reported the
    // document as one that did not compile.
    let dir = scratch(&[
        ("main.typ", b"#import \"lib.typ\": word\n= T\n#word\n"),
        ("lib.typ", b"#let word = \"sibling\"\n"),
    ]);
    let here = std::env::current_dir().expect("cwd");
    std::env::set_current_dir(dir.path()).expect("cd");
    let (compiled, read) = crate::render::read_and_note(
        std::path::Path::new("main.typ"),
        "#import \"lib.typ\": word\n= T\n#word\n",
        "T",
    );
    std::env::set_current_dir(here).expect("cd back");
    assert!(
        compiled.page.is_some(),
        "a file named with no directory did not compile: {:?}",
        compiled.diagnostics
    );
    assert_eq!(read, vec!["lib.typ".to_string()]);
}

#[test]
fn one_place_decides_what_a_filename_says_a_document_is() {
    // `publish <directory>` chooses the main file by asking which files are
    // documents, and it used to ask three predicates in a row -- which is
    // three places to forget when a fourth format arrives. A directory whose
    // document was a `.tex` would have been refused as holding no document at
    // all. Adding a format to `document_format` is what makes it a candidate
    // here, and this is the test that says so.
    for (name, format) in [
        ("main.typ", Some("typst")),
        ("main.tex", Some("latex")),
        ("paper.md", Some("markdown")),
        ("notes.markdown", Some("markdown")),
        ("index.html", Some("html")),
        ("refs.bib", None),
        ("fig/one.png", None),
    ] {
        assert_eq!(crate::render::document_format(name), format, "{name}");
    }

    // And the choice of main file follows it rather than a list of its own.
    let files = vec![
        "refs.bib".to_string(),
        "fig/one.png".to_string(),
        "paper.md".to_string(),
    ];
    assert_eq!(crate::cli::main_file(&files, "").unwrap(), "paper.md");
}
