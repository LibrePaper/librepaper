# SPEC: OpenID Connect sign-in, with Google and Entra ID as presets

Status: proposed and reviewed 2026-10-16, not started. It replaces PR #44.
It closes #30 and #43.

## Summary

LibrePaper gets one OpenID Connect (OIDC) client. Google becomes a preset of
that client, and `google.rs` is deleted. Microsoft Entra ID is a second
preset. Any other OIDC provider (Keycloak, Authentik, Okta, GitLab,
Zitadel) is a custom entry. GitHub stays its own OAuth2 adapter, because
GitHub has no OIDC for user sign-in.

The design is conservative on purpose:

- One code path handles every OIDC provider. Presets add claim rules. They
  never add a second flow.
- The id token is read and checked (`iss`, `aud`, `azp`, `exp`, `iat`). Its
  signature is not checked, because it arrives straight from the token
  endpoint over TLS. OIDC Core 3.1.3.7 allows this.
- Every sign-in is bound to one provider from start to callback. A sign-in
  started with one provider cannot finish at another.
- Every provider endpoint must be HTTPS. Loopback is the only exception, for
  tests and local development.
- The admin console keeps GitHub sign-in only. It does not change. A
  deployment that only has Entra still registers a GitHub OAuth app for
  the console.
- Existing GitHub and Google accounts keep their ids. Existing sessions stay
  valid. No registered redirect URI changes.
- One setting, `auth.session_lifetime`, bounds how long a sign-in lasts
  without the provider being asked again. The default is the 30 days of
  today, so nothing changes where it is not set.

## Goals

1. A self-hosted instance lets staff sign in with Entra ID, limited to one
   tenant, with no GitHub or Google account.
2. A self-hosted instance lets people sign in with any standard OIDC
   provider by giving its issuer URL.
3. Google sign-in behaves exactly as it does today: same ids, same
   Workspace domain rule, same callback URL.
4. One deployment can offer several providers at once, for example GitHub,
   Google and a company Entra tenant.
5. The access policies (`[access]`) keep their meaning. An email or
   `@domain` entry matches any provider that yields verified addresses.
6. Fixes that apply to every provider: provider-bound state, the RFC 9207
   `iss` check, HTTPS only, no redirects followed, capped response sizes.
7. A deployment that de-provisions people through its provider can cap how
   long a session outlives that, with one setting.

## Non-goals

- Microsoft sign-in on librepaper.org for any Microsoft account. That needs
  a multi-tenant app registration and a different email trust rule. See
  "Later".
- Personal Microsoft accounts (outlook.com, hotmail.com) as their own
  provider. They can still sign in as guests of a configured tenant.
- Group or role claims. An Entra operator limits sign-in to a group with
  the app registration's "Assignment required" setting instead.
- SAML, LDAP, Kerberos and Active Directory, with or without AD FS. AD FS
  sends no `email_verified`; see "Decided".
- OIDC sign-in for the admin console.
- Single sign-out or back-channel logout. A session disabled at the
  provider lasts until it expires. See "Known limits".
- Verifying id token signatures (JWKS). See "Security model".
- Dynamic client registration.

## What exists today

Read these before changing anything.

- `crates/librepaper-base/src/auth/mod.rs`
  - `PROVIDER_GITHUB = "github"`, `PROVIDER_GOOGLE = "google"`.
  - `Identity { provider, id, handle, name, picture, session_generation }`.
  - `Identity::github` and `Identity::google` build identities.
    `qualified(provider, id)` passes through any id that already has a
    colon.
  - `Policy::allows(handle)` matches handles against `[access]` entries. A
    handle with `@` is an email, one without is a GitHub login.
  - `read_identity` parses the session cookie
    `provider|handle|id|generation|picture|name|expiry` and refuses any
    provider other than the two constants.
  - `valid_agent_grant` also lists the two constants.
  - `acceptable_picture` keeps only short `https://` picture URLs.
- `crates/librepaper-base/src/auth/google.rs` (311 lines)
  - Code flow with PKCE, scopes `openid email profile`,
    `prompt=select_account`.
  - Reads `userinfo`, ignores the id token.
  - Requires `email_verified`. Refuses a non-Gmail address unless the `hd`
    claim equals the address domain (`UNVERIFIED_WORKSPACE_DOMAIN`).
  - Owns `pkce_verifier` and `pkce_challenge`, which GitHub also uses.
- `crates/librepaper-base/src/auth/github.rs`: OAuth2 with PKCE, then the
  GitHub `/user` API.
- `crates/librepaper-server/src/server/signin.rs`
  - `providers()`, `/auth/login`, `/auth/login/github`,
    `/auth/login/google`, `/auth/callback` (GitHub),
    `/auth/callback/google`, `/auth/logout`, `/auth/device`, `/api/me`.
  - State cookie value: `state|urlescaped-next|verifier`, 600 s, shared by
    both providers and not bound to either.
  - `sign_in` upserts the account with `provider`, `provider_subject` (the
    qualified id, for example `google:1076...`), `handle`, `display_name`
    and `email`.
- `crates/librepaper-server/src/server/origins.rs`: `callback_url()` is
  `/auth/callback`, `google_callback_url()` is `/auth/callback/google`.
- `crates/librepaper-server/src/server/mod.rs:1180`: `provider_configured`
  decides whether a signed-in identity may publish. `Server.app` is the
  `GithubApp` and `Server.google` the `GoogleApp`.
- `crates/librepaper-server/src/server/graphs/routes.rs:135`, `:186`, `:319`
  and `:328`: the admin console calls `server.app` directly (`configured`,
  `authorize_url`, `exchange`, `login`). `/api/auth/config` reads
  `self.app.client_id`. Seven test files build a `GithubApp` and assign
  `server.app`: `http_test_support.rs`, `admin_http_tests.rs`,
  `comment_http_tests.rs`, `moderation_http_tests.rs`,
  `isolation_http_tests.rs`, `mcp/recovery_tests.rs` and `routes.rs:1053`.
  This is why `Server.app` stays (see "Provider model").
- `crates/librepaper-server/src/server/metrics.rs:484`: `route_class` files
  every `/auth/` path under one class, `auth`. Nothing per provider.
- `crates/librepaper-base/src/http.rs`: one shared `reqwest::Client` that
  follows redirects. reqwest 0.13 with the `rustls` feature verifies TLS
  with `rustls-platform-verifier`, which reads the system trust store on
  Linux and honours `SSL_CERT_FILE`.
- `crates/librepaper-server/src/server/serve.rs`: sign-in advice warnings
  (around line 225) and the client id and secret pairing check (around line
  336).
- `crates/librepaper/src/cli/server_config.rs`: `[auth.github]` and
  `[auth.google]` tables, each `client_id` and `client_secret`, unknown
  keys refused.
- `crates/librepaper-engine/migrations/postgres/0001_catalog.sql`:
  `accounts.provider` and `accounts.provider_subject`, unique together for
  registered accounts. No check constraint on `provider`.
- `crates/librepaper-engine/migrations/postgres/0002_banned_handles.sql`:
  `provider IN ('github', 'google')`.
- `crates/librepaper-engine/src/storage/postgres/moderation.rs:276`: the
  same two names in `block_or_ban`.
- `crates/librepaper-server/src/server/graphs/assets/index.html:51` and
  `app.js:250`: the admin console's provider select and the code that
  prefills it.
- `web/src/components/SignIn.svelte`: a hardcoded `["github", "google"]`
  list with a GitHub icon and the Google "G". `Icon.svelte` has no generic
  sign-in icon yet. `Nav.svelte:50` and `CompactPanelMenu.svelte:136` test
  `me.provider === "github"` to prefix `@`; every other provider falls
  through to the plain name, which is right for OIDC.
- `deploy/librepaper.toml`, `docs/host/simple.md`, `docs/host/advanced.md`.

## Provider model

A provider has a **kind**, an **id** and a **label**.

| Kind | Id | Config table | Label default | Flow |
|---|---|---|---|---|
| `github` | `github` | `[auth.github]` | GitHub | OAuth2, `/user` API |
| `google` | `google` | `[auth.google]` | Google | OIDC, Google preset |
| `entra` | `entra` | `[auth.entra]` | Microsoft | OIDC, Entra preset |
| `oidc` | `oidc-<slug>` | `[auth.oidc.<slug>]` | required `label` | OIDC, no preset |

- The id is what the session cookie, `accounts.provider`, the qualified
  account id (`<id>:<subject>`), `banned_handles.provider`, the routes and
  `/api/me` all use.
- `github` and `google` keep their current ids, so nothing stored changes.
- A custom id always starts with `oidc-`. A future preset can then take any
  plain name without colliding with an operator's slug.
- There is at most one `[auth.github]`, one `[auth.google]` and one
  `[auth.entra]`. There can be up to 8 `[auth.oidc.<slug>]` tables.
- The order on the sign-in page is GitHub, Google, Entra, then custom
  entries sorted by slug.

In code, `Server.app: GithubApp` stays exactly as it is. The admin console,
`/api/auth/config` and seven test files use it directly, and nothing is
gained by moving it. `Server.google` is replaced by
`Server.oidc: Vec<OidcProvider>`, in sign-in page order:

```rust
pub struct OidcProvider {
    pub id: String,          // "google", "entra", "oidc-keycloak"
    pub label: String,
    pub preset: Preset,      // Google, Entra { tenant }, Custom
    pub issuer: String,
    pub client_id: String,
    pub client_secret: String,
    pub scopes: String,
    // test seam: when set, discovery is skipped
    pub endpoints: Option<Endpoints>,
    discovered: Arc<tokio::sync::OnceCell<Endpoints>>,
}
```

`Server::providers()` lists GitHub when `app.configured()`, then every entry
of `oidc`. `Server::oidc(id)` finds one entry. Every `match` on
`PROVIDER_GITHUB | PROVIDER_GOOGLE` becomes `PROVIDER_GITHUB`, else an
`oidc` lookup, or a syntax check (see "Session cookie"). There is no
`Provider` enum.

## Configuration

### Tables

```toml
# New, optional. How long a browser session and a CLI token last. Retention
# syntax: "24h", "7d". Default 30d, which is what every deployment has today.
[auth]
session_lifetime = "30d"

# Unchanged.
[auth.github]
client_id = "..."
client_secret = { file = "/run/secrets/github" }

# Keys unchanged. Now runs through the OIDC client.
[auth.google]
client_id = "..."
client_secret = "..."

# New.
[auth.entra]
tenant = "00000000-0000-0000-0000-000000000000"   # the Directory (tenant) ID
client_id = "..."                                 # the Application (client) ID
client_secret = "..."
label = "Contoso"                                 # optional, default "Microsoft"

# New. Any number up to 8.
[auth.oidc.keycloak]
issuer = "https://sso.example.org/realms/staff"
client_id = "librepaper"
client_secret = "..."
label = "Example SSO"                             # required
scopes = "openid email profile"                   # optional, this is the default
```

Every value accepts the same `Input` forms as other settings (literal,
`{ file = ... }`, environment), so secrets can live in files.

### Validation at load time

All of these refuse to start with a message naming the key.

- `client_id` and `client_secret` are both set and non-blank, for every
  table. Public clients without a secret are not supported.
- `[auth].session_lifetime` parses like a retention value (`parse_retention`
  in `librepaper-document`) and is between 1 hour and 30 days. `never`,
  `off` and zero are refused: "auth.session_lifetime must be between 1h and
  30d".
- `[auth.entra].tenant` is a GUID. It is lowercased at load, because
  Entra's discovery document writes the GUID in lowercase in its issuer and
  the issuer comparison is exact. A domain name such as
  `contoso.onmicrosoft.com` is refused with: "auth.entra.tenant must be the
  Directory (tenant) ID, a GUID; a domain name produces a different issuer".
  The values `common`, `organizations` and `consumers` are refused with:
  "auth.entra.tenant must name one tenant; multi-tenant sign-in is not
  supported".
- `[auth.oidc.<slug>]`:
  - The slug matches `^[a-z][a-z0-9-]{0,23}$`.
  - `issuer` parses as a URL with no query and no fragment.
  - `issuer` is `https://`, or `http://` on a loopback host (`localhost`,
    `127.0.0.1`, `::1`).
  - `label` is 1 to 40 characters with no control characters.
  - `scopes` contains `openid`. It is at most 200 characters.
  - At most 8 such tables.
  - No two OIDC providers (Google, Entra and custom entries together) share
    an issuer after trimming one trailing `/`.
- Unknown keys are refused, as today.

### Values the server derives

| Preset | Issuer | Scopes |
|---|---|---|
| Google | `https://accounts.google.com` | `openid email profile` |
| Entra | `https://login.microsoftonline.com/<tenant>/v2.0` | `openid email profile` |
| Custom | `issuer` as configured | `scopes`, default `openid email profile` |

### Startup advice (serve.rs)

These replace the three current warnings.

- A policy names a GitHub login and there is no GitHub provider: unchanged.
- A policy names an email or `@domain` and no provider yields email
  handles (none of Google, Entra or a custom entry): replaces the
  Google-only warning.
- No provider at all: unchanged, but list every callback URL the operator
  would register.
- One info line per provider at startup: id, label, issuer and callback
  URL. The secret is never printed. Every `Debug` impl redacts it, as
  `GoogleApp` does today.
- One info line with the session lifetime.

## The OIDC flow

Every OIDC provider runs these steps. Presets only change step 7 and the
subject rule in step 8.

### 1. Discovery

- `GET <issuer>/.well-known/openid-configuration`, with one trailing `/`
  trimmed from the issuer first.
- Done on first use, not at boot, so a provider that is down does not stop
  the server. A success is cached for the life of the process. A failure is
  not cached, so the next sign-in tries again.
- Concurrent first uses share one fetch (`OnceCell::get_or_try_init`).
- The fetch takes the provider-request permit (`try_provider_request`),
  like every other call to a provider. No permit: the same "busy" refusal
  the token exchange gives today.
- The document's `issuer` must equal the configured issuer, again after
  trimming one trailing `/` on both. Otherwise discovery fails with a
  message naming both values.
- `authorization_endpoint` and `token_endpoint` are required.
  `userinfo_endpoint` is optional.
- Every endpoint must pass the same HTTPS-or-loopback rule as the issuer.
- If `code_challenge_methods_supported` is present and lacks `S256`,
  discovery fails. PKCE with S256 is required.
- `authorization_response_iss_parameter_supported` is read and kept for
  step 4.

### 2. Start (`GET /auth/login/<id>`)

- Unknown or unconfigured id: 404 "this deployment has no such sign-in".
- `next` is cleaned with `local_path`, as today.
- Fresh `state` (`random_token`) and PKCE verifier (`pkce_verifier`).
- The state cookie is set (see "State cookie") and the browser is
  redirected to the authorization endpoint with:
  - `client_id`, `redirect_uri` (see "Routes"), `response_type=code`,
    `scope`, `state`, `code_challenge`, `code_challenge_method=S256`.
  - `prompt=select_account` for Google and Entra, as Google does today.
    Custom providers get no `prompt`, because some IdPs reject values they
    do not know. There is no setting for it.
  - Any query the authorization endpoint already carries is kept.
- If discovery fails: 502 with "the sign-in provider is unavailable; try
  again shortly", and a `warn` log line with the reason.

### 3. Callback (`GET /auth/callback/<id>`)

- Method other than GET: 405 with `allow: GET`.
- Read and parse the state cookie. Missing: 400 "sign-in expired; try
  again". Malformed, or `state` does not match the query: 400 "sign-in
  state did not match; try again".
- The cookie's provider id must equal the callback's provider id.
  Otherwise: 400 "sign-in state did not match; try again".
- If the query carries `error`, show 400 "<label> refused the sign-in:
  <error>". The `error_description` value is not shown, because it is
  provider-controlled text. It is logged at `info`, truncated to 200
  characters.
- `code` missing or empty: 400 "<label> sent no code; try again".

### 4. RFC 9207 `iss` check

- If the callback query has `iss`, it must equal the discovered issuer
  (exact string). Otherwise: 400 "sign-in state did not match; try again".
- If discovery said `authorization_response_iss_parameter_supported: true`
  and the query lacks `iss`: same refusal.
- Google preset: Google does not send `iss` on the redirect today, and its
  discovery does not claim support, so this check is a no-op there unless
  Google changes.

### 5. Token exchange

- `POST` to the token endpoint, form-encoded: `grant_type`,
  `code`, `redirect_uri`, `code_verifier`, `client_id`, `client_secret`
  (`client_secret_post`).
- If discovery lists `token_endpoint_auth_methods_supported` without
  `client_secret_post` but with `client_secret_basic`, send the client
  credentials as HTTP Basic instead. Otherwise post them. OIDC Discovery
  says the default for an absent list is Basic; posting is a deliberate
  deviation, because every provider named here accepts it and Google does
  it today. For Basic, the id and the secret are form-urlencoded before
  they are base64-encoded (RFC 6749 2.3.1).
- Reply must be 200 with JSON containing `id_token`. An `access_token` is
  kept only for step 7's userinfo fallback.
- On failure: 400 "<label> refused the sign-in". The provider's
  `error_description` is logged, not shown.

### 6. Id token checks

The id token is a JWT. The server splits it on `.`, requires three parts,
base64url-decodes the middle part (padding optional) and parses it as JSON.
The signature part is not checked. Then:

- `iss` equals the discovered issuer. Google preset: `accounts.google.com`
  (no scheme) is also accepted, because Google documents both forms.
- `aud` is the client id, or an array containing it.
- If `azp` is present, it must equal the client id. If `aud` is an array
  with more than one value, `azp` must be present.
- There is no `nonce`. The server fetches the token itself, with its own
  PKCE verifier, so the id token is already bound to the sign-in that
  started it. `nonce` guards tokens that come back through the browser,
  which never happens here.
- `exp` is a number greater than now minus 60 s.
- `iat` is a number less than now plus 300 s.
- `sub` is a non-empty string, at most 255 bytes, with no control
  characters and no `|`.
- The header's `alg` is not read. Signature-less verification makes it
  irrelevant, and reading it invites `alg=none` confusion in later edits.
- Any failure: 502 "<label> would not say who you are", with the failing
  check logged at `warn`.

### 7. Claims and preset rules

Claims come from the id token. If `email` is missing there and discovery
gave a `userinfo_endpoint` and the token reply gave an `access_token`, the
server calls userinfo once:

- `GET`, `authorization: Bearer <access_token>`, `accept: application/json`.
- Status must be 200.
- The userinfo `sub` must equal the id token `sub` (OIDC Core 5.3.2).
  Otherwise: 502 "<label> would not say who you are".
- Only `email`, `email_verified`, `name`, `preferred_username` and
  `picture` are taken from userinfo, and only where the id token lacks
  them. `iss`, `aud`, `sub`, `tid`, `oid` and `hd` always come from the id
  token.

`email_verified` is read leniently: `true`, `"true"` (any case, trimmed)
are true. Anything else, including absence and `null`, is false.

The email is trimmed and lowercased. It must have exactly one `@`, a
non-empty local part, a domain with at least one `.`, no `|`, no
whitespace or control characters, and at most 254 bytes. An address that
fails these is refused as unverified.

Then the preset decides:

**Google**

- `email_verified` must be true. Otherwise 403 `UNVERIFIED_EMAIL`, with the
  text unchanged from today.
- Domain `gmail.com` or `googlemail.com`: accepted.
- Any other domain: the id token's `hd` must equal the domain,
  case-insensitively. Otherwise 403 `UNVERIFIED_WORKSPACE_DOMAIN`, text
  unchanged.

**Entra**

- `tid` must equal the configured tenant GUID, case-insensitively. The
  issuer check already implies this. The second check costs nothing and
  protects against a later edit that loosens the issuer rule. Failure: 403
  "this Microsoft account does not belong to the organisation this
  deployment admits".
- `oid` must be a GUID. Failure: 502 "Microsoft would not say who you are".
- `email` must be present. `email_verified` is ignored, because Entra does
  not send it. Missing email: 403 "this Microsoft account has no email
  address; ask your IT department to set one". There is no fallback to
  `preferred_username`.
- Trust note: within a single tenant, the tenant's administrators set
  users' addresses. They are trusted to do so honestly. See "Security
  model".

**Custom**

- `email_verified` must be true. Otherwise 403 "this <label> account has no
  verified email address, so it cannot be signed in".
- There is no way to trust an unverified address. Keycloak, Authentik,
  Okta, GitLab and Zitadel all send `email_verified`. AD FS does not and is
  not supported until somebody asks. Entra does not either, and has its
  preset.

### 8. Identity

| Preset | Subject | Account id |
|---|---|---|
| Google | `sub` | `google:<sub>` |
| Entra | `oid` | `entra:<oid lowercased>` |
| Custom | `sub` | `oidc-<slug>:<sub>` |

- Entra uses `oid`, not `sub`, because Entra's `sub` is different for each
  app registration. With `sub`, re-registering the app would orphan every
  account. `oid` is stable for a user within a tenant.
- The id is always built as `format!("{id}:{subject}")`. It never goes
  through `qualified`, which passes through ids that already have a colon.
  A subject may contain colons.
- `handle`: the normalized email.
- `name`: `name`, else `preferred_username`, else the email's local part.
  If the chosen value contains `@`, its local part is used instead, because
  `preferred_username` is often the address on custom providers. Trimmed,
  control characters removed, at most 100 characters. The name is never the
  full email, as today.
- `picture`: `acceptable_picture(picture)` for Google and custom entries.
  Entra gives no picture, because its `picture` is a Graph URL that needs a
  bearer token and cannot be shown in an `<img>`.

`Identity::google` and `Identity::github` are replaced by one
`Identity::from_provider(provider_id, subject, handle, name, picture)`. It
is used by both adapters and does the trimming and length limits.

### 9. Finish

`Server::sign_in` stays the one place that sets a session cookie. It
already upserts the account and checks its status. It clears the state
cookie, as today. Nothing about issuers is checked here; that happens once
at startup (see "Database").

## GitHub adapter

GitHub keeps its flow, with three changes:

- It stays in `Server.app`, and `/auth/login/github` and `/auth/callback`
  use the shared helpers in `signin.rs` for the state cookie, `next`
  handling and errors. The helpers take a provider id and a callback URL,
  not a `Provider` value.
- Its state cookie carries the provider id like every other provider, so a
  GitHub sign-in can no longer finish at an OIDC callback, or the reverse.
- `pkce_verifier` and `pkce_challenge` move from `google.rs` to a new
  `auth/pkce.rs`.

The callback stays at `/auth/callback`, so no registered GitHub OAuth app
needs editing. `/auth/callback/github` is not added.

## Routes

| Route | Provider |
|---|---|
| `GET /auth/login` | The door, unchanged: one provider redirects, several show the page. |
| `GET /auth/login/<id>` | Start, for any configured id. |
| `GET /auth/callback` | GitHub only, unchanged. |
| `GET /auth/callback/<id>` | Every OIDC provider, for example `/auth/callback/google`, `/auth/callback/entra`, `/auth/callback/oidc-keycloak`. |

- `Arrival::callback_url()` stays for GitHub. `Arrival::oidc_callback_url(id)`
  replaces `google_callback_url` and builds `/auth/callback/<id>`. The
  redirect URI sent in step 2 and step 5 comes from this one function.
- The plain-text sign-in page (callers without `text/html`) lists every
  configured `/auth/login/<id>`.
- `/auth/login` with no providers: same 404, with the text changed to "this
  deployment has no sign-in: reading and commenting remain available, but
  publishing requires signing in".
- `metrics.rs`, `cost.rs` and `routes.rs` already treat every `/auth/` path
  the same: one route class `auth`, one cost rule, one routing rule. Leave
  them alone.

## State cookie

```
<provider id>|<state>|<urlescaped next>|<verifier>
```

- Same name (`librepaper_state`, `__Host-` on HTTPS), same flags, same 600 s
  lifetime as today.
- Parsed with `splitn(4, '|')`. All four fields must be non-empty except
  `next`.
- The provider id must equal the route's provider.
- Sign-ins already in progress during the deploy carry the old three-field
  cookie. They fail once with "sign-in state did not match; try again".
  This is accepted. There is no shim.

## Session cookie and device tokens

- The format is unchanged: `provider|handle|id|generation|picture|name|expiry`.
  Every existing GitHub and Google session stays valid.
- `read_identity` and `valid_agent_grant` stop matching a fixed list. They
  accept a provider that matches `^(github|google|entra|oidc-[a-z][a-z0-9-]{0,23})$`.
  The cookie is HMAC-signed, so this is a sanity check, not a trust
  boundary.
- Whether the provider is still configured stays the server's decision.
  `provider_configured(&Identity)` becomes `self.app.configured()` for
  `github`, else `self.oidc(&id.provider).is_some()`.
  As today, a session from a provider that was removed can still read and
  comment where policy allows, but cannot publish.

## Session lifetime

A session is a signed cookie that the server never looks up, so nothing asks
the provider again once somebody is in. A person disabled at the provider
keeps access until the cookie expires. Re-checking at the provider needs a
refresh token, and that means server-side storage, `offline_access` consent
on every provider and a new consent screen for every Google account on
librepaper.org. That stays in "Later". What ships now is a bound:

- `auth.session_lifetime` (see "Configuration") replaces the fixed
  `SESSION_MAX_AGE` as the browser cookie's lifetime. `SESSION_MAX_AGE`
  stays as the default value and the upper bound.
- A device token lasts `min(DEVICE_TOKEN_MAX_AGE, session_lifetime)`. With
  the default, that is the 90 days of today.
- The inactive-account erasure floor (`parse_inactive_after` in `serve.rs`)
  is twice the configured lifetime instead of twice the constant.
- Sessions do not renew on use. Somebody cut off mid-edit sees the reader's
  "Sign in required" or a stalled room socket, signs in again and resyncs.
  Sliding renewal is a follow-up if a fixed day proves annoying.
- The admin console's own 12 hour session is unchanged.
- The docs tell Entra and custom operators to set a working day. Re-signing
  in through SSO is one click, because the provider keeps its own session.
- The setting is deployment-wide, not per provider. A per-provider value
  would leave a self-hosted Google Workspace deployment with no bound and
  hide an asymmetry nobody can see in the config.

## Database

One migration, `0004_oidc_providers.sql`:

```sql
-- Each OIDC provider id is bound to the issuer it was first started with.
-- A subject is unique only within its issuer, so pointing an existing id at
-- another issuer would hand old accounts to whoever holds the same subject
-- there. The server refuses to start instead.
CREATE TABLE sign_in_issuers (
    provider text PRIMARY KEY,
    issuer text NOT NULL,
    bound_at timestamptz NOT NULL DEFAULT now()
);

-- Refusals may name any provider id the server accepts.
ALTER TABLE banned_handles DROP CONSTRAINT banned_handles_provider_check;
ALTER TABLE banned_handles ADD CONSTRAINT banned_handles_provider_check
    CHECK (provider ~ '^(github|google|entra|oidc-[a-z][a-z0-9-]{0,23})$');
```

Confirm the generated constraint name from 0002 in a real database before
writing the migration. Postgres names an inline column check
`<table>_<column>_check`.

Issuer binding:

- At startup, after migrations, for each configured OIDC provider:
  `INSERT INTO sign_in_issuers (provider, issuer) VALUES ($1, $2) ON CONFLICT DO NOTHING`,
  then read the row back.
- If the stored issuer differs from the configured one (after trimming one
  trailing `/`), refuse to start with: "auth provider <id> was bound to
  issuer <old> and is now configured with <new>. Existing <id> accounts
  belong to the old issuer. Restore the old issuer, or give the new one a
  different id."
- Google's row is `https://accounts.google.com`. It is written on the first
  boot after deploy and never changes.
- Rows are never deleted. Removing a provider from config leaves its row,
  so adding it back later with another issuer is still refused.
- Rebinding on purpose is a manual `DELETE` documented in
  `docs/host/advanced.md`. It is safe when the new issuer is the same
  identity provider at a new address, so every subject stays the same:
  Keycloak dropped its `/auth` path prefix in version 17, and a hostname
  move does the same thing. It hands old accounts to strangers when the new
  issuer is a different provider.

Moderation:

- `block_or_ban` accepts any provider id that passes the regex above,
  instead of the two names.
- The admin console's provider select is filled from the server's
  configured providers (send them in the existing `/data` or `/session`
  reply), not from hardcoded options.
- `app.js:250` (`blockAccount`) prefills the provider select only for
  `github` and `google`. It prefills any provider id present in the
  select instead.

Run `cargo sqlx prepare` after the query changes. The offline query data is
checked in.

## `/api/me` and the web app

`providers` changes from a list of ids to a list of objects:

```json
"providers": [
  {"id": "github", "kind": "github", "label": "GitHub"},
  {"id": "google", "kind": "google", "label": "Google"},
  {"id": "entra", "kind": "entra", "label": "Contoso"},
  {"id": "oidc-keycloak", "kind": "oidc", "label": "Example SSO"}
]
```

- This is a hard cutover. Every reader of `providers` is updated in the same
  change:
  - `web/src/components/SignIn.svelte` renders the list from the server
    with no hardcoded ids. The link is `/auth/login/<id>?next=...`. The
    text is "Continue with <label>".
  - Icons: `github` uses the existing GitHub icon. `google` uses
    `/assets/google-g.svg`. `entra` and `oidc` use one generic sign-in
    icon, a key, added to `Icon.svelte` (it has none today). No Microsoft
    logo is shipped.
  - `Nav.svelte`, `CompactPanelMenu.svelte`, `Landing.svelte` and
    `Reader.svelte` only read `providers?.length`. Update the JSDoc
    typedefs (`Reader.svelte:122`, `SignIn.svelte:10`, `lib/api.js`
    comments) and leave the logic.
- The label is operator text. Svelte escapes it. It is never put into
  `{@html}`.
- `provider` on a signed-in caller stays the id string.
- `/api/auth/config` is unchanged. It serves the GitHub client id to old
  CLI clients.

## Policies

- `Policy::allows` is unchanged. A handle with `@` is an email from any
  email provider. A handle without `@` is a GitHub login.
- `any` means any signed-in account on any configured provider. With a
  custom provider or Entra, that is everyone the provider admits. The docs
  say so.
- `anygithub` (`github_only`) is unchanged.
- `names_an_email_or_domain` advice now checks for any email provider.
- Comment on `Policy::allows` and on `Identity`: replace "a Google
  account's verified email" with "an email address a configured provider
  vouches for".

## Admin console

No change to how it signs in. `[admin] github_ids` and the GitHub OAuth
app remain required for it. The only admin changes are the moderation
provider list described in "Database".

`/auth/logout` keeps ending admin sessions for GitHub identities only.

## Security model

Who is trusted with what:

| Provider | Trusted to name | Why that is safe |
|---|---|---|
| GitHub | A login and a numeric id | Unchanged. |
| Google | A verified address | Gmail addresses Google verified. Other domains only when Google Workspace owns the domain (`hd`). Unchanged. |
| Entra | An address | One tenant, pinned by issuer and `tid`. The tenant's administrators set addresses. The operator chose that tenant. |
| Custom | A verified address | The operator chose the provider and runs it, or trusts whoever does. Only addresses the provider marks verified count. |

The consequence the docs must state plainly: **an email or `@domain` entry
in `[access]` admits whoever any configured email provider says owns that
address.** Turning on a provider means trusting its administrators with
every address in your allowlists. For example, with Entra for `@contoso.com`
staff and an allowlist entry `alice@gmail.com`, a Contoso tenant
administrator could set a Contoso user's address to `alice@gmail.com` and
publish as Alice. Operators who mix providers should prefer `@domain`
entries for domains the provider actually owns.

Threats and what stops each one:

| Threat | Mitigation |
|---|---|
| Login CSRF (attacker's code lands in a victim's browser) | `state` in an HttpOnly, `__Host-` cookie, compared on callback. |
| Stolen authorization code | PKCE S256, verifier only in the HttpOnly cookie. |
| Mix-up between providers | Provider id in the state cookie, per-provider callback path, RFC 9207 `iss` check where offered. |
| Forged discovery document | Issuer equality check. HTTPS only. Issuer is operator config, never user input. |
| Forged id token | Received only from the token endpoint, over TLS, in reply to a request authenticated with the client secret (OIDC Core 3.1.3.7). `iss`, `aud`, `azp`, `exp`, `iat` checked. No `nonce`, because the token never passes through the browser. |
| Token for another client replayed | `aud` and `azp` checks. |
| Userinfo for another user | Userinfo `sub` must equal id token `sub`. |
| Subject collision across issuers | Ids are prefixed by provider id. Each id is bound to one issuer in `sign_in_issuers`. |
| Cookie field injection | `|` refused in subject, email and picture. Names cannot shift fields because the parser splits from the left with a fixed count and from the right for expiry, as today. |
| SSRF through provider URLs | Endpoints come only from operator config and a discovery document that passed the issuer check. Redirects are not followed. |
| Oversized or slow provider replies | 15 s timeout per call (existing `PROVIDER_HTTP_TIMEOUT`), 64 KiB body cap, existing provider-request semaphore (`try_provider_request`). |
| Provider error text shown to users | `error_description` is logged, never rendered. |
| Secrets in logs | Every config and provider struct redacts secrets in `Debug`. A test checks each one. |
| Unverified Entra addresses (the nOAuth class) | Single tenant only. Multi-tenant values refused at load. |

HTTP client for provider calls:

- A dedicated `reqwest::Client` in `librepaper-base::http`, built with
  `redirect::Policy::none()`. The shared `client()` follows redirects and
  must not be used for provider calls.
- Responses are read with a 64 KiB cap. A larger body is an error.
- GitHub's adapter switches to this client too.
- TLS roots come from `rustls-platform-verifier`, which reqwest's `rustls`
  feature already selects: the system store on Linux, honouring
  `SSL_CERT_FILE`. A self-hosted provider behind a private CA works once
  the CA is in the container's store. No `ca_file` setting is added;
  `docs/host/advanced.md` shows the mount.

Why the id token signature is not verified: the token comes straight from
the token endpoint over TLS, in answer to a request authenticated with the
client secret. Checking the signature would add JWKS fetching, key
rotation, caching and an `alg` allowlist. That code would protect against
an attacker who can already forge TLS to the provider, and such an
attacker could forge the discovery document too. The current Google code
relies on the same argument with userinfo. If the id token is ever accepted
from the browser (implicit flow, front-channel), this decision must be
revisited. The code comment at the id token parser says so.

## Error messages

Shown to the person signing in. Plain sentences, no provider internals.

| Case | Status | Text |
|---|---|---|
| Unknown provider | 404 | this deployment has no such sign-in |
| Discovery failed at start | 502 | the sign-in provider is unavailable; try again shortly |
| State cookie missing | 400 | sign-in expired; try again |
| State mismatch, provider mismatch, `iss` mismatch | 400 | sign-in state did not match; try again |
| `error` in callback | 400 | <label> refused the sign-in: <error> |
| No code | 400 | <label> sent no code; try again |
| Token exchange failed | 400 | <label> refused the sign-in |
| Id token or userinfo check failed | 502 | <label> would not say who you are |
| Google unverified | 403 | existing `UNVERIFIED_EMAIL` |
| Google Workspace domain | 403 | existing `UNVERIFIED_WORKSPACE_DOMAIN` |
| Entra wrong tenant | 403 | this Microsoft account does not belong to the organisation this deployment admits |
| Entra no email | 403 | this Microsoft account has no email address; ask your IT department to set one |
| Custom unverified | 403 | this <label> account has no verified email address, so it cannot be signed in |
| Account blocked | 403 | this account is not active (unchanged) |

`<error>` is the OAuth error code from the query. It is limited to
`[a-z_]{1,40}` before it is shown, and replaced by `error` otherwise.

## Docs

Follow the docs memory rules: short bullets, `sh` and `toml` blocks with `#`
comments, plain sentences, no em or en dashes.

- `docs/host/simple.md`, section "Sign-in":
  - GitHub and Google: unchanged.
  - New "Microsoft Entra ID":
    - Register an app: Entra admin center, App registrations, New
      registration, single tenant.
    - Redirect URI (Web): `https://paper.example/auth/callback/entra`.
    - Certificates and secrets: new client secret. Note its expiry date.
      Sign-in stops when it expires.
    - The `email` scope alone yields the `email` claim on the v2.0 endpoint.
      If a sign-in is refused for a missing address, add the optional
      claim `email` to the ID token under Token configuration.
    - Copy the Directory (tenant) ID and the Application (client) ID.
    - Optional: Enterprise applications, the app, Properties, "Assignment
      required" = Yes, then assign the groups that may sign in.
    - Guests invited to the tenant can sign in too.
    - Some tenants block user consent. An administrator then grants
      consent once for `openid`, `email` and `profile`.
    - The admin console still signs in with GitHub. An Entra-only
      deployment registers a GitHub OAuth app and lists the operator's
      GitHub id in `[admin]` to use it.
    - Set `session_lifetime = "24h"` under `[auth]`. A person removed from
      the tenant keeps access until their session expires; without the
      setting that is 30 days in the browser and 90 days for a CLI token.
  - New "Other OpenID Connect providers": the `[auth.oidc.<slug>]` table,
    the callback `https://paper.example/auth/callback/oidc-<slug>`, and
    one example each for Keycloak and GitLab. One line says a provider
    must mark addresses verified, and that AD FS is not supported.
  - "Known limits" in two lines: one person signing in through two
    providers gets two accounts, and a session outlives a disablement at
    the provider.
  - The trust paragraph from "Security model", in two sentences.
- `docs/host/advanced.md`: the issuer binding, how to rebind on purpose
  and when that is safe, the full key reference for the new tables and for
  `auth.session_lifetime`, and a private CA: mount the certificate into the
  container and set `SSL_CERT_FILE` in `compose.yaml`.
- `deploy/librepaper.toml`: commented `[auth.entra]` and
  `[auth.oidc.example]` blocks, and the callback list gains the two new
  shapes.

## Tests

The rule from AGENTS.md applies: subagents write tests and do not run them.
The main agent runs `cargo nextest run -p <crate>` per crate, then `make
test`, then the Postgres-gated suite with `tools/dev/db test`.

### Unit tests, `librepaper-base`

A stand-in OIDC provider (axum on `127.0.0.1:0`) serves discovery, a token
endpoint that returns a configurable unsigned id token, and userinfo. The
PR #44 test helper is a fine starting shape.

- Discovery: success; issuer mismatch refused; trailing slash tolerated on
  both sides; missing authorization or token endpoint refused; `http://`
  non-loopback endpoint refused; `S256` missing from a present
  `code_challenge_methods_supported` refused; a failed discovery retried on
  the next call; concurrent first calls fetch once.
- Authorize URL: carries every parameter; keeps an existing query on the
  endpoint; `prompt=select_account` for Google and Entra only.
- Token exchange: `client_secret_post` by default; Basic when discovery
  says only Basic; a redirect from the token endpoint is not followed; a
  body over 64 KiB is refused; `error_description` is not in the returned
  user-facing error.
- Id token: wrong `iss`; Google's scheme-less `iss` accepted for Google
  only; `aud` string and array; array without `azp`; wrong `azp` with a
  string `aud` and with an array; expired;
  `iat` in the future; empty, oversized, `|`-containing `sub`; not three
  parts; bad base64; not JSON.
- Userinfo fallback: used only when the id token lacks `email`; `sub`
  mismatch refused; does not override id token claims.
- `email_verified`: `true`, `"true"`, `"TRUE "`, `false`, `"false"`,
  `null`, absent.
- Email normalization and refusals: case, spaces, two `@`, no dot in
  domain, `|`, 255 bytes.
- Google preset: every current `google.rs` test, ported unchanged in
  meaning. Gmail verified; Gmail unverified; Workspace with matching `hd`;
  Workspace without `hd`; `hd` for another domain.
- Entra preset: matching `tid`; other `tid`; missing `oid`; uppercase `oid`
  lowercased; missing email; `email_verified` absent but accepted; id is
  `entra:<oid>` not `sub`.
- Custom preset: unverified refused; absent `email_verified` refused; id
  is `oidc-<slug>:<sub>`; a `sub` of `github:583231` produces
  `oidc-x:github:583231`, not a GitHub id.
- `Identity::from_provider`: name fallbacks, a `preferred_username` that
  is an address reduced to its local part, length limits, control
  characters, picture filtering.
- `read_identity`: every existing cookie test passes; an `entra` and an
  `oidc-x` cookie parse; `oidc-` alone, `OIDC-x`, `evil|x` refused.
- `Debug` redaction for `OidcProvider` and each config struct.

### Config tests, `librepaper` crate

- Each validation rule in "Validation at load time" has a passing and a
  failing case. An uppercase tenant GUID is accepted and comes out
  lowercased. `session_lifetime` accepts `24h` and `7d`, refuses `30m`,
  `31d`, `never` and `0`, and defaults to 30 days.
- The redacted config dump (`server_config.rs` around line 1071) shows the
  new tables without secrets.

### Server tests, `librepaper-server`

- `/auth/login` with one OIDC provider redirects to it; with several shows
  the page.
- Full sign-in against the stand-in for each preset, ending with a session
  cookie and an account row.
- State cookie from GitHub presented at `/auth/callback/google`: refused.
  And the reverse.
- Old three-field state cookie: refused.
- `iss` query mismatch refused; `iss` required when discovery says so.
- Provider `error` in the query: shown code is filtered, description not
  shown.
- `/api/me` returns provider objects in the right order with labels.
- `can_publish` false for a session whose provider was removed from config.
- With `session_lifetime` of two hours, a sign-in sets a cookie whose
  `Max-Age` is 7200 and whose signed expiry is two hours out, and a device
  token issued afterwards expires in two hours. With the default, the
  cookie lasts 30 days and the token 90.
- The admin console's GitHub sign-in tests pass unchanged.

### Postgres-gated tests

- Migration 0004 applies on a database with existing `banned_handles` rows.
- `sign_in_issuers`: first boot binds; same issuer boots; a changed issuer
  refuses to start with the message; Google binds to its constant.
- `block_or_ban` accepts `entra` and `oidc-x`, refuses `oidc-` and
  `x|y`.
- An existing `google:<sub>` account signs in through the new code and
  keeps its catalogue id.

### Browser test

- `web/tests/browser/signin-server-browser.mjs`: the page renders
  "Continue with <label>" for every provider object, and the links point
  at `/auth/login/<id>`.
- `svelte-check` passes. It is part of `make test`.

### Manual checks before release

Done by the operator, recorded in the release notes.

1. Google on a local `admin serve` with a real Google client: Gmail
   account; Workspace account; same catalogue id as before for an existing
   account.
2. Entra with a free Microsoft 365 developer tenant: member user; guest
   user; user from another tenant refused; user without email refused with
   the IT message; check that `email` is in the id token with the `email`
   scope alone, and that userinfo fills it when a tenant strips it.
3. Keycloak in a container (`quay.io/keycloak/keycloak`, `start-dev`):
   verified and unverified users.
4. GitLab.com as a custom provider: a normal account.
5. GitHub: unchanged sign-in and admin console sign-in.

## Rollout on librepaper.org

librepaper.org uses GitHub and Google. Nothing is added there.

- Google's redirect URI does not change. The Google Cloud console needs no
  edit.
- Existing sessions stay valid. Sign-ins in flight during the deploy fail
  once.
- Migration 0004 runs at boot. The first boot writes the Google issuer row.
- Before the deploy, sign in with Google on a staging `admin serve` that
  uses a copy of production's account rows, and confirm the catalogue id
  is unchanged.
- Bump the deploy kit pins with `tools/release/version`, as every release.

## Work split

Each piece gets its own worktree and branch, on the cheapest capable model.
Briefs must say: do not run tests, builds, linters or formatters; report
the files changed. Use straight quotes in code.

1. **base: OIDC core.** New `auth/oidc.rs` (discovery, authorize URL, token
   exchange, id token checks, userinfo fallback, presets), new
   `auth/pkce.rs`, the no-redirect capped client in `http.rs`,
   `Identity::from_provider`, `read_identity` and `valid_agent_grant`
   regex, delete `google.rs`, move Google tests. Unit tests listed above.
   This is the largest piece and the one to review most closely. It goes
   to Sonnet, not Haiku: it is the security core, and the Haiku pitfalls
   on record (curly quotes, briefs pasted as comments) are not worth the
   saving here.
2. **base: GitHub adapter.** `github.rs` uses `pkce.rs`, the new client and
   `Identity::from_provider`. Depends on piece 1's signatures. Brief it
   with the exact signatures from the spec so it can run in parallel.
3. **cli: config.** `server_config.rs` tables, validation, redacted dump,
   `auth.session_lifetime`, config tests. Produces the `Vec<OidcProvider>`
   and the lifetime that piece 4 consumes through `ServeOptions`.
4. **server: plumbing.** `Server.oidc` and `providers()` in `mod.rs`,
   `signin.rs` rewrite (shared start and callback helpers, state cookie),
   `origins.rs` `oidc_callback_url`, `serve.rs` advice and startup lines,
   issuer binding at startup, the session lifetime on cookies and device
   tokens, server tests. `Server.app`, the admin console's sign-in and
   `metrics.rs` are not touched.
5. **engine: database.** Migration 0004, `sign_in_issuers` queries,
   `block_or_ban` regex, gated tests, `sqlx prepare` data (the main agent
   regenerates it).
6. **web.** `SignIn.svelte`, typedefs, generic icon, browser test, admin
   console provider select and `app.js`.
7. **docs.** `simple.md`, `advanced.md`, `deploy/librepaper.toml`.

Pieces 1, 3, 5, 6 and 7 can start together. Piece 2 and piece 4 start once
the piece 1 and piece 3 interfaces are fixed in their briefs. Review order
for the main agent: 1, 4, 5, then the rest.

Rough size: 1000 to 1300 added lines including tests, about 400 removed
(`google.rs`, duplicated callback code, hardcoded lists). The
security-relevant core is piece 1, around 350 lines without tests.

## Known limits

- A person disabled at the provider keeps an open session until it expires:
  `auth.session_lifetime`, 30 days by default. Operators who need a faster
  cut-off set a working day, or block the account in the admin console.
  Re-checking at the provider is "Later".
- Changing a custom provider's issuer means a new provider id and new
  accounts. Old projects stay with the old accounts. Transfer is manual.
- One Entra tenant per deployment. A second tenant needs a second custom
  entry with the tenant's v2.0 issuer, without the `tid` rule.
- Email addresses are trusted per provider, as described in "Security
  model". There is no cross-provider check.
- A provider that puts `|` in subjects cannot be used. None of the common
  ones do.
- One person who signs in through two providers has two accounts, each
  with its own projects, even with the same address on both. There is no
  account linking.
- The admin console signs in with GitHub only. A deployment without a
  GitHub OAuth app has no admin console.
- A provider that never sends `email_verified` cannot be used. AD FS is
  the one common case.

## Later

- OIDC for the admin console, keyed by provider id and subject. First,
  because an Entra-only deployment otherwise still needs GitHub.
- Multi-tenant Microsoft sign-in for librepaper.org. Needs the issuer
  template `https://login.microsoftonline.com/{tenantid}/v2.0`, a per-token
  issuer check against `tid`, and an email rule based on the `xms_edov`
  claim or on verified domains. Its own spec.
- Session re-check against the provider through refresh tokens. Needs
  `offline_access` on Entra and `access_type=offline` on Google, which adds
  a consent screen for every Google account on librepaper.org, and a place
  to keep the tokens, since the session cookie has no room for one.
- A second Entra tenant as a list under `[auth.entra]`.
- `email_trusted` on a custom entry, for AD FS, if somebody asks.
- A custom `prompt` value, if somebody asks.
- An `allowed_domains` setting per provider, so a provider can only produce
  addresses at domains it owns.

## Decided

- No `prompt` setting on custom entries. Google and Entra send
  `select_account`; custom entries send nothing.
- No `email_trusted`. It was the one setting that weakened a check, and
  nobody has asked for AD FS. Every other provider named here sends
  `email_verified`.
- `auth.session_lifetime` ships now, deployment-wide, default 30 days. The
  refresh-token re-check waits.
