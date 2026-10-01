# LibrePaper security and privacy

*Updated 2026-10-01 after the server-as-log cutover. This is a security
contract and decision backlog, not a fresh audit. Observations and proposals
are distinguished; proposed controls are not implemented guarantees.*

## Scope and contracts

The system uses one Rust server, PostgreSQL and blob storage, browser-side
compilation, batched source persistence and evictable CRDT state. Readers follow
live source projections; there is no published rendered-bundle system. Local
builds and agent interfaces remain supported. Source paths below are relative
to `crates/librepaper/src/` unless stated otherwise.

- The operator controls binary, database and storage and can read plaintext;
  there is no end-to-end encryption. Backup encryption protects copied backups.
- Sharing links are bearer credentials with role, expiry and revocation.
  Forwarding forwards authority. Local code execution requires separate consent.
- Documents and agent context are hostile input. HTML runs in the document
  origin; companion builds can execute code locally. Editing does not grant
  execution. Agents may act through unrelated tools too.
- Preserve separate application/document hosts, authorization at durable
  semantic-command boundaries, prompt revocation, honest durability signals,
  and recovery of unsynced work.

## Rendering and outbound requests

- **Observed:** `server/origins.rs` requires different application and document
  hosts. The document host serves the empty preview shell, PDF viewer, frame
  agent and shell assets; API routes refuse it. `serve_shell` and `serve_viewer`
  set `frame-ancestors` to the configured reader origin, and their CSP permits
  scripts from `https:` for CDN compatibility. `Preview.svelte` accepts frame
  messages only from its iframe window and configured document origin. The
  frame agent checks messages from its parent window and configured reader
  origin. The PDF viewer applies the same reader-origin check before accepting
  messages or choosing a reply target. The reader-side receiver validates
  message types, fields, size bounds and frequency. The iframe enables scripts,
  same-origin, popups and forms.
- **Observed:** The document CSP currently permits outbound HTTPS scripts,
  styles, images, fonts and connections. Remote scripts can change after a
  document revision; external resources can identify readers. The host boundary
  keeps reader cookies inaccessible to scripts in the document origin, but
  same-site requests to the reader can still carry those cookies, so origin
  checks remain essential. No CORS allow-origin header is emitted by the main
  API, so browser scripts on the document host cannot read its responses. The
  host boundary is not a network privacy boundary. The old published-bundle
  claims do not apply.
- **Recommend (not implemented):** Preserve CDN compatibility as the default
  and offer an opt-in private/offline policy that blocks remote code and, if
  its promise is network privacy, all remote resource and connection loads.
  State which document formats need network access and what the mode blocks.
  Keep frame message checks tied to the configured reader origin and exercise
  actual response headers and message validation. Align
  [privacy docs](../../privacy.md).
- Images/fonts also expose requests. LaTeX compiler/packages use the configured
  mirror (default project mirror); digest checks give integrity, not privacy or
  availability. See [hosting privacy notes](../../host.md#privacy).
- **Limit:** Browser iframe sandboxing and render budgets reduce exposure and
  accidental resource use; they are not a hard CPU/memory boundary against a
  hostile browser document. Do not claim server-side resource limits protect a
  reader's browser.

## App origin and uploaded files

- **Observed:** App pages (`server/reply.rs`, `SHELL_POLICY`) are served with
  `script-src 'self' 'wasm-unsafe-eval'; worker-src 'self'; object-src 'self'
  blob:; base-uri 'none'; frame-ancestors 'none'`. No inline script, no `eval`,
  no `blob:` or remote scripts. The reader page receives its renderer URLs as a
  JSON data block, not an inline script.
- **Observed:** The TeX engines and Biber run in
  `web/src/lib/latex/engine-host.js`, a worker loaded from the app origin. A
  same-origin worker takes its CSP from its own response, so the engines may
  `eval` (LaTeXML calls `emscripten_run_script_string`) and load their
  digest-verified blob scripts there while the page allows neither. A worker
  built from a `blob:` URL would inherit the page's policy instead.
- **Observed:** Figures reach the reader and the document frame as `data:` URLs
  (`web/src/lib/figures.js`); only PDFs stay app-origin `blob:` URLs, for the
  reader's `<object>` view, where browsers render them in an isolated PDF viewer.
  Previously every figure was an app-origin blob: the document frame could not
  load it (preview figures were broken), and an SVG figure opened in its own tab
  ran its script as the app. A page opened from a `blob:` URL inherits the CSP
  of whoever started the navigation, so a URL pasted into the address bar
  inherits none; a `data:` document gets an opaque origin instead.
- **Observed:** Stored bytes (`server/cost.rs`, `blob_response`: figures, version
  archives, fonts) are served as `application/octet-stream` with
  `content-disposition: attachment` and `content-security-policy: sandbox`. Every
  response carries `x-content-type-options: nosniff` (a layer in
  `server/mod.rs`). This is what keeps `script-src 'self'` meaningful: without
  nosniff, an uploaded file served as octet-stream runs as a classic script
  when an SVG or page points a `<script>` at it.
- **Decision:** No allowlist of upload file types. Extensions do not prove
  content, HTML documents already run scripts by design on the document origin,
  and the risk of an active type is where it runs, not that it is stored. Active
  types are confined by origin, CSP and response headers; abuse (phishing,
  malware hosting, storage) is handled by the quotas, reporting and moderation
  in the anti-abuse section.
- **Recommend:** Keep these invariants under test: no inline script or `eval` on
  app pages; no app-origin `blob:` URL of an active type (SVG, HTML, XML);
  nosniff on every response; no same-origin route that returns user bytes with a
  script or stylesheet MIME type.

## Local execution consent

- **Observed:** Companion folder bindings and isolated workspaces support local
  builds with unshared inputs; Quarto can execute R/Python. Bindings carry
  `execution_granted`; pairing and execution permission are distinct. Custom
  Quarto/Calepin binaries and args require native confirmation showing the full
  command via `PUT /integrations/{name}`.
- **Recommend:** Define whether consent approves a captured revision or grants
  continuing project trust. Disclose collaborator changes; define expiry and
  invalidation; cover actual inputs and build config. Show/revoke execution
  permissions separately from pairing and preserve existing execution gates
  and frozen-cache checks. Do not imply local runners keep context local when
  remote model providers are involved.

## Sharing, privacy and agents

- **Observed:** `LINK_DEFAULT_SECONDS` is seven days; UI offers 7 days, 30 days,
  6 months or Never. Expiry updates do not replace the key; a settings change or
  rotation without an explicit expiry keeps the current one. Rotation is
  separate.
  Readers/commenters use annotation sockets, not source collaboration
  (`web/src/lib/reader/collaboration.js`, `web/src/components/Reader.svelte`);
  `doc-presence` is editor-only (`server/socket.rs`). Operator sees requests;
  resources may contact third parties and comments identify authors.
- **Recommend:** Keep link default and renewal behavior; retain role, expiry and
  prompt revocation. Do not promise anonymous reading. Any presence control must
  say whether it hides identity, caret or counts.
- **Agents:** `server/mod.rs::resolved_role` bounds automation by link role;
  `room/agent.rs::AgentPatchCommand::persist` records source patch label,
  reason and author; explicit checkpoints exist. Check attribution across
  comments, replies, suggestions and decisions, extending existing metadata.
  Attribution aids review/recovery, not prompt-injection prevention. Keep agent
  authority bounded and transactional.

## Anti-abuse admission and enforcement (proposed)

- **Observed inputs:** Main defaults are HTML/HTM, Markdown, Quarto, Typst and
  TeX. Common assets include PNG/JPEG/GIF/SVG/WebP/PDF and OTF/TTF/WOFF/WOFF2;
  project text also permits scripts/configuration such as R, Python, Julia, Lua
  and CSS. ZIP browser import has bounded upload/output limits. These are
  extension defaults, not proof of content type. Asset hashes identify bytes,
  not format or safety. HTML data URLs can encode arbitrary media bytes.
- **Keep useful content:** Preserve legitimate embedded figures and supported
  attachments. Separately cap source/asset bytes, file count, decoded pixels,
  archive entries/depth/expanded bytes, history/storage and per-account,
  per-document and deployment usage.
- **Treat active formats explicitly:** SVG/PDF may carry active content or
  expensive rendering; HTML can execute scripts, navigate, submit forms, load
  remote resources and embed base64 payloads. Sniffing/signature checks help
  route content but do not prove safety. Do not claim scanning makes arbitrary
  HTML safe. How the app origin confines them today is under App origin and
  uploaded files.
- **Constrain network access:** CDN scripts and external document resources are
  permitted by default for compatibility; disclose tracking, phishing and
  remote mutable code. An optional private/offline policy may block remote
  references. Source upload cannot certify future bytes at remote URLs.
  Scanners must never fetch arbitrary URLs: disable network or enforce strict
  egress and deny private/link-local/metadata destinations. Decode only bounded
  data URLs; do not follow redirects or nested URLs.
- **Bound work:** Apply parser/decoder/extraction/render time and memory budgets,
  archive expansion limits and concurrency caps. ZIP import rejects traversal,
  unsafe symlinks, colliding paths and expansion bombs. Browser-side budgets are
  best effort, not a hostile-code sandbox guarantee.
- **One admission boundary:** Cover create/upload, source edits, WebSocket
  updates, semantic and agent commands, asset writes and ZIP imports. Validate
  resulting document state as well as submitted payloads, including edits that
  add encoded media or remote refs. Handle concurrent quota races and retries.
- **Separate refusal classes:** Reject over-quota writes with a clear,
  non-retryable-until-corrected response. Return transient capacity failures as
  retryable, and never report buffered/refused writes durable. Preserve the
  author's unsynced source for recovery even when moderation is pending or
  ordinary readers are denied access.
- **Report and moderate:** Give readers a report action on a document or asset.
  Route reports to an operator moderation queue with minimal metadata and a
  review/audit trail. Provide operator actions to restrict visibility, disable
  publishing, restore access or remove a document; expose status and appeal
  route to its owner. Quarantine is a publication/access decision, not deletion
  of the author's recoverable source.
- **Incremental verdicts:** Start with deterministic structural limits, then
  optional bounded detection. Key verdicts by content digest and policy version;
  changed content or policy is reevaluated. A pending automated scan must not
  block an author's ability to recover/export unsynced source. Do not send
  private content to external scanners without explicit disclosure and a
  product decision; no technical verdict promises legal compliance.
- **Deny consistently:** Apply restrictions to live reads, assets, downloads,
  history, exports, sockets and agent APIs, including already-open sessions.
  Shared blob deletion must respect other references. Retain enforcement
  tombstones so backup restore cannot silently reinstate denied publication.
  Keep audit data minimal; do not retain payload copies without need.
- **Keep execution separate:** Admission does not consent to run R/Python/Quarto
  or other project code locally. Content scanning does not prevent prompt
  injection in external agents.

**Phases:**

1. Inventory accepted formats, write/read routes, current limits and report-only
   states; choose quotas and user-visible outcomes.
2. Enforce structural byte/count/expansion limits and work budgets across every
   write route; distinguish quota from transient-capacity errors.
3. Add reader reporting, operator queue/actions, owner status/appeal and
   consistent visibility enforcement while preserving source recovery.
4. Consider optional bounded detection after privacy, retention and policy
   choices. Decide supported active SVG/PDF/HTML features and defaults.

**Acceptance scenarios:**

- Supported embedded figures remain usable; encoded video follows media policy.
- Traversal and ZIP expansion bombs are rejected within budgets.
- Source edits and agent patches cannot bypass admission or quotas.
- Remote references follow the chosen block/pin policy.
- Readers can report abuse; operators can restrict documents and suspend publishers.
- Pending moderation preserves owner source recovery through an explicitly
  authorized path unavailable to ordinary readers.
- Denied content is unavailable through reads, assets, downloads, history,
  sockets, exports and agent APIs, except an explicit owner recovery path.
- Backup restore reapplies current enforcement tombstones before serving traffic.

## Backups, supply chain and availability

- **Backups observed:** `storage/backup.rs` writes a snapshot-consistent database
dump, referenced immutable objects and integrity manifest with private file
permissions. It is not encrypted; checksums do not give confidentiality or
authenticate a replaceable manifest. The [age recovery drill](../../../tools/frugal-recovery/README.md)
uses private plaintext staging and checks restored heads/history.
- **Recommend:** Document encrypted backup/restore, staging exposure, retention,
off-host copies and key custody. Preserve snapshot consistency and object
completeness; warn that restores can reintroduce deletions.
- **Supply chain:** Cargo Dist installers verify release checksums; workflow uses
ad-hoc macOS signing; SOPS has one PGP recipient; Wasm assets are digest-pinned
in `assets.lock`. Authenticate releases with an installer-trusted identity
independent of downloaded checksums; define key rotation/recovery and remaining
pipeline risks. Record rotation dates, not secret values.
- **Availability:** Per-document pending-update limits do not prove a global
bound. Measure pending/in-flight bytes, transient CRDT builds, caches and
serialization across documents; add aggregate limits where evidence warrants.
Audit comment/reply caps and rate settings rather than assuming enforcement.
Admission must cover browser/agent writes transactionally and keep older
over-limit documents readable. Established sockets refresh authorization through
`reauthorize_connection` with a one-second cache; cache changes need complete
invalidation and local expiry checks. Keep durable command checks transactional.
Preserve unsynced work and honest durability. Exercise reconnect bursts,
slowdowns and revocation on open sockets.

## Non-goals and order

No end-to-end encryption, operator-blind storage, safe-arbitrary-code guarantee,
forwarded-link prevention, anonymous-review guarantee, prevention of external
agent prompt injection, or distributed coordination before the single-writer
model changes.

1. Resolve production CSP and privacy claims; verify actual headers.
2. Define local execution consent and preserve current gates meanwhile.
3. Inventory/enforce anti-abuse limits and denial paths; add reporting and
   moderation controls while preserving owner source recovery.
4. Address measured aggregate resource gaps and revocation correctness.
5. Complete encrypted backup recovery, authenticated releases and agent
   attribution. This order is guidance, not an implemented guarantee.
