---
title: "Access and identity"
---

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
[Share](../collaborate/share.md), and may only ever be stricter than the server it is on.
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
