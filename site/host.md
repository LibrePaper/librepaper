---
title: "Running a server"
---

## Deploy

LibrePaper is one static binary with everything compiled into it: the reader, the
renderers, and the server. Run it on your laptop for a quick trial, or on a
small host for something durable.

### Local

Run a public local instance with no GitHub setup at all:

```sh
librepaper admin serve --port 8081 --publishers YOUR-GITHUB-LOGIN
```

Open <http://localhost:8081>. Everything is stored in `librepaper-data` (see [Storage](#storage)).

### Self-managed server

Run the bundled server on your own host, with `--data-directory` set to a persistent
directory:

```sh
librepaper admin serve --port 8080 --data-directory /var/lib/librepaper --publishers YOUR-GITHUB-LOGIN
```

To let people sign in, set up a [GitHub app](#oauth) for this server's address.

If a separate marketing site stands in front of the deployment, name it with `--site-origin https://paper.example` (or `LIBREPAPER_SITE_ORIGIN`). Signing out goes there, because somebody who has just signed out is a stranger again and the site is what a stranger is shown. It must be an origin on its own: a scheme and a host, no path, and HTTPS unless it is loopback. Without the flag, signing out goes to the deployment's own front page, which is what a deployment that is its own front page wants.

Run the server behind a reverse proxy that terminates HTTPS, and have the proxy send the `X-Forwarded-Proto: https` header. That header is how the server knows its own address is an HTTPS one: without it the session cookie is not marked `Secure`, and uploads and comments are refused because the browser's idea of where the page came from does not match the server's. Plain HTTP is fine on `localhost` and nowhere else.

## Storage

`librepaper admin serve` keeps everything in the directory named by `--data-directory` or
`LIBREPAPER_DATA`, `librepaper-data` in the working directory by default: the
catalogue (`catalog.db`), the objects it names, private server state, and the
secrets that keep sessions and share links valid. Back it up if the instance
holds real work; `librepaper admin backup create` writes a verified recovery point of
all of it, and `librepaper admin backup restore` restores one into a fresh
directory.
See the [operator cost policy](https://github.com/LibrePaper/librepaper/blob/main/docs/cost-policy.md) for the complete defaults,
advanced YAML schema, the loopback `/api/status` endpoint, capacity accounting, and backup
reservations.

The storage flags bound what a deployment will store:

| Flag | Caps | Default |
| --- | --- | --- |
| `--document-size-limit` | combined source text of one document | 4 MB (maximum 8) |
| `--document-assets-limit` | combined input assets of one document | 32 MiB |
| `--publisher-storage-limit` | everything one publisher holds | 100 MB |
| `--deployment-storage-limit` | the whole deployment | 5120 MB |
| `--publisher-document-limit` | documents one publisher may hold | 50 |
| `--publisher-upload-limit` | uploads one publisher may make in an hour | 30 |

```sh
librepaper admin serve --document-size-limit 8 --document-assets-limit 16 --publisher-storage-limit 500 --deployment-storage-limit 10240
```

`--document-size-limit` may not be set above 8 MB. It bounds the text a person can see;
what has to be durably saved is the CRDT snapshot behind that text, which
carries the document's edit history and metadata as well, and this deployment
supports snapshots up to 16 MB. A document can therefore reach that second
ceiling without its visible text ever approaching the first -- an edit refused
for that reason says so, and says that the history counts too. A configuration
whose ceilings could accept work the journal could not durably save is refused
at startup rather than at the first save.

A document is a directory, so `--document-size-limit` bounds the sum of its texts and
`--document-assets-limit` bounds the combined input assets. Both count against `--publisher-storage-limit`; a figure is
an upload and counts against `--publisher-upload-limit` like any other. A Typst or
LaTeX document keeps source and input assets only. PDF and HTML output created
by a browser or companion is transient and never counts toward storage,
quotas, or uploads. On a deployment with many publishers,
`--document-assets-limit` is the one worth lowering:
figures are where a paper's bytes actually are, and it is what stops a single
document spending a publisher's whole allowance on images.

Publishing is always attributed to an authenticated Google or GitHub account
and charged against that account's quota. Anonymous readers and commenters do
not receive a publishing quota.

## Environment variables

Service settings that support environment variables follow the same name:
`--foo-bar` is `LIBREPAPER_FOO_BAR`, and the flag wins when both are set.
`librepaper admin serve --help` (and every other subcommand's `--help`) is the
reference for the full list. Advanced guardrails may be overridden in an
optional YAML file selected with `--config PATH` (or `LIBREPAPER_CONFIG`). The
file accepts only two top-level keys: `trusted_proxies`, the proxy list, and
`backup`, reporting metadata for operator-managed backups
(`destination_class`, `frequency` in seconds, `retained_count`, `encrypted`,
and `warning_count`); it does not schedule or delete backups. Omitted keys
retain their documented defaults.

These variables are useful in deployment files. Secrets are environment-only;
service settings have corresponding CLI flags, and installer variables control
the installation script:

| Variable | Purpose |
| --- | --- |
| `LIBREPAPER_GITHUB_CLIENT_SECRET` | GitHub OAuth app client secret |
| `LIBREPAPER_GOOGLE_CLIENT_SECRET` | Google OAuth client secret |
| `LIBREPAPER_BUDGET_DOCUMENT_ASSETS` | combined input assets per document, in MiB |
| `LIBREPAPER_LATEX_MIRROR` | HTTPS static mirror URL fetched directly by browsers |
| `LIBREPAPER_EXPIRE_AFTER` | delete documents after this duration, for example `24h` or `30d` (default: never) |
| `LIBREPAPER_EXPIRE_FROM` | whether that duration runs from `updated` (default) or `created` |
| `LIBREPAPER_TYPST_FONTS` | optional local directory of additional Typst fonts |
| `LIBREPAPER_CONFIG` | optional advanced YAML policy overrides |
| `LIBREPAPER_VERSION` | Version the installer fetches |
| `LIBREPAPER_BIN_DIR` | Installation directory the installer uses |

[^github-data]: LibrePaper requests no GitHub scopes through OAuth. It uses the
GitHub API only to obtain your public login name and account id; it does not
collect your email, repositories, or other profile data. The bar shows you your
own public avatar, fetched from GitHub by your browser.

## Fonts

A Typst document may name any font family. Families the compiler does not
embed come from the deployment's own library, a directory of `.ttf`/`.otf`
files:

```sh
librepaper admin serve --typst-fonts /srv/librepaper/fonts
```

The directory is read once at startup for the families each file carries, and
served by family at `/api/fonts/`. An editor's browser and a reader's browser
ask the same deployment for the same files, so a preview uses the same faces
a reader ends up seeing. Without it, a document naming a family the compiler
does not embed is set in Typst's default faces and warned about. Which fonts
a deployment offers, and under what licence, is the operator's decision.

## Rights

Two flags say who may do what.
`--publishers` says who may upload documents, and `--commenters` who may
annotate them. Both accept a comma-separated list of names:

```sh
librepaper admin serve --publishers alice,anne@example.org --commenters @example.org
```

A name is a GitHub login, a Google account's verified email address, or a whole
domain of them; the shape of the entry is what decides which, so the forms mix
freely in one list. `any` admits signed-in accounts; `anyone` is available only
for commenting:

| Value | Meaning |
| --- | --- |
| `alice` | the GitHub login `alice` |
| `alice@example.org` | the Google account whose verified email is that address |
| `@example.org` | any Google account on that domain |
| `any` | any signed-in account, on either provider |
| `anyone` | unsigned-in commenting; rejected for publishing |

A domain matches the part after the `@` exactly, so `@example.org` admits
`alice@example.org` and not `alice@mail.example.org`.

`--publishers` has no default: the server insists you say who may publish.
Publishing requires a Google or GitHub OAuth provider. Use `any` for any
authenticated account, or name specific accounts. The old `anyone` spelling is
rejected at startup.
`--commenters` defaults to `anyone`,
so readers can annotate a document straight from its link; use `any` to
attribute every comment to an account, or a list to keep a draft among named
reviewers.

Both flags apply to every document alike, and both are ceilings rather than the
last word: a document may name its own coauthors and reviewers with
[Share](collaborate/share.html), and may only ever be stricter than the server it is on.
Nothing a document says can widen `--publishers` or `--commenters`.

`--no-listing` turns the public front page off: the reserved examples stop
being listed to people who hold nothing on them, and nothing else was ever
listed to strangers.

Forwarded client identity is trusted only from networks listed in the advanced
configuration file. A single loopback proxy can use:

```yaml
trusted_proxies:
  - 127.0.0.1/32
  - ::1/128
```

The proxy must append the actual client address to `X-Forwarded-For` and
overwrite any incoming value. Without this setting, the TCP peer address is
used, so visitors behind one proxy share its IP based limits. The server walks
trusted proxy hops from right to left and stops at the first untrusted address.

Authentication hardening updates browser and terminal credentials to separate,
versioned signatures. After upgrading from unversioned credentials, sign in
again in the browser and run `librepaper login` for each terminal. Existing
anonymous visitor cookies retain their ownership and upgrade on the next page
visit. Logout removes the browser cookie; account session revocation is what
invalidates copies of issued credentials.

Google sign-in accepts verified Gmail addresses and Google Workspace accounts
whose hosted domain matches the email domain. Third-party addresses registered
with a personal Google account are refused because Google cannot establish
current ownership of those addresses. Use a supported Google account or GitHub.
The legacy `anygithub` policy remains restricted to GitHub; use `any` to admit
accounts from either provider. Allowlist contents appear in operator startup
logs; ordinary API responses show only a summary.

Device sign-in is served by the single local deployment process. Pending codes
do not survive a restart. The deployment writer lock prevents simultaneous
local servers.

## OAuth

A server that asks anyone to sign in needs at least one OAuth client of its
own, GitHub's or Google's. A server where both `--publishers` and
`--commenters` are `anyone` never asks, and runs without either.

Create the app at [github.com/settings/developers](https://github.com/settings/developers)
(New OAuth App). Point its two URLs at the server's own address: the
public HTTPS address it sits behind, with the same `/auth/callback` path:

```text
Homepage URL:               https://docs.example.org
Authorization callback URL: https://docs.example.org/auth/callback
```

Pass the client id to `admin serve` with `--github-client-id`, or its environment
variable `LIBREPAPER_GITHUB_CLIENT_ID`; the secret is environment only, since
an argument is visible in `ps` to every process on the machine and an
environment variable is not:

```sh
export LIBREPAPER_GITHUB_CLIENT_SECRET="..."
```

Readers can sign in with Google instead, or as well: create a *Web application* client at [console.cloud.google.com](https://console.cloud.google.com) under *Credentials*, with the authorised redirect URI set to this server's address plus `/auth/callback/google`, and pass its id with `--google-client-id` (or `LIBREPAPER_GOOGLE_CLIENT_ID`) and its secret as `LIBREPAPER_GOOGLE_CLIENT_SECRET`. The consent screen asks for the scopes `openid`, `email` and `profile`.[^google-data] All three are non-sensitive, so the app needs no verification review, but **publish the consent screen**: one left in *Testing* admits at most a hundred named test users, and everybody else is turned away at Google's own page.

`librepaper logout` deletes the terminal's local token. It does not revoke a
copy held elsewhere. Rotating the server's session key invalidates issued
browser and terminal credentials across the deployment.

[^google-data]: LibrePaper reads the verified email address on a Google account,
the hosted domain, the account identifier, the profile name and the profile
picture. The address is what `--publishers`, `--commenters` and a grant by name
are matched against, and where a retention notice is sent; it is shown to no
other reader anywhere. Other readers see the profile name. The picture is shown
only to you, in the bar, and its address is kept in your own session cookie.

## Retention

Retention is off by default: a server started without `--document-expire-after`
keeps every document until somebody deletes it. That is the right default for a
server whose publishers you know, and the wrong one for a server anybody may
publish to. A public deployment that accepts uploads from strangers is durable
hosting for whatever they upload, so set a lifetime on it.

Delete documents automatically after their most recent update:

```sh
librepaper admin serve --document-expire-after 24h
```

For a fixed lifetime from the first upload, use `--document-expire-from created`. Use
`--document-expire-after never` to disable expiry. Expired documents are removed by an
hourly pass, and once at startup.

## Privacy and the LaTeX mirror

Browsers download the LaTeX compiler distribution directly from the default
project mirror, `https://latex.librepaper.workers.dev/`. The mirror receives
the browser's IP address and the digest-named files it requests. Those
requests can reveal package choices and suggest a document's field or
template. Compiler downloads do not send document source or private input
assets to the mirror.

An operator can host a copy and keep compiler requests on their own
infrastructure by passing `--latex-mirror URL` to `librepaper admin serve`. The URL
must be an HTTPS static mirror with the documented mirror layout and headers.
