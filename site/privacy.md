---
title: "Privacy"
---

LibrePaper is software, not a service. Whoever runs the deployment you sign
into decides what is kept and for how long, answers requests about it, and is
the one a data-protection authority would ask. The project's authors run no
service on an operator's behalf and hold none of their data.

This page describes what the software does, so that a person using a
deployment knows what it holds and an operator can write their own notice
without reading the source. An operator may keep more than this -- a reverse
proxy, a backup system and a hosting provider each have logs of their own --
and only they can tell you about those.

## What a deployment stores

| What | Where | Written when |
| --- | --- | --- |
| Provider, the provider's identifier for you, handle, display name, and -- for a Google account -- the verified email address | `accounts` | You sign in for the first time; refreshed on later sign-ins |
| Preferences, the time the account was created, and the time it was last seen | `accounts` | Continuously |
| Projects: title, format, owner, timestamps, and every file in them | `documents`, object storage | You upload or create a project |
| Comments, highlights and suggestions, with their author and the passage they point at | `annotations`, `replies` | Somebody annotates |
| Checkpoints and the collaborative editing state | `document_labels`, `document_updates` | Continuously while a document is open |
| Who a document has been shared with, and the hashed form of each share link | `grants`, `share_links` | The owner shares |

The handle is the account's own to see. A GitHub handle is a login; a Google
handle is the verified email address the account signed in with, and it is
shown to nobody -- other readers see the display name.

What a deployment does **not** keep: there is no application access log, no
analytics, no telemetry, and no record of IP addresses. Addresses are used in
memory to rate-limit requests and are not written down.

## Cookies and browser storage

Three cookies, all set by the deployment itself and none for advertising or
measurement:

- `librepaper_session` -- proves who you are. Signed, thirty days, `HttpOnly`,
  `SameSite`, and `Secure` with the `__Host-` prefix over HTTPS.
- `librepaper_state` -- ties an in-flight sign-in to the browser that started
  it. Lasts the length of the redirect.
- `librepaper_visitor` -- names the browser, so a document uploaded without
  signing in still belongs to whoever uploaded it.

The browser also keeps, in its own local storage and never on the server:
which documents you have opened (`librepaper-viewed`) and starred
(`librepaper-favorites`), the layout of the window (`librepaper-layout`,
`librepaper-source-side`, `librepaper-panel`), the editor keymap
(`librepaper-keymap`), and the share-link keys this browser has been handed
(`librepaper-keys`). A link key is a secret; keeping it here is why opening
the same link on a phone means pasting it again, and why clearing site data
clears it.

All of this is either strictly necessary to provide the service or set
because you asked for it by using the feature. None of it profiles anybody or
is shared with a third party, so under the ePrivacy rules none of it requires
a consent banner. An operator who adds analytics of their own changes that
answer and takes on the consent question with it.

## What leaves the deployment

- **Sign-in.** GitHub or Google, whichever the operator enabled, receives the
  sign-in request and returns the identity above. Nothing about your documents
  is sent to them.
- **The LaTeX mirror.** Browsers download the compiler and its packages from
  the project mirror, which sees the browser's address and which files it
  asks for. Document source never goes there. See
  [Privacy and the LaTeX mirror](./host.html#privacy-and-the-latex-mirror).
- **The local companion.** The writing assistant runs on your own computer.
  The server relays messages between the browser and the companion while both
  are connected and keeps no transcript.

An operator's hosting, database and object-storage providers necessarily see
what they store. Which ones those are, and where, is theirs to name.

## How long things are kept

- **Projects** are kept until somebody deletes them, unless the operator set a
  lifetime with `--document-expire-after`. Expiry is measured from the most
  recent update by default, or from creation with
  `--document-expire-from created`, and an hourly pass removes what has lapsed.
  See [Retention](./host.html#retention).
- **Checkpoints** are kept until the project is deleted. Nothing prunes them:
  one exists only because somebody named a moment, restored an earlier
  version, accepted a proposal, or committed from the CLI.
- **Edit history** -- the operation log behind the checkpoints -- is kept whole
  for the life of the project. It is what the History panel reads to show the
  document at a moment nobody checkpointed.
- **Sessions** last thirty days, and are invalidated at once by signing out,
  by the operator, or by erasing the account.
- **Backups** last as long as the operator keeps them, and a restored backup
  brings back whatever it contains.

## Deleting a project

Deleting a project deletes its files, its comments and replies, its
checkpoints and collaborative state, and its share links. Live sessions on it
are closed as part of the deletion rather than left running.

## Erasing an account

**Settings → Account → Erase this account**, in any document, after typing
your handle to confirm. What then happens:

1. The session is invalidated immediately and the account can no longer sign
   in -- including to change its mind. There is no way back from the browser.
2. Every project the account owns is marked for deletion and removed after a
   recovery window, seven days on a default deployment.
3. Comments, replies and checkpoints the account left on *other people's*
   documents stay where they are, relabelled "Deleted user" and no longer
   linked to any account. Those documents belong to somebody else, and a
   conversation cannot be silently rewritten under them.
4. The account record itself -- provider, identifier, handle, name, email -- is
   deleted once its documents are gone.

Erasure does not reach into an operator's backups. A backup restored later
carries the account as it was at the time, which is why the operator
procedure below exists.

## For operators

You are the controller for your deployment. What is yours rather than the
software's:

**Publish a notice** naming yourself and a contact address, and say which of
the choices above you made: your expiry setting, your providers, your hosting
and its location, and how long your proxy keeps its logs. Point at this page
for the mechanics if it saves you writing them out.

**Your proxy is where the addresses are.** The application writes none; nginx,
Caddy or a CDN in front of it writes all of them. Decide a retention period
there and say what it is.

**Re-run erasure after restoring a backup.** Restore does not know which
accounts were erased since the backup was taken. Keep a note of erasure
requests -- the date and the account handle is enough -- and replay them after
any restore.

**Answering a request about a person.** `librepaper export` covers one
document at a time, which is not a complete answer: someone's comments on
other people's documents are not in any export. Until there is a command for
it, query the catalogue directly. Find the account:

```sql
SELECT id, provider, handle, display_name, email, created_at, last_seen_at
FROM accounts WHERE handle = 'the-handle';
```

Then everything attached to it:

```sql
SELECT d.slug, d.title, d.created_at, d.updated_at
FROM documents d WHERE d.owner_id = $1;

SELECT d.slug, a.kind, a.body, a.proposed_text, a.created_at
FROM annotations a JOIN documents d ON d.id = a.document_id
WHERE a.author_account_id = $1 ORDER BY a.created_at;

SELECT d.slug, r.body, r.created_at
FROM replies r
JOIN annotations a ON a.id = r.annotation_id
JOIN documents d ON d.id = a.document_id
WHERE r.author_account_id = $1 ORDER BY r.created_at;

SELECT d.slug, v.created_at FROM document_labels v
JOIN documents d ON d.id = v.document_id WHERE v.author_account_id = $1;

SELECT document_id, role, created_at FROM grants WHERE account_id = $1;
```

The files of an owned project come out of `librepaper export DOCUMENT
--project`, and its comments out of `librepaper export DOCUMENT`.

**Erasure on request** is the same operation the settings page performs, so
the simplest route is to ask the person to run it themselves. A request made
by other means is answered by an operator with database access, which is also
what a request to stop an erasure in its recovery window takes.

**Timelines.** A request under the GDPR is normally answered within one month,
and a personal-data breach is notified to the supervisory authority within 72
hours of becoming aware of it where it is likely to be a risk to people.
Verify who is asking before answering: a signed-in request from the account
itself is the easiest proof there is.
