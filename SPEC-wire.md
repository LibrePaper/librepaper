# The desktop wire: versions, refusals and migrations

Status: proposed 2026-11-04

The desktop app and the server it opens are released together but updated apart. The page comes from the server and changes with every deploy. The local helper (the bundled CLI, `librepaper local start`) and the desktop shell change only when the app updates. Self-hosted servers can run months behind. This document specifies how the page and the app find out that they disagree, what the person sees when they do, and how the helper keeps working with older pages while the product moves fast.

## 1. Product principle

> The newer side adapts. Nobody is ever told to downgrade.

Each pairing of page and app ends in one outcome, and each one points forward:

| Situation | Typical cause | What the person sees |
|---|---|---|
| The page needs an addition the app lacks | A deploy; the app has not updated | That feature says "Update the app". Everything else works |
| The page's major is newer than the app's | A deploy with a breaking change; the app has not updated (7.5) | A banner says "Update the app". Local features are off |
| The page's major is older than the app's, inside the support window (6) | A self-hosted server behind | Nothing. The helper answers in the older shapes |
| The page's major is older than anything the app still speaks | A self-hosted server far behind | "{host} runs an old version of LibrePaper, so features that need the desktop app are off here. Ask its administrator to upgrade." This applies to that server only |

A mismatch never blocks the editor or the shell's own functions, such as switching servers.

## 2. Scope

### In scope

- One semantic version for the helper's wire (3.2).
- A range of major versions that the app answers, published to the page when the window opens.
- A minimum version on each page call, so a newer page on an older app turns off only the features the app lacks.
- From the first major bump, a version header on helper requests and structured refusals from the shell.
- One banner for a major mismatch, and an "Update now" route on the shell.
- Update checks beyond startup.
- Rules for evolving the wire, including the order in which a breaking change ships.
- A migration chain in the helper, so an older major costs a pair of small functions, not a second implementation.
- Shared wire fixtures for the routes the render path depends on, checked by Rust and by Node.

### Out of scope

- The browser to server room protocol (`librepaper.room.v3`, `docs/dev/protocol/room.md`). It is versioned separately.
- Downloading a helper that matches each server. The app ships one helper.
- A minimum app version that the server sets by hand. It can be added later for bugs that are not wire changes.
- Thinning the helper's existing routes. Section 8 sets the rule for new routes only.
- Fixtures for every route, and a test that every field and enum value appears in a fixture. Both can be added if gaps cause trouble.
- A stale-tab reload. The app's oldest supported major only rises through an app update, which restarts the app and reloads its pages, so an open page cannot fall out of the window.

## 3. Current behavior and constraints

These are implementation facts, not proposed behavior.

### 3.1 Who talks to whom

- The desktop shell (`desktop/src/main.rs`) loads the page from a server in its list (`desktop/src/servers.rs`, default `https://app.librepaper.org/`). Several servers may be listed, self-hosted ones included.
- The shell builds `window.__LIBREPAPER_DESKTOP__ = { base, version, zoom, close }` synchronously when it builds a window (`desktop_script`, `desktop/src/main.rs:196` and `:321`). The helper starts separately (`warm`, `main.rs:145`), so the helper may not be running yet at that moment.
- `version` is the app's release version. Only the diagnostics report reads it (`web/src/lib/support/report.js:90`).
- The page calls the shell at `${base}`. The shell adds the local credential and `x-librepaper-origin`, and forwards to the helper on loopback. The helper refuses requests that carry browser headers (`crates/librepaper-companion/src/local/service/mod.rs:600`). The shell forwards only what its `Forward` struct holds: method, path, origin, content type and body (`desktop/src/service.rs:32`).
- `${base}` is a custom scheme (`lp-local:`, or `http://lp-local.localhost/` on Windows), so it is a different origin from the page. A custom header makes the webview send a CORS preflight. The shell's preflight allows only `content-type, if-none-match` (`main.rs:585`).
- The shell asks the person to confirm two routes before it forwards them: `PUT renderers/<key>` and `POST companion/api/agents`. `prompt_for` (`main.rs:995`) recognizes them by path and parses their bodies with its own types.

### 3.2 The surfaces

| Surface | Path from the page | Served by | Client code |
|---|---|---|---|
| Feature routes | `${base}librepaper/local/…`: `health`, `capabilities`, `renderers/<key>`, `jobs…`, `zotero/…`, `agents`, `assistant/…`, `backups/…`, `connections/…` | helper, `dispatch` in `service/mod.rs` | `client.js:192` |
| Management routes | `${base}companion/api/…` | helper, `service/control.rs` | `client.js:221` |
| Shell routes | `${base}shell/…`: `server`, `theme`, `close`, `notify`, `quit`, `open-logs`, `diagnostics`, `diagnostics-report`, `zoom…`, `dropped/…`, windows | shell, `main.rs` | `shellAction`, `desktopReport`, `zoom.js:27`, and scripts injected by the shell |
| Page global | `window.__LIBREPAPER_DESKTOP__` | shell | `desktop()` |

`SignIn.svelte:35` is the only caller of `shell/server`, so that route must work whatever the page's version.

`DesktopOnly.svelte` is a per-feature notice, used in `Reader.svelte`, `reader/Agent.svelte` and three settings panes. The two page roots are `Landing.svelte` and `Reader.svelte`.

### 3.3 Version handling

- `PROTOCOL_VERSIONS: &[u32] = &[3]` (`crates/librepaper-companion/src/local/protocol.rs:15`). `GET health` reports it with the helper's release version.
- The shell has its own `const PROTOCOL: u32 = 3` (`desktop/src/service.rs:19`), and `compatible` (`:359`) refuses a helper whose list lacks it, with "Reinstall LibrePaper".
- `JobRequest::decode` refuses protocol 2 with "this page is older than the LibrePaper app: reload the page to update it", and anything else with "unsupported protocol version". Both are plain strings.
- The page sends `protocol: 3` in the job body (`client.js:429`). It never compares versions.
- The page's job poller loops until it sees `done`, `failed` or `canceled` (`client.js:361`).
- A helper refusal names its cause in a `reason` field, which `client.js:164` reads. `client.js:165` shows a refusal's `error` text as it is. `SPEC-unification.md` 4.3 proposes `{error, reason, retryable, scope}` for HTTP refusals.
- The desktop updater checks once at startup (`desktop/src/updater.rs`). macOS also has Help > Check for Updates, because only macOS has that menu. It is off for Scoop and MSI installs on Windows. "Later" in its dialog is not remembered.
- Linux ships only `.deb` and `.rpm` (`desktop/tauri.linux.conf.json`), and the updater module and plugin are not compiled on Linux.

### 3.4 History

The protocol number went from 2 to 3 once, on 2026-10-25 (`02a85b75`). In the following two days about ten commits changed shapes in `protocol.rs` without a bump: fields removed, the folder-project routes dropped, and `status` turned from a string into an enum. The repository cuts a release almost every day (v0.0.56 to v0.0.65 between 2026-10-29 and 2026-11-03). So apps that report protocol 3 do not all have the same shapes.

### 3.5 Strict decoding

Eight wire types use `#[serde(deny_unknown_fields)]`: `JobOptions` and `JobRequest` (`protocol.rs:89`, `:111`), `RendererRequest` (`service/mod.rs:866`), `render::tool::Record` (`render/tool.rs:132`), three bodies in `service/backups.rs:1015-1029`, and the shell's `RendererChange` (`main.rs:971`). With strict decoding, an older app refuses a request that has gained even one optional field.

### 3.6 Reply construction

Most helper replies are built with `json!` in the handler (`service/jobs.rs`, and 74 uses in `service/connection.rs`), so no Rust type describes them. Several request types are private, and `assistant` and `backup` are `pub(crate)` (`local/mod.rs:10-11`). Multipart parsing lives in the jobs handler (`service/jobs.rs:27`). `handle` sends `/companion` to `control::handle` before it reaches `dispatch` (`service/mod.rs:609`).

## 4. The protocol version

### 4.1 One semantic version for the helper's wire

One version, `MAJOR.MINOR.PATCH` under [Semantic Versioning](https://semver.org/), covers the feature routes and management routes. It is separate from the release version (`0.0.66`), which keeps its own numbering. There are no pre-release or build suffixes.

- **Major** goes up on a breaking change (7.2). The app answers a range of majors, and keeps older ones through migrations (7.4).
- **Minor** goes up on an addition (7.1). It resets to 0 when the major goes up.
- **Patch** goes up on a fix that changes no shape but that a page must rely on. It resets to 0 when the major or minor goes up. No test can detect one, so it is raised by hand. A fix that no page depends on needs no patch bump.

**The first version is `3.1.0`.** `3.0.0` means an app from before this change, with strict decoding, no version in its global, and the varied shapes of 3.4. Phase 1 removes strict decoding, which is an addition.

### 4.2 Where the version lives

The helper has it in `crates/librepaper-companion/src/local/wire/mod.rs`:

```rust
pub const MAJOR: u32 = 3;
pub const MINOR: u32 = 1;
pub const PATCH: u32 = 0;
/// The oldest major this binary still answers. Raised only under section 6.
pub const OLDEST_MAJOR: u32 = 3;
```

`PROTOCOL_VERSIONS` becomes `OLDEST_MAJOR..=MAJOR` collected into a list, so `Health.protocol` and `lifecycle::reusable` keep working. `Health` does not change.

The desktop crate does not depend on the companion crate, and the page global must be written before the helper is running (3.1). So the shell has its own constants in `desktop/src/wire.rs`:

```rust
pub const VERSION: &str = "3.1.0";
pub const OLDEST_MAJOR: u32 = 3;
```

`PROTOCOL` in `service.rs` becomes the major of `VERSION`, so `compatible` asks for the newest major. Raising `OLDEST_MAJOR` does not affect it.

Tests in both crates compare their constants with the files `crates/librepaper-companion/wire/OLDEST_MAJOR` and `wire/v{MAJOR}/VERSION` (9.1), so the two copies cannot drift. Shell and helper ship in one release, and `compatible` catches a mismatch from `LIBREPAPER_BIN`.

### 4.3 What the page is told

```js
window.__LIBREPAPER_DESKTOP__ = { base, version, zoom, close, protocol: { oldest: 3, current: "3.1.0" } }
```

`oldest` is a major. `current` is a full version. It is a compile-time fact, so it is available at once and cannot go stale while the app runs.

A global without `protocol` comes from an app released before this change. The page treats it as `{ oldest: 3, current: "3.0.0" }`.

### 4.4 What the page sends

**A page on major 3 sends no version header.** No header means major 3, so no header is needed until a second major exists, and the shell's preflight does not change until then.

The first major bump adds the header. A page on major 4 or later sends its full version, as in `X-LibrePaper-Protocol: 4.0.0`, on feature and management requests only. `client.js` adds it in the two functions that make those requests (`:192` and `:221`). Such a page calls only an app that has passed the check in 5.2, so the app is on major 4 or later and accepts the header. That release makes these changes in the shell:

- `x-librepaper-protocol` joins the preflight allowlist (`main.rs:585`).
- `Forward` gains a field for it, and the request builder sends it to the helper (`service.rs:186`). The allowlist and the forwarded set are separate lists, and both change.
- The shell parses the header. One that is not `MAJOR.MINOR.PATCH` in decimal is refused with `400 {"reason": "bad_protocol"}`.

`POST jobs` keeps its integer `protocol` field, which holds the major. When the field and the header's major disagree, the helper refuses with `400 {"reason": "protocol_conflict"}`.

### 4.5 Shell routes are not versioned

Shell routes and the page global are not covered by the version, and the shell never refuses them over a version. The person must be able to switch servers, quit, and read diagnostics across any mismatch, and scripts injected by the shell send no header.

Instead, the rule for them is stricter: **an existing shell route or global field never changes shape.** A new shape is a new route. A page checks that a new one exists before using it, by comparing `current` with the version that added it.

The routes that get a person out of trouble never go away: `server`, `quit`, `check-update`, `diagnostics` and `diagnostics-report`. Any other shell route or global field may be removed once no major in the support window (6) uses it.

## 5. Refusals and what the page does

### 5.1 The refusal shape

This ships with the first major bump (Phase 3). Before then, no request can fail the check: every page and app is on major 3.

The shell checks the version on feature and management requests, before `prompt_for` and before forwarding, so nobody confirms a command for a request that is about to fail:

```
HTTP/1.1 426 Upgrade Required
content-type: application/json

{"reason": "app_outdated" | "page_outdated", "error": "<English sentence>", "retryable": false, "sent": "4.0.0", "oldest": 3, "current": "3.1.0"}
```

- `app_outdated`: the request's major is above the major of `current`.
- `page_outdated`: the request's major is below `oldest`.
- The field names follow the helper's existing `reason` and the shape in `SPEC-unification.md` 4.3.
- A page from this design shows a translated message chosen by `reason`. A page from before it shows `error` as it is (3.3), so `error` must be a sentence a person can act on.
- `OPTIONS` is not checked.

The page normally never receives this, because it checks first (5.2). The refusal covers a page and app that disagree anyway. `client.js:192` and `:221` check for a 426 before their generic error handling, and raise a named error carrying `reason`, so the English `error` is not shown.

The helper does not repeat the range check. Only the shell reaches it, and it reads the header only to choose migrations. `wire::upgrade` returns an error for a major it has no path for, as a guard. `JobRequest::decode` keeps its own check for the same reason.

### 5.2 The check

A new module, `web/src/lib/desktop/negotiate.js`, computes the outcome from the global. It makes no request, and the result does not change during a page load.

```
const PAGE = "3.1.0"                     // the version this page speaks; client.js imports it from here
range  = global.protocol ?? { oldest: 3, current: "3.0.0" }
if major(PAGE) > major(range.current)   -> "app_outdated"
if major(PAGE) < range.oldest           -> "server_outdated"
if major(PAGE) == major(range.current)  -> "ok", and each `since` is compared with range.current
if major(PAGE) < major(range.current)   -> "ok", and every `since` is met
```

**Per-call minimum version.** `send` and `manage` in `client.js`, which every feature and management call goes through, take a `since` option. It defaults to `3.0.0`, which every app meets. A call that needs something newer names the version of the newest route, field or fix it depends on:

```js
await send("GET", "some/new/route", { since: "3.2.0" });
```

When the outcome is not "ok", both functions throw `DesktopMismatch`, carrying the outcome, before anything goes out.

When the outcome is "ok" but `range.current` is below `since`, compared by semantic version precedence, the call does not go out. It throws `DesktopFeatureOutdated`, and the feature shows "Update the app to use this", with the banner's update action (5.4). An addition that the page uses only when present, with a fallback, keeps its older `since` and checks the response itself. It may not send a new request field to an app below `3.1.0`, which would refuse it.

`since` only matters when the page and the app are on the same major. When the app's major is newer, the page's major has ended, so the app's migrations cover every addition that major had. The comparison is a few lines in `negotiate.js`, so the page gains no dependency.

**Older apps.** An app below `3.1.0` reports `3.0.0` whatever shapes it has (3.4). A refusal from such an app is shown as `app_outdated` only when its text starts with `bad protocol 3 job description` or `unsupported protocol version`, the strings strict decoding produces. Every other failure, such as a bad entrypoint, a missing tool or a failed render, shows as it does today.

### 5.3 The banner

A component, `web/src/components/DesktopMismatch.svelte`, is mounted once at the root of `Landing.svelte` and once at the root of `Reader.svelte`, so each page has one banner. It shows one line and at most one action:

| Outcome | Text (keys in `web/messages/en.json`) | Action |
|---|---|---|
| `app_outdated` | `desktopmismatch_app_outdated` with `{have}`, the app's release version | "Update now" (5.4), or a link to `INSTALL_URL` |
| `server_outdated` | `desktopmismatch_server_outdated` with `{host}`: "{host} runs an old version of LibrePaper, so features that need the desktop app are off here. Ask its administrator to upgrade." | none |

Every key goes into all seven catalogues under `web/messages/`, translated with the glossaries, as `AGENTS.md` requires. `DesktopOnly.svelte` reads the same outcome, so each feature's notice says why it is off.

### 5.4 Updating from the page

The shell gains `POST shell/check-update`, under the rule in 4.5:

- On a self-updating install it runs `updater::check_now` and returns `{"via": "updater"}`. The existing dialog handles the rest.
- On Linux it returns `{"via": "package_manager"}`, and the page says to update through the package manager.
- On Scoop and MSI installs it returns `{"via": "download"}`, and the page opens `INSTALL_URL`.

The updater module now compiles on Linux, so that this route can answer there, but it has no plugin and never runs a check.

The page offers "Update now" only when the global carries `protocol`, which means the app has this route. An older app gets the link.

### 5.5 More update checks

`desktop/src/updater.rs` also checks when a window regains focus, at most once every six hours, silently unless it finds an update. When the person chooses "Later", the updater remembers that version for the rest of the run and does not offer it again on focus. The startup check and Help > Check for Updates still offer it.

## 6. The support window

The app answers every major whose successor came out less than **six months** ago. `OLDEST_MAJOR` is raised only in a release.

A new file, `docs/dev/protocol/local.md`, holds a table with one row per version: number, the release that introduced it, its date, and the changes it made. The paragraph on the protocol in `docs/architecture/local-service.md` links to it. `docs/releasing.md` gains one step: "If a major left the window, raise `OLDEST_MAJOR` in both crates and in `wire/OLDEST_MAJOR`, and delete its migration."

The mechanism does not depend on the window's length, which can change.

## 7. Evolving the wire

### 7.1 Changes that bump the minor

These do not break older pages, but a newer page may depend on them:

- A new optional request field, decoded with `#[serde(default)]`.
- A new response field. Older pages ignore fields they do not know.
- A new route. Older pages never call it.
- A new enum value in a response, other than a terminal job state (7.2). Older pages map unknown values to a generic state. `client.js` must do this for every enum it reads, starting with job `status`.

A page that depends on one of these sets `since` on the call (5.2). Without that, a newer page on an older app would get a 404 for a new route, or have a new field ignored without notice.

The confirmed routes (7.4) take no additions at all. The shell decodes the renderer body strictly, so even an optional field would be refused by older shells. A new field on either one means a new route.

### 7.2 Changes that bump the major

- Removing or renaming a field or a route.
- Changing a field's type or meaning.
- A new required request field.
- A new enum value in a request.
- A new terminal job state. An older page's poller knows only the three in 3.3 and would poll until its deadline.

### 7.3 Strict decoding

`deny_unknown_fields` comes off `JobRequest`, `JobOptions` and the three bodies in `backups.rs`.

It stays on `RendererRequest`, the shell's `RendererChange` and `render::tool::Record`. Those carry a command that the shell shows the person before running it. If a field were dropped silently, the person could approve one command and the helper run another.

### 7.4 The migration chain

Older majors are kept as conversions, not as code paths. Each major that is no longer current has a file in `crates/librepaper-companion/src/local/wire/`:

```rust
// wire/v3.rs: what changed going from 3 to 4.
pub fn upgrade_request(route: Route, body: Value) -> Result<(Route, Value), Refusal>;
pub fn downgrade_response(old: &Route, status: StatusCode, body: Value) -> (StatusCode, Value);
```

- Both functions work on `serde_json::Value`, so old majors never need their own Rust types.
- `Route` is the method, the path segments and the query. `upgrade_request` may return a different route, so a moved route maps onto its replacement.
- `downgrade_response` gets the route the old page asked for, so it can shape the reply as that route used to.
- Routes the version did not change pass through untouched.
- `wire::upgrade` applies the files from the major of `sent` up to `MAJOR - 1` in order. `wire::downgrade` applies the reverse.

**Where it runs.** This is still to be designed, in Phase 3. It must cover both `control::handle` and `dispatch`, and `handle` receives a streaming body that each handler reads for itself (3.6). The likely shape is a wrapper at the top of `handle` that buffers JSON bodies in both directions, chosen by content type, with `jobs::handle_jobs_post` calling the chain itself on the `job` part once it has parsed it. File parts and file downloads are bytes and pass through unchanged.

**Removed routes stay served.** While a major is inside the window, every route it had keeps working through an adapter. If the capability itself is deliberately gone from the product, that route answers `410 {"reason": "feature_removed"}`, the page shows the feature as unavailable, and `docs/dev/protocol/local.md` says so. It is never a quiet consequence of a refactor.

**Confirmed routes are frozen.** The shell recognizes `PUT renderers/<key>` and `POST companion/api/agents` by path and reads their bodies before the helper's migrations run (3.1). If a migration moved or reshaped either one, an old page's request could reach the helper without the shell's confirmation. So neither route's path or body ever changes, and no migration may map a route onto them. A new shape is a new route with its own prompt in `prompt_for`, added in the same change, and the old route stays until its major leaves the window. A test passes both routes through `wire::upgrade` for every supported major and asserts that they come out unchanged.

### 7.5 The order of a breaking change

A new major ships in the app and the page in the same release. That release raises the helper and shell to `4.0.0`, with `OLDEST_MAJOR` still 3 and the `wire/v3.rs` migration, and moves the page to `PAGE = "4.0.0"`.

**The app's update and its Linux packages must be published before the server deploys the new page**, so an update is always there to install.

An app that has not updated loses local features until it does, and the banner says why. The editor and the shell's own functions keep working. macOS and Windows NSIS installs update in one click. Linux users update through their package manager, and Scoop and MSI users by hand or through the link. This interruption is the reason to keep changes additive.

There is no waiting period. If the interruption comes to matter, the wait should depend on measured adoption, such as the share of active apps that have updated, not on a fixed date.

## 8. The rule for new routes

A new helper route offers a general building block, not a feature. Examples are "run the renderer for this format and output", "list what is installed" and "read this local item". The page decides what to do with it. A feature is redesigned in the page, which is always current, and does not move the wire.

Reviewers check this rule when a change adds a route.

## 9. Wire fixtures

The fixtures make a version bump hard to forget, for everything they show. The first set (Phase 2) covers `health`, `capabilities`, `renderers/<key>` and the `jobs` routes, the path that builds depend on. Other routes are protected by review until they are added.

### 9.1 Layout

```
crates/librepaper-companion/wire/
  OLDEST_MAJOR                              "3"
  v3/
    VERSION                                 "3.1.0"
    jobs.post.request.full.json
    jobs.post.request.minimal.json
    jobs.get.response.queued.json
    jobs.get.response.done.json
    jobs.get.response.failed.json
    capabilities.get.response.full.json
    …
```

By convention, `full` sets every optional field, `minimal` sets only the required ones, and each enum value gets a fixture named after it. A field or value that no fixture shows is not protected.

The fixtures for an older major are frozen once that major is superseded. The fixtures for the current major grow with each minor.

### 9.2 What "compatible" means

The checks compare **structure**, never bytes:

- **Structurally compatible.** Every key in the fixture is present in the value, with the same JSON type, recursively. Arrays are compared element by element against the fixture's first element. A key the value has but the fixture lacks is allowed.
- **Structurally equal.** Compatible in both directions.

Neither check compares values, apart from enum strings, which must match the fixture they are named after.

### 9.3 Rust checks

These live in `crates/librepaper-companion/src/local/service/wire_tests.rs`, a test module of `service`, because many of the types are private (3.6) and the router test helpers are there. A reply with a named type is encoded from a sample value. A reply built with `json!` is produced by driving the router, the way the tests in `service/mod.rs` already do.

1. **Every supported major still works.** For each major from `OLDEST_MAJOR` to `MAJOR`, each request fixture goes through `wire::upgrade`, must decode into the current type, and is encoded again. A key the fixture has and the new encoding lacks fails: with strict decoding gone, this is what catches a field removed from a type. Defaults filled in by decoding are allowed, so the comparison is structural compatibility, not equality. Each current reply, passed through `wire::downgrade` to that major, must be structurally compatible with the fixture.
2. **The current major has not broken.** Each current body is compared with the `v{MAJOR}` fixtures:
   - a fixture key missing, or of another type, fails with "Breaking wire change: raise `wire::MAJOR`, add `wire/v{N+1}/` and `src/local/wire/v{N}.rs`";
   - a key present only in the current body fails with "Additive wire change: raise `wire::MINOR` and run with `WIRE_BLESS=1`";
   - structural equality passes.
3. **The versions agree.** `wire::MAJOR.MINOR.PATCH` equals `v{MAJOR}/VERSION`, and `OLDEST_MAJOR` equals its file. `desktop/src/wire.rs` has the same test.
4. **Confirmed routes are frozen** (7.4). This check arrives with the migrations, in Phase 3.

**Blessing.** `WIRE_BLESS=1` rewrites the current fixtures and `VERSION`. It refuses a change that check 2 calls breaking, unless the major is new. It compares against `VERSION`:

| Change | What blessing requires |
|---|---|
| New keys | Same major, greater minor |
| No shape change | Same major and minor, greater patch |
| New major | `v{N}/` does not exist yet, `v{N-1}/` does, and `src/local/wire/v{N-1}.rs` exists. The version must be `N.0.0` |

### 9.4 Node check

`web/tests/unit/desktop-wire.mjs` runs `client.js` with an injected `fetch` that records each request. It checks that:

- a page on major 3 sends no `X-LibrePaper-Protocol` on any request. From Phase 3, a page on a later major sends it, equal to `PAGE`, on every feature and management request and on no shell request;
- `PAGE` equals the `VERSION` file of the fixtures for its major;
- each request body with a fixture has every key of `minimal` and no key beyond `full`, with matching types, so a page sending a field the helper does not know fails;
- the page's job `status` reader maps an unknown value to its generic state;
- every call's `since` has the major of `PAGE` and is not above `PAGE`.

The Rust and Node checks read the same files.

Nothing checks that a call's `since` is high enough. A new call with too low a `since` gets a 404 from an older app. Reviewers check this.

## 10. Rollout

### Phase 1: detection

Ships in one release, as version `3.1.0`.

1. The `wire` modules in both crates, the version files with their agreement tests, and `PROTOCOL_VERSIONS` and `PROTOCOL` derived from them.
2. `protocol` in the page global.
3. `negotiate.js`, per-call `since` in `client.js`, `DesktopMismatch.svelte`, the `DesktopOnly.svelte` change and the message keys.
4. `shell/check-update`, and the focus update check with its memory of "Later".
5. `deny_unknown_fields` removed as in 7.3.
6. `docs/dev/protocol/local.md`, and the step in `docs/releasing.md`.

A page from Phase 1 must still work with apps released before it. Such an app has no `protocol` in its global and refuses in plain strings. The page treats it as `3.0.0` and handles its refusals as in 5.2.

### Phase 2: fixtures

The `v3` fixtures for the routes in section 9, the blessing mode, and the Rust and Node checks. This changes nothing for users, and it must land before the wire version moves past `3.1.0`: it is what stops unversioned changes.

### Phase 3: the first major bump

Done when the next breaking change lands, not before, in the order given in 7.5.

1. The header, in the page and the shell (4.4).
2. The `426` check in the shell, and its handling in `client.js` (5.1).
3. The migration chain, including where it runs (7.4), and `wire/v3.rs`.
4. The `wire/v4/` fixtures and the frozen-route check.

## 11. Testing

| Change | Checks |
|---|---|
| `wire` module, migrations, fixtures | `cargo nextest run -p librepaper-companion` |
| Shell header check (Phase 3), `shell/check-update`, global, `desktop/src/wire.rs` | `cargo nextest run` in `desktop/`, and a manual run against a local server with `LIBREPAPER_SERVER` |
| `negotiate.js`, `client.js` | `node --test --experimental-transform-types web/tests/integration/build-local.mjs` and `web/tests/unit/desktop-wire.mjs` |
| `DesktopMismatch.svelte`, messages | `web/tests/unit/untranslated.mjs`, `bun run check:types` |

Manual checks before Phase 1 ships, on Windows and on one other platform, because the base URL differs:

- an app with this change against a page built with `PAGE = "4.0.0"`, which should show the banner, where "Update now" reaches the updater and switching servers still works;
- a page with this change against v0.0.66 of the app, which should work as it does today, where a failed render shows its own error and not "Update the app";
- a page with a call whose `since` is `3.2.0` against an app at `3.1.0`, where only that feature should ask for an update;
- a window opened before the helper has started, which should show the correct range;
- on Linux, "Update now", which should point to the package manager;
- choosing "Later" in the update dialog, which should not bring the dialog back on the next focus check.

## 12. Open questions

None.
