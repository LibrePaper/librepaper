# SPEC: Store published HTML

## Purpose

Readers and commenters must be able to read and annotate a publication without
receiving its editable project. Keep compilation on the publisher's device,
store the latest explicitly published HTML on the server, and serve that
publication to readers. Minimize storage and transfer by separating publishing
from live editing and saving.

This specification replaces the current assumption that every reader receives
the source tree and renders it locally. Hiding the source pane or Download
project button is insufficient: confidentiality must be enforced by the server.

## Product behavior

- Owners and editors retain source editing, live source collaboration, local
  previews, and project downloads, including for raw HTML projects.
- Readers and commenters receive only the current published HTML, its public
  assets, and permitted discussion metadata. Commenters select passages in the
  rendered document to leave comments.
- Saving, typing, checkpointing, reconnecting, and compiling a local preview
  never automatically upload a publication. There is no debounce timer or
  periodic publication job.
- Publish and Publish update are explicit actions. Edits remain saved even
  when they have not been published; readers continue seeing the previous
  publication until its replacement succeeds.
- Before the first publication, a reader sees that no version has been
  published yet. Never fall back to delivering source for local rendering.
- The initial publish/upload workflow may create the first publication as
  part of that explicit action. Uploading raw HTML can use that HTML directly;
  uploading source requires a successful local HTML render before a reader
  publication becomes available.

## Publishing controls

Put a Published version section at the top of Share, containing:

- The last successful publication time and publisher, or Not published yet.
- An Unpublished changes status when current source differs from the source
  snapshot used for the publication.
- Publish before the first publication, Publish update when changes exist,
  and Published with a disabled action when the source snapshot matches.
- A short explanation: Readers and commenters see the last published version.
- Progress while preparing or uploading, and an actionable error on failure.

Provide a small clickable Unpublished changes indicator in the main toolbar
that opens this section. The primary publication button lives in Share.

Editors must be able to open the publication section. Link creation, rotation,
revocation, and other sharing administration remain owner-only, enforced by the
server. Opening Share must not grant editors access to owner-only controls or
link secrets. Readers and commenters have no publication controls.

An edit during publication leaves the new publication tied to its captured
snapshot and leaves Unpublished changes visible afterwards. A successful upload
must never mark later edits as published.

## Source confidentiality

Enforce authorization on every route and transport, using the effective role
of the presented session/link. An owner account opening a reader link must not
silently widen that link's access where links already act as a role ceiling.

| Resource or operation | Reader | Commenter | Editor / owner |
| --- | --- | --- | --- |
| Current published HTML and public assets | Yes | Yes | Yes |
| Permitted rendered comments and highlights | Read | Read and annotate | Read and annotate |
| Source files, full tree, project download | No | No | Yes |
| Source history, source diffs, source-bearing snapshots | No | No | Yes |
| Source synchronization state and updates | No | No | Yes |
| Prepare/upload/activate a publication | No | No | Yes |
| Administer access links | No | No | Owner only |

Audit HTTP source, snapshot, history, file, and asset endpoints; room state
references and WebSocket synchronization; MCP and assistant tools; diagnostics;
and metadata containing paths, text, or embedded source. Reader sessions must
not receive a full Yjs document, even if updates from those sessions are refused.
Separate the reader annotation/publication channel from source synchronization.
Source anchors and editorial suggestions must not leak private source through
otherwise readable annotation responses.

Authorize asset reads through the current publication's manifest. Knowing a
private asset hash or guessing a project path must not grant access. A source
asset may be public only when the publication explicitly includes it. Preserve
document access checks on publication responses and cached asset delivery.

HTML, visible text, images, CSS, JavaScript, fonts, and data embedded in a
publication are downloadable by its readers. Raw HTML itself cannot be secret
while being displayed. The protected material is the editable project and
anything excluded from the publication. Source maps, hidden source payloads,
project archives, and private inputs must not be added implicitly.

## Publication representation

Keep a small current-publication record with:

- A publication identifier and a digest of the complete display bundle.
- The digest of the captured source tree and the rendering configuration used.
- Publication time and authenticated publisher attribution.
- The HTML object hash, byte sizes, and a manifest of public assets with their
  hashes, MIME types, and public relative paths.

Capture a consistent source snapshot before rendering. Generate the HTML and
collect display dependencies from that snapshot, not from a moving live tree.
The publication builder must use a display-asset allowlist and must not export
the project directory wholesale. Rewrite asset references into the publication
namespace. Bundle required display assets instead of relying on private source
URLs; report missing dependencies before activation.

Serve authored scripts in the existing isolated document origin and preserve
the applicable sandbox, CSP, and message-origin protections. Published scripts
must not acquire source API credentials. Confidentiality must not depend on
whether a publication's JavaScript behaves honestly.

The server validates authority, manifest membership, paths, MIME handling,
digests, and resource limits. It does not compile the source or claim to verify
that a publisher's HTML faithfully represents it. An authorized publisher is
responsible for the content they publish.

## Upload and activation

1. Capture the source snapshot and render locally following an explicit action.
2. Hash the HTML and display assets. Ask the server which objects are missing
   from this project's authorized publication storage.
3. Upload only missing objects into a bounded staging area. Compress HTML and
   other compressible payloads during transfer; validate decoded sizes and
   hashes. Do not recompress images that gain nothing from it.
4. Recheck publication authority and validate that every manifest object exists.
5. Atomically switch the current-publication pointer to the complete bundle.
6. Reclaim superseded objects once no retained publication or active staging
   operation references them, subject to a short bounded cleanup grace period.

Use an idempotency key for retries and an expected current-publication identifier
for activation. Concurrent publishers must not silently overwrite one another;
a stale activation reports a conflict and requires an explicit retry decision.
Changing the live source while a publication is prepared is allowed and differs
from a conflicting publication activation.

If display bytes are unchanged, reuse the existing bundle without uploading it.
After successful publication, record the newly captured source identity even
when its rendered output is identical, so unrelated source changes do not leave
a permanent Unpublished changes indicator.

Failed renders, failed uploads, quota refusals, revoked permissions, and server
restarts leave the previous complete publication available. Expire abandoned
staging objects. Activation and garbage collection must remain correct across
crashes and retries.

## Storage and network costs

- Retain one current display bundle per document. Do not create a permanent
  rendering archive for source checkpoints, comments, or each publication.
- Deduplicate unchanged assets and HTML by content hash within a project.
  Cross-project deduplication is outside the initial scope; avoid a global
  hash-existence API that reveals other projects' contents.
- Count publication objects and staging against explicit storage limits, with
  bounded temporary headroom for atomic replacement. Limit compressed and
  decoded sizes, asset counts, concurrent uploads, and staging lifetime.
- Use compressed HTML delivery and content hashes for cache validation. Cache
  immutable asset bytes only behind document authorization; hash URLs are not
  bearer permissions. For private documents, prefer private browser caching
  and conditional revalidation. Any shared cache must authorize each request.
- On reader entry or explicit Refresh, resolve the current publication through
  a small metadata request. Do not poll or automatically download new bundles.
- An already open annotation channel may announce a new publication identifier.
  Show New published version available with a Refresh action; the announcement
  must not trigger a full document fetch or interrupt a comment draft.
- Reuse cached assets between publications and load below-the-fold images lazily
  where this preserves document behavior.
- Start with complete compressed HTML uploads per explicit publication. HTML
  delta protocols are outside the initial scope.

Cache policy cannot revoke bytes a reader has already downloaded. Revoking
access must prevent new authorized responses and must not be defeated by a
shared cache serving private objects without checking access.

## Comments across publications

Each rendered annotation records its publication identifier, selected quotation,
surrounding text, and appropriate rendered positions. Readers do not need source
anchors to create or navigate these annotations. Editors may maintain private
source mappings separately when reliable.

When a new publication becomes current, re-anchor by rendered content. Preserve
the original quotation and publication identifier; do not silently attach an
ambiguous match. Mark missing or ambiguous passages as referring to an earlier
publication and retain their discussion.

Keep only the latest full rendering. Older comments therefore retain their
quoted context but do not promise access to an old page, layout, or figure.
Annotation identifiers must not pin old display bundles in storage indefinitely.
If an open reader is on an obsolete publication, preserve their draft and ask
them to refresh before submitting a new annotation; server enforcement must
handle a publication changing between selection and submission.

## Implementation and scope

LibrePaper is unreleased. Implement this as a clean replacement: no data
migrations, backfills, compatibility shims, legacy protocol support, or parallel
implementations of the old reader behavior.

Remove obsolete reader-side source rendering, source synchronization, and
project-download paths, along with their unused helpers, configuration, and
tests. Retain source rendering and synchronization where editors need them.
Update affected callers, fixtures, tests, and documentation to the new contract;
do not leave dead code or fallback branches for the previous architecture.

Source history remains available to editors and owners under existing retention
rules. Storing one rendered bundle does not change source-history retention.

This spec covers HTML publications. A project whose renderer cannot produce
HTML must report that publication is unavailable; it must never fall back to
source delivery. Stored PDF publication can follow the same authority and
activation model in a separate extension.

## Acceptance tests

1. For reader and commenter links, request source, full snapshots, history
   source, private assets, room state references, and source WebSocket state
   directly. All must refuse without returning source bytes or private paths.
   Cover MCP/assistant equivalents and a signed-in owner using a restricted
   link under the existing link-ceiling rules.
2. A private fixture contains source text, an unused data file, a bibliography,
   a private image, and a source map. None appears in reader network responses,
   HTML, manifests, comments, or error payloads. Guessing their paths or hashes
   does not retrieve them. Explicit public display assets remain readable.
3. Readers and commenters cannot create staging uploads, activate a publication,
   or download a project. Editors and owners can publish and download source;
   editors cannot administer owner-only sharing settings.
4. Readers can render and commenters can select, submit, reload, and navigate
   annotations without any source-bearing connection or request.
5. Repeated typing, autosaves, local compilation, checkpoints, and reconnects
   cause zero publication uploads. Explicit publication uploads only missing
   objects; identical output reuses all bytes and records the source identity.
6. Changed HTML with unchanged images uploads no image bytes. A warm reader
   refresh reuses unchanged assets under the chosen authenticated cache policy.
7. Failed or interrupted publication preserves the old bundle. Concurrent
   activation, retries, revocation during upload, and crash recovery never
   expose a partial publication or overwrite a newer one silently.
8. An edit during publication leaves Unpublished changes visible. Readers stay
   on their current version until refresh, and comment drafts survive notices.
9. New publications re-anchor unique quotations and preserve unmatched comments
   without retaining the old full rendering. Stale annotation submission gives
   a recoverable response without losing the draft.
10. Replacement and abandoned-upload cleanup keep storage bounded; shared
    objects survive while referenced and become reclaimable afterwards.

Measure stored bytes, uploaded bytes, downloaded bytes, metadata requests, and
cache reuse separately. Use a fixture with large unchanged images and a small
text edit to demonstrate the intended cost behavior before considering deltas
or broader deduplication.
