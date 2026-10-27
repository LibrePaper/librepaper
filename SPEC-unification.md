# Unifying the Rust workspace's core concepts

Status: proposed 2026-10-27, revised the same day after review

The workspace reimplements the same few concepts in every crate. Most of the copies agree, and the bugs sit where they do not. This spec gives each concept one type in one place, and deletes the copies.

It came from an eight-part read-only survey of `crates/` on 2026-10-27. Line numbers are from that day and will drift. Claims marked **confirmed** were checked by hand against the code. The others come from the survey and must be rechecked before a piece starts.

The first draft merged some things that only look alike: project admission with repository paths, every external offset with UTF-16, and three overlap rules into two. A review caught those, every point was checked against the code, and sections 3.2, 4.3, 5, 6 and 8.4 were rewritten. The lesson is the second principle below.

## 1. Principle

> One concept, one type, parsed once at the boundary.

> Unify the mechanism. Keep each distinct policy, and give it a name.

- A value that has rules (a path, a digest, a role, an offset) is a newtype with a fallible constructor. Code past the boundary cannot hold an unchecked one.
- Two checks that look alike but answer different questions stay two checks. They share the code underneath, and each has a name that says which question it answers. Before merging two rules, find out why they differ. If the difference is deliberate, keep it as a named policy, with a test example for each case where the policies part.
- A decision that has a table (error to status, role to capability, which bytes count against a quota) has exactly one `match` or one function.
- Meaning never travels as text. No branch reads an error message.
- The project is unreleased, so this is a hard cutover. Old helpers are deleted in the same piece that replaces them. No shims, aliases or feature flags.

## 2. Scope

### In scope

- Leaf types: `Digest`, `RelPath` with its three path policies, `LineEnding`, `Role`, `Token`, `Secret<T>`, `Bytes`. Most go in `librepaper-base`. `ProjectPath` goes in `librepaper-document`, which owns the admission rules.
- Typed errors and one refusal projection per transport.
- A text offset model: unit newtypes, `Span<U>`, `TextIndex`, `SourceLines`.
- An access guard for server handlers.
- Storage helpers for usage accounting, write transactions and keyset pages.
- Runtime helpers for background loops, keyed registries, retry and subprocesses.
- Shared request and response types between server, CLI, sync and companion.

### Out of scope

- Behaviour changes beyond the bugs listed in section 4.
- Changing which paths a project admits. The browser has its own copy of the admission rules (`web/src/lib/paths.js`) and of the projection (`web/src/lib/projection.js`). Both are held equal to the Rust side by shared fixtures (`web/tests/fixtures/projection.json`), and the projection digest drives etags and comment staleness. A Rust-only change to admission would fork the file tree. Section 3.2 keeps admission exactly as it is. A later change to admission must change Rust, the browser and the fixtures in one piece.
- The rest of the web app (`web/`). Wire changes that the browser reads keep their current JSON spelling unless section 4 says otherwise.
- Database schema changes. Section 9 notes the one place where a column type might change, and that is deferred.

## 3. Leaf types (do first)

These have no dependencies on the rest and unblock every later piece.

### 3.1 `Digest`

```rust
pub struct Digest([u8; 32]);
impl Digest {
    pub fn of(bytes: &[u8]) -> Self;
    pub fn of_reader(r: impl Read) -> io::Result<Self>;
    pub fn prefix(&self, hex_chars: usize) -> String; // for display ids only
}
// FromStr/Display: strict lowercase hex, exactly 64 chars. serde as hex.
// sqlx::Type as bytea where the column is bytea.
```

Replaces at least nine helpers: `document/results/mod.rs:89` `sha256`, `engine/storage/store.rs:1773` `digest_of`, `base/auth/device.rs:327` `digest_of`, `room/agent_query.rs:1634` `digest`, `sync/files.rs:25` `sha256_hex`, `document/projection/mod.rs:156-161` `digest`/`digest_bytes`, `companion/local/render/workspace.rs:935`, and about 30 inline `hex::encode(Sha256::digest(..))` sites. The three decode-then-`try_into::<[u8;32]>` blocks in `store.rs:1508,1551,1578`, `source_archive.rs:332` and `room/source_change.rs:706` `parse_digest` go away.

A digest that fails to parse is an error, never skipped. Today `store.rs:661` drops it with `filter_map(..ok())` and `store.rs:648` turns it into `None`.

Check while doing this: `sync/files.rs:12` says `FileRecord.sha` hashes the LF text. `projection/mod.rs:260` hashes `body.as_bytes()`. Confirm that bodies are always LF there, and write the answer into `Digest`'s doc comment for that field.

### 3.2 Relative paths: one structure, three policies

The path validators answer three different questions. The first draft merged them, which would have broken Git operations on `.gitignore`, `asset:` staging paths and the deployment's configured limits. They stay three policies over one structural type.

```rust
/// Structure only: relative, `/`-separated, no empty, `.` or `..` segment, no `\`,
/// no NUL or control characters, no leading `/`, no Windows drive prefix (`C:`).
/// Never trims and never normalises. Says nothing about what a project admits.
pub struct RelPath(String);
impl TryFrom<&str> for RelPath { type Error = PathRefusal; }
impl RelPath {
    pub fn segments(&self) -> impl Iterator<Item = &str>;
    pub fn under(&self, root: &Path) -> Result<PathBuf, PathRefusal>; // today's sync/files.rs:41 inside()
}

/// Policy 1, project admission: what a document may contain.
/// Owned by `librepaper-document`, mirrored by web/src/lib/paths.js.
pub struct ProjectPath(RelPath); // NFC-normalised
impl ProjectPath {
    pub fn admit(rules: &Rules, raw: &str) -> Result<(ProjectPath, Kind), PathRefusal>;
    pub fn collision_key(&self) -> String;
}

/// Policy 2, repository paths: what may be named to Git. Admits dotfiles and
/// `.github/...`; refuses only a `.git` segment, case-insensitively.
pub struct RepoPath(RelPath);
impl TryFrom<&str> for RepoPath { type Error = PathRefusal; }

/// Policy 3, folder portability: whether a project path can be written to a
/// local folder on every desktop OS. Refuses `:`, a segment that ends in a
/// space or a dot, and reserved Windows names. A refusal reports the file and
/// skips it; it never renames the file.
pub fn portable(path: &ProjectPath) -> Result<(), PathRefusal>;

pub struct PathRefusal { pub path: String, pub reason: Cow<'static, str> }
```

**Project admission** (`ProjectPath::admit`) is today's `document/projection/paths.rs:65` `check`, unchanged in behaviour. It trims and NFC-normalises first (the browser trims too, `web/src/lib/paths.js:24`). It checks `rules.max_path` (configurable, default 200), `rules.max_segments`, dotfiles, and the extension against the configured kinds, refusing unsupported and derived extensions. It stays fallible and keeps the rules argument. It moves no rule, so the browser and the fixtures need no change. Its callers are uploads, renames, source changes (`room/source_change.rs:486` `validate_path` moves onto it) and sync admission. `sync/rules.rs:54` `refusal` and `:102` `check` duplicate admission, and move onto `admit` followed by `portable`. `rules.rs:35` and `projection/paths.rs:114` `kind_of` merge.

**Repository paths** (`RepoPath`) are today's `companion/local/service/git.rs:623` `project_path`: structure, plus no `.git` segment. Git diffs and selected commits keep working on `.gitignore` and `.github/workflows/...`.

**Folder portability** (`portable`) is the part of `sync/rules.rs` `refusal` that admission lacks: `:` and trailing spaces. It is a separate policy on purpose. `notes:2026.md` is a legal project file that cannot be written to a Windows folder, so pull reports and skips it. That is today's behaviour, and it becomes a named rule instead of an accident.

**Structure only** (`RelPath`) is what the staging and output paths need. `document/paths.rs:53` `safe_relative_path` accepts `asset:figures/plot.png` on purpose (its own test says so), and refuses only a drive prefix. `RelPath` keeps exactly that. `companion/local/backup.rs:1090`, `cli/export.rs:295`, `engine/storage/source_archive.rs:388` and the `protocol.rs:230` re-export move onto `RelPath`. Each takes paths from a project the server already admitted, or from its own archive, so structure is the right check. `source_archive.rs` keeps its 4096-byte ceiling as a structural size cap.

Object keys and archive prefixes (`blob.rs:19`, `backup.rs:402`) are not relative paths, and keep their own validators.

Each policy gets a table of test examples, including the ones where policies part:

| Path | Structure | Admission | Repository | Portable |
|---|---|---|---|---|
| `main.tex` | yes | yes | yes | yes |
| `.gitignore` | yes | no (dotfile) | yes | n/a |
| `.git/config` | yes | no | no | n/a |
| `notes:2026.md` | yes | yes | yes | no |
| `asset:figures/plot.png` | yes | not a project path | n/a | n/a |
| `C:/x` | no | no | no | n/a |
| ` a.tex ` | yes | yes, as `a.tex` (trimmed) | yes, verbatim | yes after admission |
| a path of 201 bytes | yes | no with the default rules | yes | n/a |

### 3.3 `LineEnding`

```rust
pub enum LineEnding { Lf, CrLf, Cr }
impl LineEnding {
    pub fn detect(bytes: &[u8]) -> Self;
    pub fn normalise(text: &str) -> Cow<'_, str>;   // to LF
    pub fn apply(self, lf_text: &str) -> String;
}
```

Replaces `sync/files.rs:31` `lf`, `:54` `line_ending`, `:65` `with_ending`, and `document/session/edits.rs:60-68` `line_feeds`.

### 3.4 `Role` and `Capabilities`

```rust
pub enum Role { Reader, Commenter, Editor, Owner } // Ord
impl TryFrom<u8> for Role { .. }   // no default arm
impl Role { pub fn wire(self) -> u8; }
impl FromStr for Role { .. }       // all accepted aliases, one table
pub struct Capabilities { pub read: bool, pub comment: bool, pub edit: bool, pub share: bool, pub moderate: bool }
impl From<Role> for Capabilities { .. }
pub struct Ceiling { pub edit: bool, pub comment: bool }
impl Ceiling { pub fn clamp(&self, granted: Role) -> Role; }
```

Replaces `engine/storage/store.rs:387` `Role`, `engine/storage/postgres/access.rs:8` `AccessRole`, the `u8` in `base/auth/mod.rs:75,85,174`, `server/agent_auth.rs:70-72`, `server/mod.rs:205` `grant_role_ceiling` (**confirmed** fail-open `_ => Role::Editor`), and `server/sharing.rs:105,142` `parse_role_word`/`parse_token_role`. `engine/log/sequencer.rs:73` `Role { Editor, Reader }` is a different concept and is renamed `SubscriberClass`.

`Ceiling::clamp` replaces the three hand-written clamps in `store.rs:249-268` (twice) and `server/mod.rs:418-437`, and the clamp-then-demote at `server/mod.rs:~1541`.

Loose capability bools go: `server/socket.rs:136-137` `may_edit`/`can_comment` become a stored `Role`, and reauthorisation compares roles. The `is_owner`/`editor` bools in `room/comments.rs` (which mean the Editor rung) become `Role`. `server/routes.rs:1544` `owned`, `assistant.rs:6-16` and `mcp.rs:916` emit `Capabilities`, keeping today's JSON key names.

### 3.5 `Token`, `Secret<T>`, `Bytes`

```rust
pub fn random<const N: usize>() -> [u8; N];
pub enum Encoding { Hex, Base64Url }
pub fn token<const N: usize>(encoding: Encoding) -> String;

pub struct Secret<T: Zeroize>(T); // Debug prints "[redacted]", no Display
impl<T: Zeroize> Secret<T> { pub fn expose(&self) -> &T; }

pub struct Bytes(u64);
impl Bytes { pub const fn kib(n: u64) -> Self; pub const fn mib(n: u64) -> Self; pub fn get(self) -> u64; }
```

- Tokens: `base/util.rs:69` and `companion/local/store.rs:70` are both named `random_token()` and differ (**confirmed**: 16 bytes hex and 32 bytes base64url). Both go. Each caller names its length and encoding. Also replaces `server/chat/hub.rs:10`, `server/sharing.rs:182` `mint_link_key`, `document/session/shape.rs:57` `mint_id` and `companion/local/bindings.rs:35`. `auth/device.rs:312,323` user codes keep their alphabets but use `random`.
- Secrets: nothing in the workspace uses `zeroize` or `secrecy` (**confirmed**). Link keys, session keys, agent tokens and the sealed bytes in `LinkGrant` (`store.rs:118`, which derives `Debug` today) become `Secret`. The hand-written redacting `Debug` impls in `auth/github.rs:33`, `auth/oidc.rs:152` and `cli/server_config.rs:91` go. Check `Viewer.key` (`server/mod.rs:~492`) and wrap it if it is the link key.
- Sizes: one `limits` module in `base/config` owns every byte cap and default. Constants that share a name and differ in value get distinct names: `MAX_RESPONSE_BYTES` is 64 KiB in `companion/automation/peer.rs:64` and 160 MiB in `companion/local/backup.rs:23`. `MAX_MESSAGE` is 64 KiB in `server/mcp.rs:14` and 200 in `server/source_change.rs:16`. `cli/server_config.rs` renames its `_mb` fields to `_mib` where the value is MiB.
- Durations are `Duration` everywhere inside the program. Seconds and milliseconds appear only at serde and SQL edges.

### 3.6 Typed identifiers

`Slug`, `LinkKey` and `AgentToken { origin, slug, key }` get one `FromStr`/`Display` each in base.

- Slugs: `server/routes.rs:880` `valid_slug`, `server/reply.rs:6` `slug_pattern` and `sync/link.rs:57` `is_slug` merge. The configurable server regex stays, as a check layered on the shared syntax.
- Agent tokens: `server/mod.rs:445` `parse_agent_token` and `cli/agent.rs:104-119` merge.
- `""` as "no link" in `Viewer.link` becomes `Option<LinkDigest>`. `hash_link_key` (`server/sharing.rs:170`) trims and returns `""` for empty. `LinkKey::from_str` takes over the trim and refuses an empty key, because keys arrive from URLs and headers where stray whitespace is plausible. `LinkKey::digest()` then hashes the parsed key and never trims. The survey's comparison with `server/chat/hub.rs:19` was a false lead: that hashes a server-minted chat channel token, not a link key.

`DocId` and `AccountId` newtypes are deferred. The ids are already `Uuid` inside the store, so the only bug they would prevent is passing one kind of id where the other is meant, and nothing in the survey found that happening. They would touch hundreds of lines for that. Revisit after phase 7, starting at the engine store API if at all.

## 4. Typed errors and one refusal shape

### 4.1 Engine errors stop carrying meaning in text

`postgres::Error` / `CatalogError` gains typed variants:

```rust
pub enum CatalogError {
    NotFound,
    Conflict,                         // the row moved; reload
    Duplicate(DuplicateOf),           // e.g. a project name
    Quota(QuotaScope),                // Owner | Deployment
    RateLimited,
    Invalid(&'static str),
    Storage(StorageCause),
}
```

Each quota producer returns `Quota(..)`: `postgres/document_log.rs:410,420,517,526,727`, `repository.rs:1236`, `labels.rs:351`, `chat.rs:275,277`. `store.rs:509` `source_put_error` then matches variants. It no longer calls `message.contains("quota")` (**confirmed**, `store.rs:511`), or compares against the sentence "A project with this name already exists..." (**confirmed**, `store.rs:522`).

**Bug fixed:** "deployment storage threshold exceeded" (`document_log.rs:420`, **confirmed** to be `Error::Conflict`) has no "quota" in it, so it falls to the 409 "the document changed; reload it" arm. It reaches `source_put_error` through `command_put_error` (`store.rs:547`). The route that shows it to a user was not traced. Write a test that fills the deployment limit and expects 507 before changing the code.

The same variant is answered with different statuses at `server/documents.rs:514,1594` (429), `:1551,1689` (409 with raw text), `server/signin.rs:229` (403) and `server/mod.rs:807`. Each site matches the variant it means.

### 4.2 No `From<String>` on public error enums

Remove these, and name a variant at each `?` site: `room/error.rs:122` (makes every string a retryable 503), `companion/local/service/backups.rs:104-113`, `companion/local/render/workspace.rs:463-475` (both directions), and `cli/documents.rs:146-160` (loses the HTTP status). `engine/storage/backup.rs` has 17 `Result<_, String>` sites, which get a `BackupError`.

### 4.3 One `Refusal`, three projections

```rust
pub struct Refusal {
    pub kind: Kind,          // NotFound, Forbidden, Unauthenticated, Conflict(Reason), Invalid,
                             // Quota(QuotaScope), RateLimited, Unavailable, Internal
    pub retry: Retry,        // Never | After(Duration) | Safe
    pub committed: Committed,// No | Unknown
    pub message: String,
}
impl Refusal {
    pub fn http(&self) -> Reply;          // {error, reason, retryable, scope}
    pub fn socket(&self, ids: FrameIds) -> Value;
    pub fn mcp(&self) -> Failure;          // {code, message, recovery}
}
```

It lives in `librepaper-room`, which every transport already depends on. `WriteError`, `AgentError`, `PutError`, `ModifyError` and graph errors convert into it with `From`, each a total `match`.

It replaces:
- `room/error.rs:69-94` `WriteError::status()`, `reply.rs` `refused_with`, `socket_refusal` and `refusal_value`, `documents.rs` `tag_failure` (~2180), `graphs/routes.rs:802-835` `json_status`/`text`, and `source_change.rs:219`.
- The MCP classifiers `mcp/operations.rs:65-75`, `:~110` `definite_noncommit_code` and `mcp/comments.rs:51-80`.
- The roughly 15 hand-built `{"type":"error"}` frames in `server/socket.rs` (414-603) and `server/mod.rs:1857,1909,2017,2052`. Today these pick different subsets of `temp_id`, `request_id`, `version` and `protocol`. `FrameIds` makes the ids required where the client clears pending state by them.

A storage failure is classified by what is known about the write, not by which transport carried it. Today it is retryable 503 over HTTP, `outcome_unknown` over MCP operations and `unavailable` over MCP comments. Those may not all be wrong: a failure before the write started is `committed: No` and safe to retry, and a failure during the commit is `committed: Unknown`. Each producer sets `committed` where it knows the answer. Each transport then projects both states, and `outcome_unknown` stays the MCP spelling of `Unknown`. Check each of the three current sites for which state it really is before porting it.

Before changing any `reason` or `error` string, grep `web/src` for it. The browser keys off some of them.

### 4.4 Companion refusals

`companion/local/service/connection.rs:~520-530` and `sync.rs:~843-847` both build `(409, {"reason":"changed"})` and `(409, {"reason":"suggestion"})`. The `merging` refusal is built two ways (`connection.rs:~275`, `sync.rs:81`). One `ConflictReply` enum in `service/mod.rs` owns the reason strings.

## 5. Text offsets

### 5.1 Units per boundary

External offsets are not all one unit. The first draft treated every caller offset as UTF-16, which is wrong. The units are deliberate, and they stay:

| Boundary | Unit | Where |
|---|---|---|
| Editor, socket edits, Loro text, comment anchors | UTF-16 | `session/edits.rs:12` `Edit`, Loro `*_utf16` |
| Agent source queries, source patches, MCP range ids | UTF-8 bytes | `room/agent_query.rs:811` `range_value`, `room/source_change.rs:67` `Patch`, `server/mcp.rs:937` `decorate_ranges`, `server/source_change.rs:527` |
| Folder sync diffs | UTF-8 bytes | `sync/diff.rs:14` `Patch` |
| Track-changes planning | code points (internal only) | `document/track.rs:168,1777` |

The same numbers mean different text in different units. In `😀abc`, the range 4..5 is `a` in bytes and `c` in UTF-16. So the unit is part of the type, and a protocol's unit never changes as a side effect of this work. The MCP and source-change protocols stay in bytes. Their range ids keep encoding byte indices.

### 5.2 Types

```rust
pub struct Utf8(pub usize);
pub struct Utf16(pub usize);
pub struct CodePoint(pub usize);
pub struct Span<U> { pub start: U, pub end: U }

pub struct TextIndex<'a> { text: &'a str, /* lazy UTF-16 checkpoints */ }
impl TextIndex<'_> {
    // UTF-16 to bytes
    pub fn utf8_of(&self, at: Utf16) -> Result<Utf8, OffBoundary>;    // strict
    pub fn utf8_floor(&self, at: Utf16) -> Utf8;                       // to the start of the char it splits
    pub fn utf8_ceil(&self, at: Utf16) -> Utf8;                        // to the end of the char it splits
    // bytes to UTF-16
    pub fn utf16_of(&self, at: Utf8) -> Result<Utf16, OffBoundary>;   // strict: refuses a non-char-boundary byte
    // checked slices in either unit
    pub fn slice8(&self, span: Span<Utf8>) -> Option<&str>;
    pub fn slice16(&self, span: Span<Utf16>) -> Option<&str>;
    pub fn slice16_lossy(&self, span: Span<Utf16>) -> Cow<'_, str>;   // keeps U+FFFD at a split edge
    // code points, for track.rs only
    pub fn code_point_of(&self, at: Utf16) -> Result<CodePoint, OffBoundary>;
}
```

It lives in `librepaper-document`, which room and server depend on. Each call site picks a conversion by name, so its edge rule is visible. Conversions happen only where a value crosses from one boundary in the table to another. Example: a byte range from an agent becomes UTF-16 at the point it is written to Loro.

### 5.3 What it replaces

The converters, in both directions:
- `document/anchors.rs:268-310` (`utf16_len`, `byte_at`, `slice16`, `last_char_start`, `width_at`)
- `room/agent.rs:316-331` (`utf16_slice`, `utf16_byte`)
- `server/mcp/operations.rs:1479` `byte_of_utf16`
- `document/track.rs:1064` `utf16_to_code_point`
- `room/text.rs:47` `slice16`
- `room/locate.rs:558` `unit_char`
- the `str_indices` byte-to-UTF-16 call in `room/source_change.rs:573,639`, whose behaviour on a non-boundary byte is unverified. `utf16_of` makes it a refusal.

**Confirmed divergence:** `byte_of_utf16` returns `None` for a position inside a surrogate pair, and the caller turns that into a client error. `utf16_byte` moves to the next character.

The rule is decided, and depends on where the offset came from:
- An offset that a caller sent, in that boundary's own unit, is converted strictly (`utf8_of` or `utf16_of`). An offset that splits a character is refused as `invalid_range`. That is a UTF-16 offset inside a surrogate pair from the editor or socket, or a byte offset inside a multi-byte character from MCP or a source patch. Silently moving it would put the edit somewhere the caller did not ask for.
- An offset the program worked out itself (anchor resolution, relocated comments, display excerpts) uses `utf8_floor` for a start and `utf8_ceil` for an end. The span then widens to cover the whole character, and never narrows.
- Display text that must show something uses `slice16_lossy`.

`room/agent_query.rs:811-827` `range_value` emits `start`, `end` and `start_byte`, all as byte numbers. That is the protocol, so it stays. Drop `start_byte` only if no client reads it: grep the agent bridge and `web/src` first.

### 5.4 Edits and overlap

`Edit { at, delete }` (`session/edits.rs:12`), `Patch { start, end }` (`sync/diff.rs:14`, `room/source_change.rs:67`) and the `(String, usize, usize)` tuples (`room/source_change.rs:502`, `server/mcp.rs:1042`) become `Edit<U> { span: Span<U>, insert: String }`. Each keeps its unit.

The edit-against-edit overlap rules are three distinct policies, not two. An empty range (an insertion) needs its own answer, separate from ranges that touch at an end:

```rust
pub struct OverlapPolicy {
    pub touching: Touching,          // ranges meeting at one end: [0,2) and [2,4)
    pub insert_at_boundary: Allow,   // an insertion at the start or end of a range
    pub coincident_inserts: Allow,   // two insertions at the same position
}
pub fn check_overlaps<U: Ord + Copy>(edits: &mut [Edit<U>], policy: OverlapPolicy) -> Result<(), Overlap>;
```

The two current edit-against-edit policies, written as named constants:

| Case | `APPLY_SORTED` (`edits.rs:96` `apply_text_edits`) | `AGENT_PATCH` (`room/source_change.rs:502` `reject_overlaps`) |
|---|---|---|
| `[0,2)` then `[2,4)` | allowed | allowed |
| insert at 2, then `[2,4)` | allowed | allowed |
| `[0,2)`, then insert at 2 | allowed | allowed |
| insert at 2, then insert at 2 | allowed (applied in order) | refused |
| `[0,3)` and `[2,4)` | refused | refused |
| unsorted input | refused (`at` before the previous end) | sorted first, then checked |

`APPLY_SORTED` also requires its input to be sorted, and `AGENT_PATCH` sorts. That stays as it is. Each constant gets these rows as tests before either function is ported.

`Unsuggested::to_stored` (`track.rs:2348`) answers a different question: does a range meet an existing suggestion mark? It counts touching as meeting (`start <= m.end && m.start <= end`). That is not an edit overlap, so it stays where it is, and gets a test for the touching case.

### 5.5 Finding a passage

```rust
pub enum Occurrence { None, One(Span<Utf8>), Many }
pub fn unique_occurrence(hay: &str, needle: &str) -> Occurrence;
```

This replaces the exact-once searches `room/agent.rs:552` `locate_unique` and `document/anchors.rs:172` `once`. The fuzzy locator `room/locate.rs:320` stays. Its three wrappers (`server/mcp.rs:990-1076`, `room/comments.rs:443`) convert once, through `TextIndex`.

### 5.6 Lines

```rust
pub struct SourceLines<'a> { .. }
pub struct Line<'a> { pub number: usize, pub span: Span<Utf8>, pub body: &'a str } // body excludes \r\n or \n
pub struct Position { pub line: usize, pub column: Utf16 }
```

This replaces the line splitters: `split_inclusive('\n')` (`room/agent_query.rs:268,758`, which keeps `\r`), `lines()` (`document/render.rs:19,77`, `shell/src/lib.rs:88`) and `split('\n')` (`document/opening.rs:160`). It also replaces the line/column code in `agent_query.rs:300-357` and `:1013`, which counts chars while the editor counts UTF-16 units.

### 5.7 Loro roots

One typed `Roots<'doc>` gives access to text by file id, file id by path, path by file id, anchor records and change records. It replaces `text_of`/`text_at` (`anchors.rs:645`, `track.rs:1736`, `session/edits.rs:33`, `session/shape.rs:148`), the linear `id_of_path` scan, `shape.rs:128`, and the roughly 20 `doc.get_map(CHANGES)` match blocks in `track.rs`.

`session/edits.rs:107-110` discards the result of `delete_utf16`/`insert_utf16` with `.ok()`. That becomes an error, even though validation should make it unreachable.

## 6. Access guard

```rust
/// Everything a recheck needs to rerun the same decision later: the request's
/// credentials and the identity it was admitted as.
pub struct Caller {
    headers: HeaderMap,        // or the parsed credentials taken from them
    arrival: Arrival,
    actor: String,             // today's `actor_scope(..)`, compared on recheck
    runner: Option<RunnerLease>, // execution epoch + runner conversation, when present
}
pub struct Authorized<R: Rung> { slug: Slug, entry: IndexEntry, viewer: Viewer, caller: Caller, _rung: PhantomData<R> }

impl Server {
    async fn authorize<R: Rung>(&self, ctx: &RequestContext, slug: &Slug) -> Result<Authorized<R>, Refusal>;
    /// Reruns the full decision, then runs `write` with the fresh viewer and
    /// no await between the two that the caller controls.
    async fn mutate<R: Rung, T>(
        &self,
        auth: &Authorized<R>,
        write: impl FnOnce(&Viewer, &IndexEntry) -> WriteFuture<T>,
    ) -> Result<T, Refusal>;
}
```

`authorize` runs the full chain once: credential, `auth_failed`, sign-in, `may_read`, rung, and for MCP the actor scope and runner lease. It returns one refusal shape. `Caller` keeps the request context, so a recheck can rerun every part of the decision. Today `mcp_recheck` (`server/mod.rs:504`) needs the headers, the arrival, the expected actor and the runner-lease headers. A guard that dropped them could not replace it.

The types do not prove freshness. `authorize` and a recheck would return the same type, and a caller could hold either across another `await`. So freshness is enforced where the write happens. The only way a handler reaches a mutation is `mutate`, and `mutate` always revalidates first. Mutating room and store calls take the `&Viewer` that `mutate` hands to `write`, not one the handler kept. Review checks that no handler mutates outside `mutate`. A test per route revokes access between `authorize` and the write, and expects `permission_changed`.

It builds on `authenticated_entry`, `entry_at_least` and `check_readable_at` (`server/mod.rs:898-965`). Inline copies move onto it:
- `documents.rs:1025-1035`, which uses `entry_viewer_in` and skips `auth_failed`
- `documents.rs:1103,1252` and `history.rs:807`
- `figures.rs:30,93,180`
- `mcp.rs:267,340,518`
- `source_change.rs:77`
- `socket.rs:180`

The post-await rechecks become `mutate` calls:
- `documents.rs:425-449`
- `history.rs:1036`
- `sharing.rs:336`
- the six `mcp_recheck` sites in `mcp/operations.rs`
- `chat/mod.rs:284-306`

`recheck_chat_caller` checks no rung. Confirm what chat delete should require before porting it.

Comment moderation gets one `Moderation::may(action, &Comment, &Caller)`. Today the command path (`room/comments.rs:1558`) compares `author_key`. The MCP and validation paths (`comments.rs:332` `deletable`, `:361-386`) compare `author`. They also disagree on whether an author may resolve. `author` is a server-derived stable key (**confirmed**: `server/mod.rs:1337` `comment_author`), so this is not exploitable. It is two identities for one person, and the rules should be one. The anonymous `render_digest` gate in `documents.rs:447` and `socket.rs:1016` moves into `Caller`.

The rule is decided: an editor may delete or resolve any comment, and an author may delete or resolve their own. `Moderation::may` compares one identity. Use whichever of `author` and `author_key` the command path stores for every comment, and check that the two are always equal for existing records before dropping either.

## 7. Storage

### 7.1 Usage accounting

```rust
pub enum UsageScope { Deployment, Owner(Uuid) }
bitflags! { pub struct Sources: u8 { ARCHIVES, ASSETS, SNAPSHOTS, UNCOMPACTED_AND_CHAT, RESERVATIONS } }
async fn usage_bytes(tx: &mut PgConnection, scope: UsageScope, sources: Sources) -> Result<i64>;
async fn lock_storage_usage(tx: &mut PgConnection) -> Result<()>;
```

One SQL builder replaces the roughly 12 hand-written sums: `postgres/document_log.rs:413,506,669,730`, `labels.rs:316,438`, `chat.rs:256`, `repository.rs:1134,1228,1552,1585,1647,1706`, `moderation.rs:89`. It also replaces the 13 `storage_usage WHERE singleton FOR UPDATE` locks (**confirmed** count). Each caller names its sources, so the deliberate omissions become visible: `durable_usage_bytes` drops reservations on purpose (`repository.rs:1544`). `document_storage_by_owner` drops reservations and filters `status='active'`, and nobody has said whether that is on purpose.

### 7.2 Write transactions

```rust
pub enum Fence { Required, Exempt(&'static str) }   // the reason is mandatory
pub enum Lock { StorageUsage, Account(Uuid), Document(Uuid) } // acquired in this order
async fn begin_write(&self, fence: Fence, locks: &[Lock]) -> Result<WriteTx<'_>>;
```

This replaces `begin_writer_transaction`, the bare `begin_metered` calls, the raw `pool().begin()` at `operation_outcomes.rs:84`, and about 14 hand-written `documents ... FOR UPDATE` preambles, some with `status='active'` and some without. Lock order is written once in code, instead of in comments (`chat.rs:120`).

The fence rule is decided. The epoch fence exists so that a process that lost the writer lease cannot commit something that depends on the writer's in-memory state: the sequencer, budgets and reservations. So:

- `Fence::Required` for anything a command, the sequencer or a byte reservation decides. That includes both operation outcome paths (`operation_outcomes.rs:28` and the raw transaction at `:84`). An outcome is the answer to a command, and only the writer decides commands. The cost is one `FOR SHARE` read.
- `Fence::Exempt` for plain row writes that hold no writer state, and that Postgres row locks already serialize. These are `repository.rs:414` `upsert_account` ("account row, no writer state"), `tags.rs:179` and `marks.rs:58` ("per-account preference rows"), and the `moderation.rs` writes ("operator action, row-locked"). A write from a superseded process here is an ordinary concurrent write, not a split brain.
- `document_log.rs:251,295`, which add the check by hand after `begin_metered`, become `Fence::Required`. That is no change in behaviour.

### 7.3 Pages

`LIMIT n+1` everywhere, so `has_more` is not guessed from `len < PAGE`. Today `moderation.rs:407-438` truncates operator lists at 500 and 50 with no cursor. `blob.rs:99` lists everything and filters in memory. `worker.rs:729-753` has four `*_done` flags. The wire-side `Page` and cursor in section 9.3 share the shape.

### 7.4 Clocks

Every expiry comparison uses one clock. `operation_outcomes` compares the database clock (`extract(epoch FROM now())`) with the app clock (`mod.rs:1038`), and forces expiry with `expires_at=0` (`mod.rs:1110`). Pick database `now()` for SQL predicates. Columns stay as they are.

## 8. Runtime

### 8.1 Background loops

```rust
pub struct Shutdown(CancellationToken);
pub struct Tasks { shutdown: Shutdown, set: JoinSet<&'static str> }
impl Tasks {
    pub fn every<F, Fut>(&mut self, name: &'static str, period: Duration, missed: MissedTickBehavior, f: F);
    pub fn spawn<Fut>(&mut self, name: &'static str, f: impl FnOnce(Shutdown) -> Fut);
    pub async fn stop(self, grace: Duration) -> Vec<&'static str>; // names that did not exit
}
```

A loop that panics is logged by name and restarted on its next period. This replaces the fire-and-forget loops:
- `server/serve.rs:694` (retention janitor, **confirmed**) and `:720` (sweeper)
- `server/metrics.rs:549`
- `companion/local/service/mod.rs:524` `run_reaper`
- `cli/agent.rs:153` `follow_grant`
- `companion/local/service/projects.rs:809` `run_poller`

The six stop mechanisms collapse into the token: `assistant/lifecycle.rs:99` `StopSignal`, the watch channels in `service/jobs.rs:108` and `service/mod.rs:2399`, the `Mutex` flag in `projects.rs`, and abort-as-stop. The file-polled stop request in `companion/local/cli.rs:197` and `lifecycle.rs:39` stays, because it crosses processes, but it cancels the token instead of being polled everywhere.

### 8.2 Keyed registries

```rust
pub struct KeyedRegistry<K, V> { .. }
impl<K: Hash + Eq + Clone, V> KeyedRegistry<K, V> {
    pub async fn get_or_admit<F, Fut>(&self, key: K, init: F) -> Result<Lease<V>, E>; // single flight
    pub fn retire_idle(&self) -> Vec<K>;
}
pub struct Lease<V> { .. } // explicit hold; idle means zero leases
```

This replaces idleness by reference count: `room/mod.rs:560,567` (`strong_count == 2` and `> 1`, **confirmed**) and `engine/log/mod.rs:243,357` (`== 2` and `== 1`, **confirmed**). It also replaces the hand-rolled single flight (`engine/log/mod.rs` `admitting`, `companion/assistant/registry.rs` `start_gate`). Other candidates are `projects.rs:667` `pollers`, `backups.rs:406` and `chat/hub.rs:25`.

The sequencer and room registries carry the fencing and writer-lease logic, and `close()` can deadlock on the writer lease. Port them last, with the gated Postgres suite (`tools/dev/db test`) as the check.

### 8.3 Retry

```rust
pub struct RetryPolicy { pub base: Duration, pub ceiling: Duration, pub max_attempts: Option<u32>, pub jitter: bool }
impl RetryPolicy {
    pub fn delay(&self, failures: u32) -> Duration;
    pub async fn run<T, E, Fut>(&self, op: impl FnMut() -> Fut, retryable: impl Fn(&E) -> bool) -> Result<T, E>;
}
```

`engine/storage/schedule.rs:42` `backoff` is the model and becomes a `RetryPolicy`. Ported: `sync/client.rs:304` (no jitter), `companion/assistant/transport.rs:82` (no jitter, unbounded), `companion/local/native.rs:137` (20 ms, no cap).

### 8.4 Subprocesses

```rust
pub struct Limits {
    pub timeout: Option<Duration>,
    pub cancel: Option<Shutdown>,
    pub keep: Keep,            // which part of the output survives the cap
    pub kill_group: bool,
}
pub enum Keep { Tail(Bytes), Head(Bytes) }
pub enum Ending { Exited(i32), TimedOut, Canceled, SpawnFailed(io::Error) }
pub struct Run { pub ending: Ending, pub stdout: Vec<u8>, pub stderr: Vec<u8>, pub truncated: bool }
pub async fn supervise(cmd: Command, stdin: Option<Vec<u8>>, limits: Limits) -> Run;
```

Output is returned whatever the ending. A render that hits its deadline or is canceled still shows the log it produced. Today `run_confined_logged` returns `(RunOutcome, Vec<u8>)` for every outcome, and `render/mod.rs:231` puts that output in `log_tail` even after a deadline. `Keep` preserves the two documented truncation rules. Render logs keep the tail, because the error is at the end. Git and probe output keep the head, because the version line or the first error is at the start. Each current caller states which one it uses.

This is the only spawn path in the companion. It merges `native.rs:38` `run_confined_logged`, `native.rs:192` `run_captured` and `folder.rs:136` `run_picker_process`, which kills only the child, so a grandchild can hold the pipe. It also covers `tools.rs:75` and `engine/storage/backup.rs:606` `postgres_tool` (kill path not checked). The server's `AgentTrackedCommand` and `RecordedCommand` are room commands, not subprocesses, despite the names, and are out of scope.

### 8.5 Byte pools

`engine/log/budget.rs:170` `Budget::reserve` (with a deadline) and `log/pending.rs:157` `reserve_scratch` (no deadline, deliberately) share one `Pool` with `take(bytes, Wait::Until(d) | Wait::Forever)`. The "enable the notify before rechecking" step is written once. Small; do it last.

## 9. Wire types

### 9.1 Snapshots

`VersionSnapshot`, `HeadSnapshot` and the existing `ProjectSnapshot` (`document/projection/mod.rs:139`) live together and derive both `Serialize` and `Deserialize`. Each has `fn verify(&self) -> Result<(), Mismatch>`. The server builds `history.rs:70-88` and `documents.rs:1218` from them. `sync/client.rs` (~280-300) deserializes them instead of walking `Value`, and calls `verify`, as `cli/export.rs:135` already does. `cli/history.rs:38` and `sync/client.rs:37` share one `LabelRow`.

### 9.2 Outcome enums

```rust
#[serde(rename_all = "kebab-case")]
pub enum PullOutcome { Skipped, FastForward, Merged, UpToDate, Conflicts(Vec<RepoPath>), Dirty(Vec<RepoPath>) } // Git reports these, so dotfiles can appear
pub enum JobState { Queued, Running, Done, Failed, Canceled }
```

**Confirmed divergence:** `companion/local/service/sync.rs:761` says `"already"` and `git.rs:1089,1102` says `"up-to-date"` for the same outcome. Check which spelling `web/src` reads before choosing. `protocol.rs:199` `JobStatus.status: String` becomes `JobState`.

### 9.3 Pages and cursors

The listing cursor `{after_updated, after_slug}` (`routes.rs:858,1420`) and the templates cursor `{after_updated, after_id}` (`documents.rs:1802`) become one cursor type. `clamp_limit` replaces three copies of `.parse::<u32>().ok().unwrap_or(200).clamp(1, 200)`. One client `walk(fetch, max_pages)` replaces `cli/documents.rs:163` and `companion/local/backup.rs:443`, which already disagree on a page cap.

### 9.4 Clients

```rust
pub enum Profile { Shared, Provider, Credentialed, Probe(Duration) }
pub struct ApiClient { http: reqwest::Client, site: SiteUrl, who: Credentials }
impl ApiClient { pub async fn send(&self, method: Method, path: &[&str], query: &[(&str, &str)], body: Body) -> Result<Reply, TransportError>; }
```

- `Credentialed` never follows redirects. `base/http.rs:16` `client()` follows them (**confirmed**), and the CLI and sync send bearer tokens and `x-librepaper-key` through it. Whether reqwest 0.13 strips custom headers on a cross-origin redirect is unverified. Do not rely on it. The CLI and sync move to `Credentialed`.
- The roughly twelve `reqwest::Client::builder()` sites in the companion (`backup.rs:103`, `service/backups.rs:574,653,1244`, `lifecycle.rs:46`, `cli.rs:284`, `zotero.rs:118`, `automation/peer.rs:503`) pick a `Profile`.
- `Credentials::headers()` is the only place auth and marker headers are built. Today `sync/client.rs:179` hand-builds `x-librepaper-client: cli`, and `peer.rs:237` sends `"1"`. `sync/client.rs:~370` stops copying `base::http::decode`.

### 9.5 `SiteUrl`

One parse and one canonical form for a server address, used wherever one enters the program. It replaces `cli/mod.rs:672` `server_or_die`, `companion/local/backup.rs:1139` `normalize_server`, `store.rs:78` `normalize_origin`, the key normalisation in `credentials.rs:10`, `control.rs:127`, and `sync/client.rs` `base()`, which drops the path. A deployment is an origin. Nothing in the server config supports a path prefix, and the companion already refuses any server address whose path is not `/` (`companion/local/service/control.rs:119`). `SiteUrl` keeps that rule everywhere: scheme, host and port, lowercase. It refuses credentials, a query, a fragment, and any path other than `/`. A trailing `/` is accepted and dropped. Plain HTTP is allowed only for loopback. The CLI's `server_or_die` therefore stops accepting `https://host/sub`, which today it would pass through and then mis-join.

Stored credentials are keyed by the canonical form. `companion/local/store.rs:78` `normalize_origin` already produces it, so existing companion keys still match. Check the CLI's stored keys and `credentials.rs:10` against it. If any differ, note in the release that people sign in again.

## 10. Order of work

Each phase is a set of independent pieces. A piece is one worktree and one Haiku worker, following `AGENTS.md`. Workers write code and report. They do not run checks. The coordinator reviews every diff, then runs `cargo check -p`, `cargo nextest run -p` for the touched crates and finally `make test`.

| Phase | Pieces (parallel within a phase) | Gate |
|---|---|---|
| 1. Leaf types | `Digest`; `RelPath` + `ProjectPath` + `RepoPath` + `portable` + `LineEnding`; `Role` + `Capabilities` + `Ceiling`; `Token` + `Secret` + `Bytes` + limits table; `Slug` + `LinkKey` + `AgentToken` | `make test`, including the browser projection fixtures unchanged |
| 2. Errors | Typed `CatalogError` with the deployment-quota test first; remove `From<String>`; `Refusal` and its three projections; companion `ConflictReply` | `make test` and the gated Postgres suite |
| 3. Text | Unit newtypes + `Span` + `TextIndex`; overlap policies with their tables as tests first; `unique_occurrence`; `SourceLines`; `Roots` | `make test`, plus an astral-text test per conversion site in both units (`😀abc`) |
| 4. Access | `Authorized<R>` + `Caller` + `mutate` on server handlers; `Moderation::may` | `make test`, the authz HTTP tests, and a revoke-before-write test per mutating route |
| 5. Wire | Snapshot types + `verify`; `PullOutcome` + `JobState`; pages and `walk`; `ApiClient` + `Profile` + `SiteUrl` | `make test`, `agent-bridge.mjs`, full-deployment Node tests |
| 6. Runtime | `Tasks` + `Shutdown`; `RetryPolicy`; `supervise`; then `KeyedRegistry` (room, then sequencer) | `make test` and the gated Postgres suite |
| 7. Storage | `usage_bytes` + `lock_storage_usage`; `begin_write`; pages; clock | `make test` and the gated Postgres suite |

Phases 3, 4 and 5 depend only on phase 1 and can run side by side. Storage is last because it waits on the fence decision.

Each piece's brief names the files to change, the helpers to delete and the call sites to port. It also restates that the worker must not run tests, builds or linters. Every piece ends with a grep showing that the old helper names are gone.

## 11. Decisions

Decided 2026-10-27.

1. **Writer fence.** Required for command, sequencer and reservation writes, including both operation outcome paths. Exempt, with a stated reason, for account, tag, mark and moderation rows. See section 7.2.
2. **Path prefixes.** None. A deployment is an origin, and `SiteUrl` refuses a path. See section 9.5.
3. **Link key trimming.** Trim once in `LinkKey::from_str`. Hashing never trims. See section 3.6.
4. **Offsets that split a character.** Refuse offsets that callers send, in their boundary's own unit (UTF-16 for the editor, bytes for MCP and patches). Widen offsets the program works out itself. See sections 5.1 and 5.3.
5. **Comment resolve.** An author may delete or resolve their own comment, and an editor may do either to any comment. See section 6.
6. **Id newtypes.** Deferred until after phase 7. See section 3.6.

Still open:

- Chat delete has no rung check in `recheck_chat_caller` (section 6). Decide the rung when porting it.
- `document_storage_by_owner` leaves out reservations and filters `status='active'` (section 7.1). Confirm that this is intended when porting it.
- Which `PullOutcome` spelling the browser reads (section 9.2). Settle it by grepping `web/src`.
