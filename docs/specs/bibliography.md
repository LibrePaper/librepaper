# SPEC: Zotero integration

The remaining bibliography work is importing and searching Zotero libraries
from the author's machine. Reuse the existing document library and citation
completion. LibrePaper runs no background sync on the author's behalf, and
stores no third-party credential on the server.

## Three paths, and what each costs

An author can get their Zotero library into a document three ways. They are
not competing designs; they serve different people, and they differ in one
property worth naming up front.

| Path | Needs | Third-party credential |
| --- | --- | --- |
| Export a `.bib` from Zotero and upload it | nothing | none |
| `librepaper bib pull` / local app | the CLI or app installed | none |
| Zotero web API from the browser | a Zotero API key | **yes** |

The first two are credential-free *by construction*. Uploading a `.bib` means
the author authenticated inside Zotero, which already holds their
credentials, and handed LibrePaper a file. The local paths read files on the
author's own machine that they already have access to. Neither creates a
secret that can leak, expire, or need revoking.

The web API is the only path that works with no install, and the only one
that introduces a credential into the system. That is the trade. It is not
"is sync too heavy" -- sync is a separate question, settled below.

## Citation keys

**Decided: LibrePaper generates citation keys itself, on every path, and
never adopts a key supplied by Zotero or by Better BibTeX.**

Better BibTeX is a local Zotero plugin. Its keys do not exist on the web API
and do not exist for an author without the plugin. Adopting BBT keys where
they are available would mean the same paper gets one key when pulled through
the CLI and a different key when imported through the browser -- the drift
problem, moved from front ends to transports. Owning generation unconditionally
makes a key a function of the entry's own fields, identical on every path, and
removes the plugin as an undeclared dependency.

The generation rule, the collision rule, and the removal rule are one
decision and must land before any import is written. A pull that silently
renames a key breaks every citation in the document; so does one that
silently drops an entry still being cited.

- **Generation.** A deterministic function of author, year, and title, with a
  defined transliteration for non-ASCII names. Same entry, same key, forever.
- **Collision.** Two entries deriving the same key get a defined
  disambiguating suffix, assigned in the entry order below so the suffix
  itself is stable across pulls.
- **Removal.** An entry that leaves the Zotero collection but is still cited
  in the document is reported, and the pull fails rather than producing a
  dangling citation. The author removes the citation or keeps the entry in a
  second `.bib`.

## File format

"Deterministic" is a promise about bytes, not a sentiment. A pull that adds
one paper must be a one-entry diff.

- Entries sorted by generated citation key.
- Fields within an entry in a fixed, documented order.
- UTF-8 throughout; no TeX escaping of accented characters.
- Title casing brace-protected.
- A documented Zotero-item-type to BibTeX-entry-type mapping, with a defined
  fallback for types that have no BibTeX equivalent.
- Attachment paths, notes, and tags are not exported. Local filesystem paths
  must never reach a document tree.

## Zotero

**From the terminal.** `librepaper bib pull` reads the author's Zotero library on
their machine and writes it into the document's tree through the existing
publish path:

```sh
librepaper bib pull "$LIBREPAPER_DOCUMENT" --collection "Paper: sandbox costs"
librepaper bib pull "$LIBREPAPER_DOCUMENT" --group 2451 --into references.bib
```

The default target is `references.bib`, replaced whole on each pull. The
pulled file belongs to the pull: hand edits to it are overwritten, and an
author who wants entries of their own keeps them in a second `.bib`, which the
library unions in. Saying that plainly is kinder than a merge that guesses.

The command requires write access to the document and fails with that
explanation on a document the author can only read or comment on.

**From the local app.** The local app in `crates/librepaper/src/local/` already
discovers tools on the author's machine for LaTeX; it gains one more
discovery, a Zotero on the machine, and exposes search and export over the
existing bridge protocol. With it, the browser's completion list can offer
entries the tree does not have yet, and choosing one adds the entry to the
pulled `.bib` and inserts the key in one gesture. Without it, completion is
over the tree, which is the whole feature minus the convenience.

The capability is reported like any other local capability, as an additive
field on `Capabilities` alongside `quarto` and `calepin`: present, absent, or
incompatible, never assumed.

**From the browser.** For an author with no CLI and no local app, the Zotero
web API reaches the library they have synced to zotero.org. This path is
designed but not ratified; its gate conditions are in Open questions.

Its shape, if built:

- The author supplies a Zotero API key. It is scoped and revocable at
  zotero.org independently of their account password.
- The key is entered into a password input inside a real form, so the
  browser's own password manager offers to save it. The key then lives in OS
  credential storage, is not readable by page scripts until the author
  autofills and submits, and syncs across their devices through browser sync.
- **LibrePaper stores the key nowhere.** Not on the server, and not in
  `localStorage`, `sessionStorage`, or IndexedDB. Browser storage is readable
  by any script on the origin, and this origin renders author-authored content
  in a collaborative editor and a PDF viewer. A LibrePaper session token is
  ours and dies at sign-out; a Zotero key belongs to a service we do not
  control and that the author would likely never think to revoke.
- The fetch is foreground and initiated by the author. Note that
  "user-initiated" describes *when* the request happens; it is not itself a
  statement about where the credential lives. Both properties are required,
  and they are independent.
- Imported entries land in the document through the normal edit path, keyed by
  the same generator as every other path.

**What is deliberately not built.** Not a server-side Zotero sync: not a stored
key on the server, not a background job run on an author's behalf, not a
rate-limited third-party API on the serving path. Those costs belong to sync
specifically, and a foreground import the author waits for incurs none of
them. The distinction matters, because rejecting sync is not a reason to
reject the web API.

## Where machine access lives

**The `local/` crate owns all access to the author's machine. The loopback
service is one entry point into it, not a required hop.**

The precedent exists: `librepaper local doctor` calls `discovery::discover()`
directly (`local/cli.rs:345`) with no service running and no pairing, and the
bridge service calls the same function to answer `GET capabilities`. One
module that knows how to inspect the machine, two front ends onto it.

Zotero follows that shape. A `local/zotero.rs` knows how to find the library,
read it, map items to BibTeX, and apply the key policy. `librepaper bib pull`
calls it directly -- no service, no port, no paired origin, so it works in a
Makefile, in CI, and over SSH. The bridge exposes search and export by calling
the same code. Key generation, item mapping, and ordering are written once.
Two implementations of citation-key generation that drift apart is a
sufficiently nasty bug class to rule out by construction.

Reading a Zotero library is a read, not an execution, so it does not go
through `confine.rs`. It does need a read scope, which the folder-binding
model does not currently answer; see Open questions.

## Scope boundaries

- No reference manager other than Zotero.
- No server-side Zotero sync, no server-stored Zotero credential, and no
  writing back to Zotero; imports are one-way.
- No searching Crossref, PubMed, arXiv, or other external databases.
- No changes to citation rendering or citation-style configuration.
- No adoption of Better BibTeX citation keys on any path.
- Registering and rendering `.qmd` documents is Quarto support, not this work.

## Order of work

1. Decide and document the citation-key policy: generation, collision, and
   removal. Everything else depends on it and nothing else may start first.
2. Decide the file format: entry order, field order, encoding, item-type
   mapping.
3. Resolve the web API gate conditions in Open questions, since the answer
   determines whether step 6 exists and how early it should run.
4. Implement `local/zotero.rs`: library discovery, read, item mapping, key
   generation.
5. Implement `librepaper bib pull` as a thin front end onto it, with
   collection/group selection and clear replacement and failure behavior.
6. Add Zotero discovery, search, and export to the local bridge, reporting
   present, absent, or incompatible capabilities.
7. Extend citation completion with local Zotero search. Selecting an entry
   adds it to the document's bibliography and inserts its key in one gesture.
8. If ratified, the browser web-API import, keyed by the same generator.

Steps 5 and 8 are the two that reach an author with nothing installed and an
author with a terminal respectively. For a browser-first product, step 8 likely
reaches more people than step 5; sequence accordingly once step 3 resolves.

## Tests

Rust, `crates/librepaper/src/tests/bib_cli.rs`: `bib pull` writes a deterministic
file; pulling again with one added entry produces a one-entry diff; existing
citation keys remain stable. Two entries deriving the same key get stable
disambiguated keys, and the suffixes do not shuffle on a later pull. An entry
removed from the collection while still cited fails the pull and leaves the
bibliography intact. Accented and non-Latin author names produce stable keys
and round-trip as UTF-8. An unreachable Zotero returns setup guidance and
preserves the existing bibliography rather than replacing it with an empty
file. A pull against a document the author cannot write fails with that
reason. Two concurrent pulls do not interleave into a corrupt file.

Local bridge: discovery distinguishes supported, absent, and incompatible
Zotero installations; search and export failures leave document files intact.

Browser: choosing a Zotero result adds the entry and inserts its key; failed
imports leave the source untouched; completion over existing document entries
continues to work when Zotero is unavailable. If the web-API path ships, the
API key is absent from `localStorage`, `sessionStorage`, IndexedDB, document
state, and error reports -- asserted, not assumed.

## Open questions

- **Web API gate conditions.** Does `api.zotero.org` send CORS headers
  permitting a direct browser-origin call? If not, the path needs a proxy, and
  a stateless proxy forwarding a key supplied per request is a materially worse
  trade that must be re-argued. Separately, Zotero's OAuth flow appears to be
  OAuth 1.0a, whose request signing needs a client secret and therefore a
  server component; if so, the credential-free-server version of this path
  exists only with a manually pasted key. Both need verifying before step 8 is
  designed, and together they decide whether the path is worth building.
- **Read scope for Zotero data.** A Zotero library lives outside any bound
  folder, so "which paths may be read, and who granted that" is a question the
  folder-binding model does not answer today. Reading is not executing, so
  confinement is not the mechanism; consent still is.
- **Collection selection.** Zotero collection names are neither unique nor
  flat. `--collection` needs an ambiguity rule, a nesting rule, a stable
  identifier so a renamed collection does not silently stop matching, and a
  way to list what is available. An empty collection must preserve rather than
  truncate, as an unreachable one does.
- **Remembering the target.** Whether the collection-to-file mapping is stored
  with the document, so a re-pull is one command rather than a retyped
  invocation. The one-entry-diff property is undercut if every pull means
  retyping flags.
- **Search privacy.** Bridge-backed completion puts library metadata for
  entries *not in the document* into the browser. Whether any of it reaches
  the server, whether it is cached, and how long it lives.

## References

- [Local bridge protocol](../../crates/librepaper/src/local/protocol.rs) -- local service wire contract.
- [Local CLI](../../crates/librepaper/src/local/cli.rs) -- the `doctor` precedent for one module, two front ends.
- [Tool discovery](../../crates/librepaper/src/local/discovery.rs) -- the discovery and capability-reporting pattern Zotero follows.
- [Citation completion](../../web/src/lib/bibliography.js) -- the browser completion this work extends.
- [Publish CLI tests](../../crates/librepaper/src/tests/publish_cli.rs) -- the test-module convention `bib_cli.rs` follows.
