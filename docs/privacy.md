---
title: "Privacy"
---

LibrePaper is software, not a service. The operator decides what is kept, for
how long, and is answerable to regulators. The project authors run no service.

An operator's reverse proxy, backups and hosting provider keep their own logs.

## Security and encryption

No end-to-end encryption. The server and database can read drafts, comments,
identities and presence in the clear. Backups are unencrypted.

## What a deployment stores

- **Accounts:** provider, identifier, handle, display name, email (Google only),
  preferences, created and last-seen times
- **Documents:** title, format, owner, timestamps, and every file
- **Annotations:** comments, highlights and suggestions with author and passage
- **Checkpoints:** collaborative editing state
- **Shares:** who documents are shared with and hashed share links

No access logs, analytics, telemetry, or stored IP addresses.

Handles are visible only to their owner. GitHub handles are logins; Google
handles are verified emails, shown to no one else.

## Cookies and browser storage

Three cookies:
- `librepaper_session` - signed, 30 days, HttpOnly, SameSite, Secure, __Host- prefix
- `librepaper_state` - ties sign-in to the browser that started it
- `librepaper_visitor` - names anonymous browsers

Browser storage (never sent to server):
- `librepaper-viewed`, `librepaper-favorites` - documents
- `librepaper-layout`, `librepaper-source-side`, `librepaper-panel` - layout
- `librepaper-keymap`, `librepaper-keys` - editor settings and share secrets

None profiles anybody or is shared. No consent banner needed.

## Reading and visibility

Anonymous readers get signed visitor credentials so comments stay linked across
visits. While open, the collaboration layer broadcasts presence and cursor position.

Pseudonymous comments hide identity from other readers, not the operator.

## What leaves the deployment

- **Sign-in:** GitHub or Google sees the request, returns identity. No documents sent.
- **LaTeX mirror:** Compilers and packages come from the project mirror, which sees
  browser address and files requested (not source). See [Privacy and the LaTeX
  mirror](./host.html#privacy-and-the-latex-mirror).
- **Local companion:** The writing assistant runs on your computer; the server relays
  messages while connected.
- **Embedded resources:** Published documents fetch images and data from any host
  named, which learns your address, browser and open time. For anonymous review,
  use PDFs or `embed-resources: true` in Quarto.

Hosting, database and object-storage providers see what they store.

## Retention

- **Documents:** kept until deleted or `--document-expire-after` period passes
  (measured from last update or creation). See [Retention](./host.html#retention).
- **Checkpoints:** kept until document is deleted
- **Edit history:** kept whole for the life of the document
- **Sessions:** 30 days; invalidated by sign-out or account erasure
- **Backups:** as long as the operator keeps them

Deleting a document deletes files, comments, replies, checkpoints and share links.

## Erasing an account

**Settings > Account > Erase this account** (confirm with your handle):

1. Session invalidated immediately; no sign-in possible; irreversible from browser
2. Owned documents marked for deletion and removed after 7 days (configurable)
3. Comments and checkpoints on others' documents stay, relabelled "Deleted user"
4. Account record deleted once documents are gone

Erasure does not reach backups; a restored backup restores the account as it was.

## For operators

**Publish a notice** naming yourself, contact address, expiry setting, providers
and proxy log retention. Point to this page.

**IP addresses** are in your proxy logs (nginx, Caddy, CDNs), not the application.

**Data requests:** `librepaper export` covers one document at a time. Query the
database for complete answers:

```sql
SELECT id, provider, handle, display_name, email, created_at, last_seen_at
FROM accounts WHERE handle = 'the-handle';
```

Join against `documents`, `annotations`, `replies` and `grants` by account ID.
Export owned documents with `librepaper export DOCUMENT DIR`.

**Backup restore:** Re-run erasure requests. Keep notes (date and handle) and
replay them.

**GDPR:** answer data requests within one month; notify authorities of breaches
within 72 hours where people are likely to be at risk.
