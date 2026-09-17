//! The shared document, as the server holds it.
//!
//! One `LoroDoc` per open document, held natively so the server and the command
//! line can hold it without a JavaScript runtime beside them. The browser holds
//! the same document through the same Rust compiled to WebAssembly, so there is
//! one implementation of the format rather than two agreeing by convention.
//!
//! ## A document is a directory
//!
//! What the document holds is four maps rather than one text, because a paper
//! is `main.tex`, a `chapters/`, a `refs.bib` and a `fig/`, and a document that
//! is one text renders on one laptop and nowhere else.
//!
//! | map | keys | values |
//! | --- | --- | --- |
//! | `files` | an id | the text at it |
//! | `paths` | an id | the path that text is known by |
//! | `assets` | a path | the digest of the bytes at it |
//! | `meta` | `main` | the id of the main file |
//!
//! A text is keyed by an id and named separately because that is what makes a
//! rename free: renaming moves a string in `paths` and leaves the text where it
//! is, so a keystroke somebody makes into the file at the moment it is renamed
//! lands in the text it was always going to land in. Keying texts by path
//! instead would make a rename a delete and an insert, and would lose that
//! keystroke into a text no key reaches.
//!
//! Assets are keyed by path, because their bytes are not here: an asset is kept
//! in the store under its digest, and what the document holds is the name
//! somebody gave it.
//!
//! ## Offsets are UTF-16
//!
//! Every offset that crosses this module counts UTF-16 code units, because that
//! is what a browser counts in. The `*_utf16` family is the only text API used
//! here. Two things underneath are indexed differently and are converted at
//! their boundary rather than leaking: cursors are Unicode code points (see
//! [`anchors`]), and diff deltas are whichever basis the crate that computed
//! them was built for (see [`crate::document::hunks`]).

mod anchors;
mod edits;
mod presence;
mod repair;
mod shape;
mod sync;

pub use anchors::*;
pub use edits::*;
pub use presence::*;
pub use repair::*;
pub use shape::*;
pub use sync::*;

#[cfg(test)]
mod astral_tests {
    //! Every offset that crosses this module counts UTF-16 code units, because
    //! that is what a browser counts in (§3.2). An astral character is two of
    //! those and one code point, so it is the character that tells the two
    //! apart -- and a path that quietly counts code points is correct on every
    //! test that does not contain one.
    //!
    //! These are those tests. The emoji is placed BEFORE the edit in each case,
    //! so that an implementation counting code points lands one unit short and
    //! corrupts the text rather than merely disagreeing about a number.

    use super::*;
    use loro::LoroDoc;

    /// One astral character, two UTF-16 units, one code point.
    const EMOJI: &str = "\u{1F600}";

    fn doc_with(body: &str) -> LoroDoc {
        let doc = new_doc();
        put_text(&doc, "main.md", body);
        doc
    }

    fn body(doc: &LoroDoc) -> String {
        texts_of(doc).get("main.md").cloned().unwrap_or_default()
    }

    #[test]
    fn an_edit_past_an_astral_character_lands_where_it_was_aimed() {
        // "😀 cat" -- the emoji is 2 UTF-16 units, so "cat" starts at 3.
        let doc = doc_with(&format!("{EMOJI} cat"));
        let edits = [wasm_helpers::text::Edit {
            at: 3,
            delete: 3,
            insert: "dog".into(),
        }];
        assert!(apply_edits_at(&doc, "main.md", &edits));
        assert_eq!(
            body(&doc),
            format!("{EMOJI} dog"),
            "an offset past an emoji is UTF-16; counting code points would eat the space"
        );
    }

    #[test]
    fn an_insert_between_two_astral_characters_splits_neither() {
        let doc = doc_with(&format!("{EMOJI}{EMOJI}"));
        let edits = [wasm_helpers::text::Edit {
            at: 2,
            delete: 0,
            insert: "x".into(),
        }];
        assert!(apply_edits_at(&doc, "main.md", &edits));
        assert_eq!(body(&doc), format!("{EMOJI}x{EMOJI}"));
    }

    #[test]
    fn replacing_a_whole_body_keeps_astral_characters_intact() {
        let doc = doc_with("plain");
        let wanted = format!("{EMOJI} a {EMOJI} b {EMOJI}");
        put_text(&doc, "main.md", &wanted);
        assert_eq!(body(&doc), wanted);
        // And again, so the minimal-diff path runs against astral text rather
        // than against an empty file.
        let second = format!("{EMOJI} a {EMOJI} c {EMOJI}");
        put_text(&doc, "main.md", &second);
        assert_eq!(body(&doc), second);
    }

    #[test]
    fn an_astral_document_survives_a_round_trip_through_the_wire() {
        let doc = doc_with(&format!("{EMOJI} first"));
        put_text(&doc, "notes.md", &format!("second {EMOJI}"));
        let peer = new_doc();
        apply_update(&peer, &encode_state(&doc)).expect("an encoded document decodes");
        assert_eq!(texts_of(&peer), texts_of(&doc));
    }

    #[test]
    fn a_cursor_past_an_astral_character_resolves_to_the_same_utf16_offset() {
        let doc = doc_with(&format!("{EMOJI} cat"));
        let file_id = paths_of(&doc)
            .into_iter()
            .find_map(|(id, path)| (path == "main.md").then_some(id))
            .expect("main file has a stable Loro id");
        // Aim at the "c": UTF-16 offset 3, which is code point 2.
        let cursor = cursor_at_file_id(&doc, &file_id, 3, loro::cursor::Side::Left)
            .expect("an anchor in a file that exists");
        let (offset, _) = offset_of_cursor(&doc, &cursor).expect("it resolves");
        assert_eq!(
            offset, 3,
            "anchors speak UTF-16 on the way in and on the way out"
        );

        // Insert before it; the anchor should follow its content, not its number.
        let edits = [wasm_helpers::text::Edit {
            at: 0,
            delete: 0,
            insert: "AB".into(),
        }];
        assert!(apply_edits_at(&doc, "main.md", &edits));
        let (moved, _) = offset_of_cursor(&doc, &cursor).expect("it still resolves");
        assert_eq!(moved, 5, "two units inserted before it moved it by two");
    }
}
