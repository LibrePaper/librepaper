---
title: "Architecture"
---

## Where code runs

LibrePaper ships as a single executable: the server, the command line, and the
embedded web application. At runtime the work divides three ways.

| Where | What happens there |
| --- | --- |
| The browser | Editing, typesetting, rendering, offline copies |
| The server | Identity, authority, durability, live relay, review decisions |
| The author's machine | Quarto, Typst to HTML, Zotero |

Typesetting runs in the browser. The Markdown, Typst and LaTeX engines are
WebAssembly modules that execute on the reader's machine. The deployment never
compiles a document and never stores a compiler, so a small server can host a
lot of papers and a self-hoster does not operate a TeX installation. The LaTeX
engines and their pinned TeX Live package set are fetched from an HTTPS mirror,
which an operator can point at their own copy.

Authority is decided by the server, on every request, never inferred from
what a browser sent. Which proposed change enters a document is the server's
decision alone.

The author's machine is optional. Markdown, LaTeX and Typst all compile in
the browser, Typst to PDF included, so nothing local is required to write or
read a paper. A loopback companion, the same binary run as
`librepaper local start`, covers the two cases the browser cannot: Quarto,
which executes code, and Typst to self-contained HTML. It also reaches Zotero,
read-only. No TeX job ever reaches it.

## Records and caches

Stored information is either a record or a cache.

A *record* describes what somebody did, at a moment. Nothing that happens
afterwards may edit it. What a reviewer selected when they left a comment is a
record, and so is who wrote a sentence.

A *cache* describes where something is now. It is recomputed whenever the
document moves and discarded without consequence. Where a commented passage sits
in today's version of the paper is a cache.

The two are kept in separate fields, computed by separate code, and the cache is
never written back into the record. A comment that loses its place reports that
rather than being re-pointed at different words.

## Trust model

Four kinds of participant, in decreasing order of what they may assume.

1. The operator runs the binary and the database, and sees every draft,
   comment, identity and presence event in the clear. There is no end-to-end
   encryption. Self-hosting is the answer to not wanting to trust someone
   else's operator.
2. The owner of a document holds every authority over it.
3. A link holder holds exactly the role their link names, for as long as the
   link lives. The link is the credential, which is why links expire.
4. A document is hostile. The bytes somebody uploads run their own scripts
   and are framed inside the reader, and may have been written by anyone with
   editor access.

The fourth point is what most of the isolation in the system exists for.
Published documents are served from a second configured origin, and the server
refuses to start if the two share a host. A published document may run its own
code and may not fetch code from another host. Messages crossing between the
reader and the framed document are origin-checked at both ends and addressed to
a named origin rather than a wildcard.

## The document

### A directory

A paper is rarely one file. The shared, live object in LibrePaper is a whole
directory, held as four maps in one CRDT document:

| map | keys | values |
| --- | --- | --- |
| `files` | a file id | the text at it |
| `paths` | a file id | the path that text is known by |
| `assets` | a path | the digest of the bytes at it |
| `meta` | `main`, engine, markers | the document's own settings |

### File identity

A text is stored under an opaque id. The path it is known by is a separate entry
pointing at that id. A rename moves one string in `paths` and leaves the text
where it is, so a keystroke made into a file while it is being renamed still
lands in the right text.

A comment names the file it is about by id, so renaming a chapter does not
orphan the review on it.

### Assets

A figure's bytes are not in the shared document. The document holds the path and
the digest. The bytes sit in the object store under that digest. Identical
uploads are stored once, and an asset is verified on retrieval: the bytes either
hash to the digest the document names or they do not.

### Source and rendering

The editable source is the only authoritative content. A comment made on a
rendered page is translated into a range of the source when it is made, and the
source range is what is kept, so re-rendering cannot orphan a comment.

### One implementation

The CRDT is a single Rust library. The server links it natively, so it can hold
a document without a JavaScript runtime beside it. The browser runs the same
library compiled to WebAssembly. Encoding an anchor and decoding it months later
to decide which range a rejected change rewrites is therefore the same code on
both sides.

### UTF-16 offsets

Every offset that crosses a boundary counts UTF-16 code units, matching what a
browser counts and what every reader selection arrives measured in.

Two things underneath count differently and are converted at their edge.
Cursors, the CRDT anchors described under [comments](#comments), are
indexed in Unicode code points. Diffs are indexed in whichever basis the
library was compiled for: code points on the server, UTF-16 in the browser.

Diff offsets are held to two rules. Diff information never crosses the network;
each side computes the diff it needs from the state it already has. And a diff
offset is never compared with a UTF-16 offset. Anything stored outside the
document refers to positions by cursor, which carries no integer basis. Both
rules are pinned by tests.

### Size ceilings

A deployment bounds two separate things. `--document-size-limit` caps the
combined source text, 4 MB by default and 8 MB at most. What must be durably
saved is the CRDT state behind that text, which carries the edit history as
well, and has its own ceiling. A heavily rewritten paper can reach the second
without approaching the first, so a refusal for that reason says which limit was
hit and that history counts toward it.

A configuration whose limits could accept work the deployment could not durably
save is refused at startup rather than at the first save.

## Storage and history

### Two stores

A deployment keeps its state in PostgreSQL and its bulk bytes in an object
store.

Postgres holds what must be ordered, related and transactional: accounts,
documents, grants and share links, annotations and replies, the version
timeline, bundle records, proposals and their hunk decisions, storage
accounting and background jobs.

The object store holds immutable blobs: collaboration bases, source archives,
document assets and published files, keyed by content digest or by a name that
is never reused. Nothing in it is modified in place. A new state is a new key,
and an old key is reclaimed only once nothing points at it.

### The live document

A document's collaborative state is persisted as one compressed base plus an
ordered log of updates in Postgres, compacted periodically.

The base is the document's full operation history, exported as updates from an
empty version vector and compressed with zstd. Character-by-character typing
run-length-encodes almost perfectly, so a million keystrokes of history costs a
few hundred kilobytes.

Because the base carries the whole operation graph, there is no retention window
on edit history, no thinning pass, and no point past which blame is lost. A
document whose history exceeds the deployment's ceiling is refused at admission.

### Checkpoints

Live saving and the version timeline are separate mechanisms. Saving is
continuous. A checkpoint is a distinct event that records the whole document
directory as a canonical tree: every file, its path and its contents, plus the
compile settings in force.

Nothing schedules a checkpoint. One is written when somebody labels a moment,
publishes, restores an earlier version, leaves a comment on a passage, or
commits from the command line -- and at no other time. The clock does not
write history, because the operation graph above already holds every keystroke
between two checkpoints and can be read at any position in it. A checkpoint is
therefore a name somebody put on a moment, not a recovery point: recovery is
the operation log's job.

Recording the whole directory is what makes a checkpoint restorable. A chapter
and the file that includes it cannot come back out of step, because they were
captured together.

The manifest, meaning the list of checkpoints with their times, actors and
labels, is served to the browser as it stands. Checkpoints can be labelled, and
every checkpoint -- labelled or not -- is kept until the document is deleted.

A checkpoint's actor is who recorded the event, which is not necessarily who
wrote each changed passage. The API reports authorship as unknown rather than
naming the checkpointer.

### Source archives

Each checkpoint also has a format-independent source archive: the directory as
plain files. It is the fastest way to retrieve a document at a point in time,
and it is readable without the CRDT, which is what makes an export or a restore
independent of the collaboration format.

### Asset storage

Document assets are content-addressed. An asset key is the digest of its bytes,
nothing deletes from that table directly, and an orphan sweeper treats live keys
as live. Uploading the same figure twice stores it once, and two documents
referencing the same bytes share them.

An upload reserves its quota claim before the blob write begins and releases it
if the write is cancelled, so a slow upload cannot be double-counted and an
abandoned one cannot leak capacity.

### Quotas

Storage limits are set per deployment and enforced at several scopes:

| Flag | Caps |
| --- | --- |
| `--document-size-limit` | combined source text of one document |
| `--document-assets-limit` | combined input assets of one document |
| `--publisher-storage-limit` | everything one publisher holds |
| `--deployment-storage-limit` | the whole deployment |
| `--publisher-document-limit` | documents one publisher may hold |
| `--publisher-upload-limit` | uploads one publisher may make in an hour |

Owners can set a softer history budget, retention density and warning thresholds
of their own, but these cannot raise the deployment's hard limits. Material
reductions require a preview and confirmation and carry a grace period.

Documents may also expire. A retention duration counts from creation or last
bundle, and zero means never.

### Backups

`librepaper admin backup create` writes a recovery point covering both stores
together, a snapshot-consistent Postgres dump plus the immutable objects it
references, and verifies it. `restore` restores one into a fresh directory.

Backup contents are plain at rest. An operator holding backups holds the
documents.

### Background work

Maintenance runs as typed jobs in Postgres: compaction, orphan reclamation,
retention passes, and settling interrupted allocations. Reclamation releases a
charge only after physical deletion is confirmed. Superseded objects become
eligible only after a grace period during which no current root or lease
protects them. Reaching a byte quota does not prevent cleanup from running.

## Comments

A comment is about a passage of a document's source, at a checkpoint. Three
separate things are kept about it.

| | what it is | lifetime |
| --- | --- | --- |
| Original anchor | what the reviewer selected, in the document as it then stood | written once, never modified |
| Derived attachment | where that passage is now | a cache, recomputed on every edit |
| Presentation context | the words as the page had them | display evidence, never identity |

The original anchor is a checkpoint plus either a UTF-16 range of one file, named
by its stable id, or the document as a whole. Nothing writes to it after it is
created.

The derived attachment is keyed by the checkpoint it was computed against and is
discarded whenever the source moves. Nothing it contains is written back into
the anchor.

The presentation context is the rendered quotation. It is what a person reads to
understand a comment, and what a highlight is painted from. Nothing resolves
through it, so re-rendering a document cannot orphan a comment.

Every anchor is a range of words, because every annotation is made by selecting
some. There are no point annotations.

### Locating

A reader selects text on a rendered page, and the comment has to be about the
source. The renderers cannot bridge that, since they return a page and a list of
diagnostics and say nothing about where either came from. The server does the
crossing itself, once, when the comment is made.

The bridge is the prose. A heading is `# Title` in Markdown and `Title` on the
page, and a formula is neither, but the words between the markup are the same
words. Both sides are flattened, with syntax characters blanked and runs of
whitespace collapsed, and the selection is looked for in the flattened source.
Blanking preserves length and the collapse carries a map, so a position found in
the flattened copy is a position in the file itself.

It refuses rather than guesses. A passage that reads identically in several
places, with nothing around it to distinguish them, is not located. The result is
verified against the checkpoint and then frozen.

### Resolving

Where the passage has since moved is a separate question, asked afresh every
time the document is served.

A cursor captured when the comment was made follows the characters it sits
between through every subsequent insertion and deletion, and reports where they
went. A resolved cursor is not by itself proof that anything survived, because a
cursor whose content was deleted is deliberately relocated to the boundary it
occupied. So a resolved pair is checked against the quoted text before it is
believed, and a collapsed one is reported as deleted rather than as a position.
A cursor carries its container, so an anchor that resolves into a different file
than the one it names is rejected.

When there are no usable cursors, resolution falls back to searching for the
quoted words in their source context. Two equally good candidates produce
`ambiguous`, not the first of them.

Replacement cursors returned during resolution are stored into the cache and
never into the anchor.

A comment is therefore attached at a position, deleted, or ambiguous, and each
of those is shown as what it is.

### Marks

The highlight a reader sees is painted in the browser. Source identity and
current attachment come from the server, and the in-page search only decides
where to draw. A mark that cannot be placed does not move the comment.

### Export

Comment fields follow the W3C Web Annotation Data Model, so exporting is
reshaping rather than translation: `exact`, `prefix` and `suffix` are a
`TextQuoteSelector`, `motivation` uses the standard vocabulary, and `creator` and
`created` mean what the specification says. `resolved` is an extension, which the
model permits.

`librepaper export DOCUMENT` writes them out in Markdown or JSON.

## Track changes

A tracked change is a branch: a fork of the document at a known point,
collecting ordinary edits.

```
main:  --*--*--*--*--*--------------*  merge
           \--o--o--o--o----------/
              branch: ordinary edits
```

Everything that means "a change awaiting a decision" is this one object,
differing only in who opened it and how it is presented:

- typing with **Track changes** on, usually one hunk
- a suggestion from a comment thread, one hunk
- an agent run, with many

All of them appear in the **Changes** pane, where they are reviewed. The stored
object is called a proposal in the database and the protocol.

A branch is stored as a blob of operations alongside a row in Postgres. It is
not a room, holds no sockets, and nothing about it lives inside the shared
document. Review state, meaning its id, base and tip, author, status, decider,
decision time and comment, is written only by the server.

### Hunks

Review is the text difference between where the branch forked and where it has
reached. That arrives as a flat run of retain, insert and delete operations with
no notion of which belong together.

A *hunk* is a maximal run of non-retain operations. Replacing "cat" with
"tabby" is one decision, not a delete and an insert.

The raw difference is character-level: that replacement is several tiny edits
with a letter or two retained between them, because that is the shortest path
from one to the other. Nobody reviews at that grain, so two changes separated by
eight or fewer retained units are treated as one hunk, roughly a word.

A decision names a hunk by its index, so the browser and the server must group
identically or declining the second hunk reverts something else. Both sides carry
the constant, and neither may change it alone.

### Deciding

Accepting a whole branch imports it. Rejecting one drops it.

Accepting part of one is a merge followed by a revert:

1. Import the whole branch. Every one of the author's operations, with their
   authorship, enters the graph.
2. Compute the inverse difference, which would undo the branch entirely.
3. Keep only the rejected hunks of it. An accepted hunk collapses to a retain,
   because that text stays.
4. Apply that as the reviewer.

The result is exactly the accepted subset, with the accepted text attributed to
its author and the removal attributed to the reviewer.

Applying text re-authors it as the applying peer, so this ordering is what keeps
a reviewer from appearing to have written the prose they approved.

Steps 1 to 4 are one atomic server operation. The intermediate state contains the
rejected text and is never relayed to a peer or persisted as a version.

### Naming a hunk

The server counts text in code points and the browser counts UTF-16, so the two
disagree about offsets. No difference information crosses the network in either
direction. Each side computes what it needs from the document state it already
has, and a decision names a hunk by branch, base, tip and index: four
identifiers, no offsets.

### Contention

Two branches touching the same passage are two answers to the same question. The
browser already holds every open branch's bytes, so it detects overlap itself:
changes to the same words in the same file are grouped into one card with one
choice. Adjacent edits are not grouped, since two changes that merely touch at
the ends can both happen. Stale rows are excluded, because their extent
describes text that is no longer present.

### Speculative reading

Because a branch is a fork rather than an overlay, any subset of open changes
can be merged into a scratch document and read as finished prose. The browser
forks the room, imports the chosen subset and reads the result. Nothing is
persisted and nothing syncs to it.

### Blame

Attribution survives a partial accept, and every intermediate state stays
reachable in the operation history. After a chain of changes, each partly
accepted, the question of who wrote the sentence now in the paper has an answer.

## Publishing and rendering

### The engines

Markdown, Typst and LaTeX are compiled by WebAssembly modules that run in the
browser. They are plain WebAssembly with a small set of exports rather than
generated bindings, so the glue is short: reserve memory in the module, write the
source into it as UTF-8, call compile, read the page back out.

Each module's URL carries a digest of its bytes, so a module cached for a year
cannot outlive the loader that talks to it. The same pinned releases back the
native command-line tools, so a document renders the same way in both.

The LaTeX engines and their pinned TeX Live package set are not served by the
deployment. A browser fetches them from an HTTPS mirror, as verified
content-addressed bundles that stay in browser storage, so the next document
costs nothing to fetch. That mirror therefore sees a browser's IP address and
which digest-named files it asks for, which can suggest a document's field or
template. It never receives document source or input assets. An operator can
host a copy and pass `--latex-mirror URL` to keep those requests on their own
infrastructure.

Compilation runs in a worker, one per module, so warming a large Typst module
cannot delay a Markdown preview. Packages and fonts fetched by an earlier compile
are kept for the life of that worker.

Markdown and Quarto produce flow HTML. LaTeX and Typst produce a paged PDF.
Rendering stays in the browser or, for the two cases it cannot cover, in a
local companion. The deployment never renders and never stores a compiler.

The editor keeps the last successfully rendered page and paints it while the
engine warms, so opening a document does not start with a blank pane.

Quarto is handled as a browser-side subset. A `.qmd` is always kept as source,
and a short-lived draft representation is built for the Markdown renderer,
recording enough structure to map source annotations without guessing from
rendered paragraphs. It does not execute code, filters, shortcodes or JavaScript.

### Publishing

Readers do not load a document's source. They load a *bundle*, a display
bundle an editor rendered and explicitly published. Source saves, checkpoints,
previews and reconnects never upload one. Before the first bundle, a
document's bundle metadata is null.

A bundle stores its rendering and nothing else. The rendered page is the one
artifact the server cannot reproduce, since the renderers run in the author's
browser. Figures are not copied into it, because document assets are already
content-addressed and the bundle references them.

Only dependencies actually referenced by the rendered page become public. Inputs
used by the compiler, such as bibliographies, data files, maps and source, are
deliberately absent from the display allowlist.

### Publish sequence

Publishing is three operations under one idempotency key, so a retry after a lost
response cannot produce a second bundle.

1. `prepare` sends a manifest and receives the list of objects the server is
   missing. It atomically reserves the whole bundle's capacity before any upload
   begins.
2. `upload` sends each missing object. Limits and hashes are checked against
   decoded bytes.
3. `activate` re-sends the same body and key, and commits the bundle's roots,
   pointer and receipt together after rechecking access and the source and
   bundle generations.

The manifest names a bundle digest, the source and render-config digests, the
HTML, and each asset by path, digest, size and type. The server supplies the
bundle identity, timestamp and attribution. A conflicting expected
bundle is a 409, and a changed descriptor cannot reuse a prepared key.
Successful retries reuse the original attribution and timestamp.

Unfinished preparations expire after fifteen minutes. Superseded objects become
reclaimable after a grace period once no root or lease protects them.

### Serving

Published HTML and its assets are served only on the document origin, never the
one the application answers on. A signed display capability conveys no source
credential, and the reader's live link or account authority is rechecked on every
response, including conditional asset requests.

Assets use stable document-scoped paths with private revalidation, so unchanged
assets reuse browser cache bytes across bundles. Bytes already downloaded
cannot be revoked retroactively.

Every published document renders under one policy: it may run its own code and
may not fetch code from another host. Publishing reports the hosts of a document
that tries to. Images, fonts and connections still reach the open web, which is
an accepted leak rather than an oversight.

## Live collaboration

### The room

A room is one document's comments and the open sockets of everyone reading it,
held in one process. A write by one reader reaches the others without anybody
polling.

The socket carries document updates, presence, comments and bundle notices.
Presence, meaning who is here and where their cursor is, travels as ephemeral
state that is never persisted.

Editors synchronise source through the socket. Readers of a published document
use the same connection for annotations, without any source state at all.

The source is edited in CodeMirror 6, bound to the shared document, so each
editor keeps their own undo history and sees the others' carets where they
actually are. CodeMirror loads when the editor opens.

### Losing the socket

A dropped socket loses nothing. Comments post over an ordinary HTTP route, and
the opening frame on reconnect resends the full list, so a broadcast missed
during a gap heals itself. What a drop costs is seeing other people's comments as
they arrive.

A silent disconnection is handled separately. A NAT mapping expires or a laptop
sleeps, the socket is gone without a close frame, `readyState` stays open, and
the page looks connected while receiving nothing. So the connection is polled
periodically about whether it is still alive rather than trusted to report its
own death.

Both queue depth and transport writes are bounded on the server, so a slow peer
cannot pin the reader or block the connection from being torn down.
Authorisation is rechecked while a socket is open, not only when it is
established.

### Identity

Identity comes from a provider, GitHub or Google. A browser signs in through that
provider's OAuth flow and carries a signed cookie afterwards. A terminal signs in
through the deployment's own device flow and carries the resulting token as a
bearer credential. Both end as a handle, which deployment policy either allows or
not, and an id, which everything else keys on.

Cookies use `__Host-` naming wherever the deployment is HTTPS, the OAuth flow
uses PKCE and state cookies, and logout is POST-only. Capabilities are stored
only as digests.

### Share links

Access to a document comes from a grant on an account or from a share link. A
link names a role, reader, commenter or editor, and possession of the link is the
grant.

Links carry an expiry by default, because a round of review has an end and a link
that lives forever can be forwarded indefinitely. The share dialog offers
renewal, which mints a new key. A revoke and a mint in one request is a rotation.

A key never reaches a log. What is recorded anywhere is its hash. Browsers
present it on a header, except on the socket, which carries it as a query
parameter because a browser cannot set a header there.

Ownership can be transferred outright, which moves every authority to the new
owner.

### Working offline

An editor can make a project available offline while connected. LibrePaper then
stores the application resources, editor modules, project identity and metadata
locally, and the project can be opened from an offline start page after closing
every tab and restarting the browser.

Local typing never waits for the network. While offline, source edits, file
creation and renames, choosing the main file, reading cached assets and
previewing with available dependencies all work locally. Adding asset bytes needs
a connection. Comments are kept as local drafts and submitted explicitly after
reconnecting. Operations requiring current server authority, such as deciding a
proposal, restoring, publishing or changing sharing, are not queued for delayed
execution.

Local persistence and remote durability are separate states, and the
interface reports them separately: saving on this device, saved on this device
but not yet synced, or synced. A connected socket, a successful send, or an empty
queue after a restart is not evidence of durability, and conservative status is
preferred to false success.

On reconnection, document identity and current authority are checked before local
changes are sent. A rejection preserves recoverable local work rather than
discarding it, and a reconciliation that might remove locally edited content
preserves a recoverable version first.

Browser storage is not a backup. Eviction, clearing site data, or a lost profile
or device can remove unsynchronised work. The application says so when offline
availability is enabled, and offers ordinary source-and-asset export.

Local caches are isolated by account and access context, so switching accounts
cannot expose another account's cache, and multiple tabs cannot report each
other's pending work as remotely durable.

## Companion app

`librepaper local start` runs a loopback service that executes tools installed
on the author's machine, and the same binary is what an agent runs in to edit a
document. Discovery resolves local tools to explicit absolute paths.

### What runs

Two things, and both are cases the browser cannot cover.

Quarto executes code, so it is never run in the browser: the browser treats
a `.qmd` as a subset that will not run code, filters, shortcodes or JavaScript.
A document with computations is rendered here.

Calepin produces a self-contained HTML page from a Typst document, with its
images embedded. It wraps `typst`, and it is the only Typst path that leaves
the browser.

The companion also reaches Zotero, read-only, through that application's own
loopback API.

Everything else compiles in the browser and never reaches the companion. Typst
produces its PDF there. LaTeX and Biber are WebAssembly there, and no TeX or
Biber job is accepted here at all.

### The protocol

A versioned protocol over loopback HTTP, with bounded limits on body size,
upload size, file count, PDF size, log size and job deadline. The health
endpoint returns enough to identify the service and negotiate a version, and
nothing else: no tool paths, no projects, no jobs.

A request whose `Host` header does not match is answered before anything else
is parsed, since that is what DNS rebinding looks like.

### Pairing

A browser pairs with the service per origin and project. Pairings live in a
small file under the user's state directory, keyed by origin and project. The
token itself is never written to disk, only its hash, so reading that file does
not hand out access. Tokens expire.

### Detection

A browser cannot enumerate installed applications or prove one is absent. The
client makes one bounded probe of one documented endpoint, remembers the
outcome for a while, and lets the caller decide what to try next.

### Presets

Presets are owned by the companion. Only an opaque id and descriptive metadata
cross the bridge to the browser; executable paths, argument vectors and
environment values stay on the machine. A preset may not set loader or
interpreter environment variables, refused at validation time.

### Agent edits

An agent edits a document through the same room and the same durability rules
as a person, over MCP to the server rather than over the loopback protocol
above. MCP owns the transport, and the room owns effects and durability.

The patch language is independent of MCP. A caller supplies a source tree
identity and immutable byte ranges, and the validator produces a new source
without guessing an anchor: a patch whose ranges do not match the tree it names
is refused rather than applied approximately. A validated batch is applied
across every text file in one commit.

The operation is persisted before the document changes, and the receipt is
committed only after the resulting state is durable. A caller that loses its
response can therefore ask what happened rather than repeating the edit.

An agent's work arrives as a [tracked change](#track-changes), a branch
with as many hunks as it made, so it is reviewed and decided like a human
suggestion. Nothing it writes enters the document without a decision.

An agent's authority is the link it was given. A caller's own session never
widens it.

Highlights an agent paints inside a document are drawn on the document origin,
outside the application's stylesheet, because that frame is a different origin
by design.

### Assistant channel

The private assistant channel is a short-lived rendezvous between one browser
and one local runner. The server relays bounded events while both sockets are
connected. It is not a queue and not a transcript store. Task queues and
reconnect reconciliation belong to the runner on the user's computer.

### Prompt injection

A document is written by whoever has editor access, and an agent reading one is
reading untrusted text. Nothing in the system bounds this. An agent operating
on a shared document should be treated as acting on the instructions of
everyone who can write to it.

### Running others' code

The companion executes code authored by anyone with editor access to the
document being built. Quarto runs a document's code by design, which is the
point of sending it here. Running the companion against a shared document
therefore means running collaborators' code on your machine.

## The interface

The pages are [Skeleton](https://skeleton.dev) on Tailwind 4. Skeleton supplies
the furniture, meaning buttons, cards, inputs, tables, dialogs, tooltips and
toasts, and `web/src/styles/theme.css` colours all of it from LibrePaper's own
four colours, so the palette is written down once.

Three rules keep a growing application looking like one application, and
`make test` enforces them:

1. A colour or a size comes from the theme. A hex value or an arbitrary
   Tailwind size in a component is a decision made twice.
2. A control is a component. There is one `IconButton`, so there cannot be a
   fourth kind of button that is almost like the other three.
3. Layout comes from `Page`, `Stack` and `Row`, so a new screen is assembled
   rather than measured.

Two things sit outside that system deliberately. An agent paints its highlights
inside a document on the document origin, where none of this stylesheet reaches
it. And the colours identifying people in a shared editing session travel over
the wire to other browsers, so they cannot be a local theme decision.

## Building from source

The binary embeds the web build and the pinned browser renderers.

| | What it is | Built by |
| --- | --- | --- |
| `web/` | the pages: Svelte, Skeleton, CodeMirror 6, Loro | bun and vite |
| `crates/librepaper/` | the server and the command line | cargo |

Renderer implementations live in the `wasm-*` repositories. Cargo links their
pinned native libraries, and the browser uses WebAssembly artifacts from the
same release tags. The web build writes into `web/dist`, which the binary
embeds; nothing under that directory is edited by hand.

```sh
make web      # the pages, from web/
make wasm     # the pinned browser renderers
make build    # dist/librepaper, with the pages and renderers embedded
make install  # build and install to ~/.local/bin (override PREFIX= or BINDIR=)
make test     # rustfmt, clippy and the test suite
make test-external          # Quarto/R/Python and local-service integrations
make test-release-workloads # supported limits and diagnostic workloads
```

`make build` needs [bun](https://bun.sh) and Node.js. The browser renderers are
fetched from the exact tags and SHA256 digests in `wasm-modules.lock`;
`make wasm-check` verifies that those tags also match the native renderer
dependencies without network access. To update one renderer, name both values
explicitly, for example `make wasm-update REPO=wasm-markdown TAG=v0.2.0`, then
review the resulting Cargo and lockfile diff.

### Running it locally

`make deploy` runs the application locally. Configure an OAuth app in `.env`
(see `.env.example`), then sign in: every new account receives five private,
editable examples, one each in HTML, Markdown, Typst, LaTeX and Quarto. Use
`PUBLISHERS=any COMMENTERS=anyone` for local development.

Examples are created once per new account, survive restarts, and stay deleted
if you remove them. Existing accounts are left unchanged, and `make deploy`
never resets or seeds the shared catalogue. The five starter sources ship
inside the binary, so signing in needs neither Quarto nor a checkout of this
repository.
