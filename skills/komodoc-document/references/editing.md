# Editing source

## The SHA is the contract

`komodoc agent read` and `komodoc agent source` return the source together
with its SHA. `--expected-sha` must be the SHA of the source **you actually
inspected and revised** — not one carried over from an earlier read, and not
omitted.

```sh
komodoc agent source "$KOMODOC_DOCUMENT"
# ... revise, writing the full new source to revised.md ...
komodoc agent edit "$KOMODOC_DOCUMENT" --file revised.md --expected-sha SOURCE_SHA
```

If the edit fails as stale, someone changed the document while you worked.
Read again, reconcile their changes into your revision, and retry with the new
SHA. Never remove the check to force the edit through: that discards their
work silently.

## Input

- `--file PATH` reads the new source from a local UTF-8 file. Prefer this.
- `--source TEXT` passes source inline. Only for short, single-line content —
  it goes through the shell and through your transcript.

The two conflict; pass exactly one. An edit replaces the whole source, so the
file must contain the complete document, not a fragment or a diff.

## Multi-file projects

A directory document has several files. `--path` selects one, relative to the
document root; omitted, it means the document's main file.

```sh
komodoc agent edit "$KOMODOC_DOCUMENT" \
  --path chapters/introduction.tex \
  --file introduction.tex \
  --expected-sha SHA_OF_THAT_FILE
```

The SHA belongs to the file named by `--path`, not to the document as a whole.
`komodoc agent comment` takes `--path` the same way for annotations that
anchor inside a non-main file.

## Checkpoints

```sh
komodoc agent checkpoint "$KOMODOC_DOCUMENT" --why "revised methods section"
```

A checkpoint asks the server for a durable point in the document's history.
Take one after a substantive set of edits so the user has something to return
to. `--why` defaults to `agent`; a short description is more useful.

## Transport

Edits synchronize through the live collaborative session and wait for durable
acknowledgement — the same path the browser editor uses. Do not attempt to
write source through an unrelated HTTP endpoint; it bypasses the merge, the
comment re-anchoring, and the permission model.

## Don't reformat what you weren't asked to

A formatter that rewrites every line is a change against everyone in the
document and re-anchors every comment. Change the text the task calls for and
leave the rest byte-identical.
