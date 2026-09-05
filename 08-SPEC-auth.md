# SPEC: signing in with Google as well as GitHub

Status: not built. Extends `komodoc/src/auth.rs`, the `/auth/*` routes in
`server.rs`, `komodoc login` in `cli.rs`, and the sign-in controls in the
web shell. `03-SPEC-sharing.md` grants roles "by name" to a GitHub account;
this spec widens what a name can be, and that spec should be read with it.
`07-SPEC-retention.md` takes the contact email from GitHub; the last section
here says what it takes from Google.

## The problem

Identity is GitHub, and only GitHub. `auth.rs` opens with the sentence.
The session cookie carries a GitHub login and a GitHub numeric id, the CLI
holds a GitHub token from GitHub's device flow, `--publishers` and
`--commenters` are lists of GitHub logins, and every refusal says "sign in
with GitHub".

Most people who read or review an academic document do not have a GitHub
account and will not make one to leave a comment. Nearly all of them have a
Google account, and on a university deployment every one of them does,
under the institution's domain. The sharing spec meets the reviewer without
an account with a link that carries a role; this spec meets them with a
sign-in they already have, which is what gives a comment a name that
survives a cleared `localStorage`, and what lets an owner name a coauthor by
the address they already write to.

Nothing in the model actually depends on GitHub except the strings. Ownership
keys on a stable id that the provider guarantees; policies match a handle;
comments show a name. Each of those has a Google equivalent. What is missing
is the word *provider* in three places: the identity, the configuration, and
the routes.

## What an identity is

Today:

```text
Identity { login, id }        login: GitHub login, lowercased; id: GitHub numeric id
```

After:

```text
Identity { provider, id, handle, name }
```

| field | GitHub | Google | used for |
| --- | --- | --- | --- |
| `provider` | `github` | `google` | which sign-out and which routes; display of the badge |
| `id` | `github:583231` | `google:107691503500061507151` | ownership, comment authorship, grants: the only field anything is keyed on |
| `handle` | login, lowercased | verified email, lowercased | matching `--publishers`, `--commenters`, and grants by name; never shown to other readers for a Google account |
| `name` | the login | the profile name, or the email's local part when Google returns no name | what other readers see on comments, checkpoints and the nav |

The id is qualified with the provider, because GitHub ids and Google `sub`
values are both decimal strings and nothing but the prefix keeps one
namespace out of the other. The handle needs no prefix: a GitHub login
cannot contain `@`, a verified email always does, so the character decides.

`is_signed_in` becomes "the id is not empty". The anonymous identity is all
four fields empty, as now.

**Reading what is already stored.** A `publisher_id` written by today's
server is a bare number and means a GitHub account. Rather than rewrite the
index, `owned_by` qualifies a bare stored id as `github:` before comparing.
The same rule applies wherever an id is read back: the `editors` and
`commenters` grants of the sharing spec, and the `by` on a checkpoint. New
writes are always qualified. A comment's `author` key stays `github:<login>`
for GitHub, as it is, and is `google:<sub>` for Google, since Google has no
login to put there.

**The session cookie** payload gains the provider and the name:

```text
provider|handle|id|name|expiry
```

A three-field cookie from today's server is a GitHub session and is read as
one until it expires, provider `github`, name equal to the login. That is
not the "half-trusted" case the two-field rule refuses: nothing is missing
from it, only implied. The name is in the cookie so that a page render
never needs a provider round trip to label the nav.

**Two accounts are two people.** A person who signs in with GitHub on
Monday and Google on Tuesday owns two disjoint sets of documents, and this
spec does not link them. Linking needs a verified claim that both accounts
are one person, which is a feature with its own spec; until then, the
sharing spec's grant by name lets someone share a document with their own
other account, which covers the case that comes up.

## Configuration

Two more variables, alongside the two GitHub ones:

```sh
export KOMODOC_GOOGLE_CLIENT_ID="...apps.googleusercontent.com"
export KOMODOC_GOOGLE_CLIENT_SECRET="..."
```

The client is created at console.cloud.google.com under *Credentials*, type
*Web application*, with the redirect URI set to the server's own address
plus `/auth/callback/google`, HTTPS except on localhost, which is what
Google enforces and what local `serve` on `http://localhost:<port>` still
satisfies. The consent screen asks for the scopes `openid`, `email` and
`profile`; all three are non-sensitive, so the app needs no verification
review, but a consent screen left in *Testing* admits at most a hundred
named test users, and the README says to publish it.

A provider is configured when its client id is set. The rule in `serve.rs`
that dies without a GitHub app becomes: if either policy needs a sign-in,
at least one provider must be configured, and the message lists both ways
to get one. Two further checks, warnings rather than deaths, because a
deployment may be mid-migration: a policy naming a login when GitHub is not
configured, or an email or domain when Google is not, names people who can
never sign in, and says so at startup.

The flags `--client-id` and `--client-secret` keep meaning GitHub; Google
has no flags, only the environment, since the README already tells
operators to prefer the environment for secrets.

## Policies

`Policy::parse` accepts three more shapes in a list:

| entry | matches |
| --- | --- |
| `alice` | the GitHub login `alice` |
| `alice@example.org` | the Google account whose verified email is that address |
| `@example.org` | any Google account whose verified email is on that domain |
| `any` | any signed-in account, on either provider |
| `anyone` | no sign-in at all |

Matching is against the handle, case-insensitive, and a domain entry
matches the part after the `@` exactly, so `@example.org` does not admit
`@mail.example.org`. `any` widens from "any GitHub account" to "any
account": an operator who wants one provider only leaves the other
unconfigured, which is a property of the deployment rather than of every
policy on it. `describe()` follows the same words, so a refusal reads
"this deployment allows alice, @umontreal.ca".

The domain form is in this spec because it is one line of matching and it
is the shape a course or a department actually wants. It relies on the
`email` claim's domain, with `email_verified` true, and not on Google's `hd`
claim, which is absent for non-Workspace accounts and says nothing the
verified email does not.

## Signing in from a browser

Routes, with today's kept where an OAuth app has already been registered
against it:

| route | does |
| --- | --- |
| `GET /auth/login` | with one provider configured, redirects into it; with two, a small page with a button for each; `?next=` as now |
| `GET /auth/login/github` | today's `/auth/login`: state cookie, redirect to GitHub |
| `GET /auth/login/google` | state cookie plus PKCE verifier, redirect to Google |
| `GET /auth/callback` | GitHub's callback, unchanged, so no existing OAuth app has to be edited |
| `GET /auth/callback/google` | Google's callback |
| `POST /auth/logout` | unchanged |

The Google authorize URL is `https://accounts.google.com/o/oauth2/v2/auth`
with `response_type=code`, `scope=openid email profile`, `state`,
`code_challenge` (S256) and `code_challenge_method`, and
`prompt=select_account`, so a person with a personal and an institutional
account picks the one they mean rather than being handed whichever Google
last used. The PKCE verifier rides in the state cookie beside the state
token and the `next` path, in the same `|`-joined value, which is HttpOnly
and `__Host-` on HTTPS and so is as private as the state already is.

The callback exchanges the code at `https://oauth2.googleapis.com/token`
with the client secret, the verifier and the same redirect URI, then asks
`https://openidconnect.googleapis.com/v1/userinfo` with the access token.
It reads `sub`, `email`, `email_verified` and `name`. The id token that the
exchange also returns is not used: verifying it means fetching Google's
signing keys and checking a JWT, and the userinfo call proves the same
thing through the same trust the GitHub `/user` call already rests on,
namely that the server itself just exchanged the code with its own secret.
An answer with `email_verified` false, or no email at all, is refused with
a page saying the Google account has no verified address, since the handle
would be empty and no policy could ever admit it.

The session cookie is then set exactly as for GitHub. The access token is
discarded; nothing after sign-in needs Google again.

**The page.** The nav's "Sign in" goes to `/auth/login`, which is one
provider's redirect or the two-button page, so the shell never has to know
which are configured to render the button. Where the reader says "Sign in
with GitHub to comment", it says "Sign in to comment", and the same link.
A signed-in GitHub account shows as `@alice`, a Google account as the name
with no `@`, since the `@` on a handle is what marks a GitHub login and a
Google name is not a handle. `/api/me` returns `provider`, `handle` and
`name` in place of `login`, and `providers`, the list of those configured,
in place of `can_sign_in`; the page shows a sign-in control when the list
is non-empty.

## Signing in from the terminal

`komodoc login` today runs GitHub's device flow against GitHub and stores
the GitHub token; the server verifies each bearer against GitHub's
check-token endpoint. Doing the same for Google is possible and is the
wrong shape. Google's device flow requires a second client of the *TVs and
Limited Input devices* type, whose secret the CLI must send on every poll,
so the deployment would have to publish it; its access tokens expire after
an hour, so the CLI would hold a refresh token and the server would verify
against `tokeninfo` on a token that changes; and `komodoc login` would need
a `--provider` flag before it could ask for a code. Every deployment that
adds a third provider later would repeat all of it.

Instead the CLI signs in through the deployment, and the deployment signs in
through whichever provider the person picks in their browser:

1. `POST /api/auth/device` returns `{device_code, user_code, verification_url, expires_in, interval}`.
   The server keeps the pending code in memory, keyed by user code, holding
   a SHA-256 of the device code and no identity yet. Ten minutes, then gone.
   A restart forgets pending codes, and a `login` that was mid-flight is
   told to start again, which is the only harm.
2. The CLI prints the URL and the code, as it does now. The URL is
   `<server>/auth/device?code=<user_code>`.
3. That page is a shell route. It requires a signed-in session, sending the
   visitor through `/auth/login?next=` otherwise, then shows the code and
   the account it would sign in, and an *Approve* button. Approval is a
   POST with the cross-site checks `/auth/logout` uses, and never happens
   from the link alone, because a link someone else sends you must not be
   able to put your identity on their terminal.
4. The CLI polls `POST /api/auth/device/token` with the device code at the
   interval given, and receives `authorization_pending` until the approval,
   `expired_token` after ten minutes, or the token.
5. The token is `kmd_` followed by the same signed payload the session
   cookie carries, with a ninety-day expiry rather than thirty. It is
   stored where the GitHub token is stored now, with the same permissions.

`whoami` then has three cases for a bearer: a `kmd_` token verifies locally
against the session key, with no network and no cache; anything else is a
GitHub token and goes through check-token as now, so `KOMODOC_TOKEN` with a
token from a GitHub app keeps working; and a bearer with no provider
configured to verify it is anonymous, as now. The user code is eight
characters from an alphabet without look-alikes, about forty bits, and the
device code is 128 random bits; guessing a pending user code inside ten
minutes is not a practical attack, and the worst it could do is sign the
guesser's own identity onto a stranger's terminal, which the stranger sees
in `signed in as`.

GitHub's device flow is retired from the CLI along with the requirement
that the OAuth app have *Device Flow enabled*. `/api/auth/config` stays for
one release so an older CLI gets an answer, then goes.

Revocation is what it is for cookies: `komodoc logout` deletes the file,
and rotating the session key signs every browser and every terminal out at
once. A token cannot be revoked one at a time, and the README says so.

## Sharing by name

The sharing spec's `komodoc share c9k --editor annegrandchamp` names a
GitHub login. With this spec a name is a handle, so
`--editor anne.grandchamp@umontreal.ca` is a grant to the Google account
with that verified address, recorded as `{"id": "google:...", "handle":
..., "name": ...}` in the same list. Resolving a handle to an id at share
time needs the account to have signed in at least once, so the server has
seen the pair; a grant to a handle the server has never seen is stored by
handle alone and is resolved on that account's first sign-in, which is what
"share with a reviewer who has not signed up yet" means. That resolution
is an addition to the sharing spec's step 2, not a change to it.

## Contact email, for retention

`07-SPEC-retention.md` requests the `user:email` scope from GitHub and
takes the primary verified address. For a Google account the verified
email is already in hand from `userinfo`, is the handle, and is what
retention writes to `contact_email`; there is no extra scope and no second
call. The retention spec's principle that "GitHub remains the authority for
contact email" reads as "the provider the account signed in with remains
the authority", and its user-facing summary says "the email address on the
account you sign in with".

## What is not in this spec

- Linking a GitHub account and a Google account into one person.
- Any provider beyond these two. The shape is meant to take a third, and
  an ORCID sign-in is the obvious candidate for this audience, but each
  provider is a consent screen, a redirect URI, and a userinfo shape, and
  gets its own short section when it comes.
- Email and password. The service does not hold credentials, and this spec
  does not start.
- Showing a Google account's email to other readers, anywhere. The handle
  is for matching. If two Google accounts named *Jean Tremblay* comment on
  the same document, their comments are distinguishable to the server by
  id and not to a reader by eye, and that is accepted; GitHub logins are
  unique and Google names are not.

## Steps

1. **Identity carries its provider.** `Identity` gains `provider`, `handle`
   and `name`; `id` is qualified; the session cookie gains two fields and
   reads the three-field shape as GitHub; `owned_by` and the grant reader
   qualify bare stored ids. `/api/me` changes shape and the shell follows.
   Nothing user-visible changes.
2. **Google in the browser.** `GoogleApp` beside `GithubApp`, the two
   environment variables, the startup check, the three new routes, PKCE,
   the two-button page, the `email_verified` refusal, the refusal messages
   that no longer say GitHub.
3. **Policies by email and domain.** `Policy::parse` and `describe`, the
   startup warnings for unsatisfiable entries.
4. **The deployment's own device flow.** The pending-code table, the two
   API routes, the approval page, `kmd_` tokens in `whoami`, `komodoc
   login` rewritten against it, the GitHub device flow removed.
5. **Docs.** README: the Google client, the consent screen, the policy
   table, the privacy footnote for Google (`openid email profile`; the
   email matches allowlists and receives retention notices, and is shown
   to nobody), the revocation note.

Tests, by step: `Policy::parse` with emails, domains and mixed lists, and
`describe` of each; a session round trip with each provider and a
three-field cookie read as GitHub; `owned_by` with a bare stored id against
a qualified caller; the Google callback against a stand-in userinfo, with
`email_verified` false refused; the device flow end to end in the harness,
including a poll before approval, a poll after expiry, and a `kmd_` token
accepted by `whoami` with no provider call; a GitHub bearer still verified
through check-token.
