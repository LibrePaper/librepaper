# SPEC: Zotero integration

The remaining bibliography work is importing and searching Zotero libraries
from the author's machine. Reuse the existing document library and citation
completion. LibrePaper stores no third-party credentials and runs no background
sync on the author's behalf.

## Zotero

Zotero holds the library the author actually maintains, and the useful
integration is the one that puts it in the tree as a `.bib` without a
credential ever reaching LibrePaper.

**From the terminal.** `librepaper bib pull` reads the author's Zotero library on
their machine and writes it into the document's tree through the existing
publish path:

```sh
librepaper bib pull "$LIBREPAPER_DOCUMENT" --collection "Paper: sandbox costs"
librepaper bib pull "$LIBREPAPER_DOCUMENT" --group 2451 --into references.bib
```

The default target is `references.bib`, replaced whole on each pull. Entries
are written in a deterministic order with stable keys and stable field
ordering, so a pull that adds one paper is a one-entry diff in the history and
not a rewrite of the file. The pulled file belongs to the pull: hand edits to
it are overwritten, and an author who wants entries of their own keeps them in
a second `.bib`, which the library unions in. Saying that plainly is kinder
than a merge that guesses.

**From the local app.** The local app in `crates/librepaper/src/local/` already
discovers tools on the author's machine for LaTeX; it gains one more
discovery, a Zotero running on its loopback port, and exposes search and
export over the existing bridge protocol. With it, the browser's completion
list can offer entries the tree does not have yet, and choosing one adds the
entry to the pulled `.bib` and inserts the key in one gesture. Without it,
completion is over the tree, which is the whole feature minus the convenience.

The exact loopback endpoint, its version negotiation and what Zotero versions
expose it belong to the implementation milestone, in the manner
[wasmtex.md](wasmtex.md) defers local transport research. The capability is
reported like any other local capability: present, absent, or incompatible,
never assumed.

**What is deliberately not built.** Not a server-side Zotero sync. It would be
the first third-party credential LibrePaper stores and the first background job
it runs for a user, and it would drag in secret storage and rotation, token
revocation, the interaction with the retention and erasure rules in
[catalog.md](catalog.md), and a rate-limited third-party API on the serving
path. A browser-only author with no Zotero on the machine exports a `.bib`
from Zotero and uploads it, which is the workflow they have today and which
keeps working.

## Scope boundaries

- No reference manager other than Zotero.
- No server-side Zotero sync or writing back to Zotero; imports are one-way.
- No searching Crossref, PubMed, arXiv, or other external databases.
- No changes to citation rendering or citation-style configuration.
- Registering and rendering `.qmd` documents belongs to [quarto.md](quarto.md).

## Order of work

1. Establish the supported Zotero export/search transport, version negotiation,
   and citation-key policy. Resolve key stability before implementing imports.
2. Implement `librepaper bib pull` with collection/group selection, deterministic
   exports, and clear replacement and failure behavior.
3. Add Zotero discovery, search, and export to the local bridge, reporting
   present, absent, or incompatible capabilities.
4. Extend citation completion with local Zotero search. Selecting an entry
   adds it to the document's bibliography and inserts its key in one gesture.

## Tests

Rust, `crates/librepaper/src/tests/bib_cli.rs`: `bib pull` writes a deterministic
file; pulling again with one added entry produces a one-entry diff; existing
citation keys remain stable. An unreachable Zotero returns setup guidance and
preserves the existing bibliography rather than replacing it with an empty file.

Local bridge: discovery distinguishes supported, absent, and incompatible
Zotero installations; search and export failures leave document files intact.

Browser: choosing a Zotero result adds the entry and inserts its key; failed
imports leave the source untouched; completion over existing document entries
continues to work when Zotero is unavailable.

## Open questions

- **Key stability across pulls.** Zotero's own citation keys are a Better
  BibTeX concept, not a Zotero one. Which key a pulled entry gets, and whether
  it can change under an author who has already cited it, needs deciding
  before the pull command is written; a pull that renames keys silently breaks
  every citation in the document.

## References

- [wasmtex.md](wasmtex.md) -- local discovery and the bridge protocol.
- [quarto.md](quarto.md) -- remaining Quarto support.
- [catalog.md](catalog.md) -- secrets, retention, and erasure constraints.
