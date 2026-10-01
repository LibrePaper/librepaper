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
  created and last-seen times
- **Documents:** title, format, owner, timestamps, and every file
- **Annotations:** comments, highlights and suggestions with author and passage
- **Checkpoints:** collaborative editing state
- **Shares:** who documents are shared with and hashed share links

The application does not log visitor requests or persist IP addresses. Its
monitoring endpoint exports aggregate counts without user or document identifiers.

The official deployment's optional monitoring uses separate, free and open-source
Prometheus, Grafana, PostgreSQL exporter, and Node Exporter containers. Grafana's
upstream analytics and update reporting are disabled. Prometheus stores aggregate
service, database, and host measurements, including request and document counts,
without account, document, or IP identifiers. Its time series have a 30-day
retention limit and an 8 GB retained-block size limit; WAL and headroom can use
additional disk, and storage pressure can shorten retention. Exporter metrics
include operational metadata such as database names, device statistics, and
filesystem measurements.
Grafana administrative and error logs may include an administrator account,
source IP, or request path. Operators should restrict access to those logs and
set a retention policy for them separately.

The monitoring containers are not included in the application's document and
database backup set: dashboards can be reprovisioned from the deployment kit,
while historical time series and Grafana state are not covered by those backups.
These controls describe this deployment's data handling; they do not by themselves
determine an operator's legal obligations.

Handles are visible only to their owner. GitHub handles are logins; Google
handles are verified emails, shown to no one else.

## Cookies and browser storage

Three cookies:
- `librepaper_session` - signed, 30 days, HttpOnly, SameSite, Secure (over HTTPS only), __Host- prefix (over HTTPS only)
- `librepaper_state` - ties sign-in to the browser that started it
- `librepaper_visitor` - signed visitor credential that keeps legacy visitor activity tied to the same browser

Browser storage (never sent to server):
- `librepaper-viewed`, `librepaper-favorites` - documents
- `librepaper-layout`, `librepaper-source-side`, `librepaper-panel` - layout
- `librepaper-keymap` - editor settings

Browser storage sent to the deployment:
- `librepaper-keys` - share secrets, sent as the X-LibrePaper-Key header

None profiles anybody or is shared. No consent banner needed.

## Reading and visibility

Signed-in readers can use their account for comments and presence. While open,
the collaboration layer broadcasts presence and cursor position to other viewers
of the document. Read, comment, and edit links require sign-in.

Pseudonymous comments hide identity from other readers, not the operator.

## What leaves the deployment

- **Sign-in:** GitHub or Google sees the request, returns identity. No documents sent.
- **LaTeX mirror:** Compilers and packages come from the project mirror, which sees
  browser address and files requested (not source). Package choices are fingerprintable.
  Operators can host their own mirror to avoid this. See [Privacy](./host.html#privacy).
- **Local companion:** The writing assistant runs on your computer; the server relays
  messages. When you start an agent from an open document page, the browser silently
  renews a document- and link-scoped five-minute token; the headless CLI uses the
  separate `librepaper login` flow. The browser contacts the companion only when you initiate: turning on local execution,
  choosing a local build tool, Zotero lookup, or opening Local app settings.
- **Embedded resources:** Published documents fetch images and data from any host
  named, which learns your address, browser and open time. To avoid requests to
  those hosts, use PDFs or `embed-resources: true` in Quarto.

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
-- Find the account
SELECT id, provider, handle, display_name, email, created_at, last_seen_at
FROM accounts WHERE handle = 'the-handle';

-- Documents owned by account $1
SELECT d.slug, d.title, d.created_at, d.updated_at
FROM documents d WHERE d.owner_id = $1;

-- Annotations (comments, highlights, suggestions) by account $1
SELECT d.slug, a.kind, a.body, a.created_at
FROM annotations a JOIN documents d ON d.id = a.document_id
WHERE a.author_account_id = $1 ORDER BY a.created_at;

-- Replies to annotations by account $1
SELECT d.slug, r.body, r.created_at
FROM replies r JOIN annotations a ON a.id = r.annotation_id
JOIN documents d ON d.id = a.document_id
WHERE r.author_account_id = $1 ORDER BY r.created_at;

-- Checkpoints by account $1
SELECT d.slug, v.created_at FROM document_labels v
JOIN documents d ON d.id = v.document_id WHERE v.author_account_id = $1;

-- Document access grants for account $1
SELECT document_id, role, created_at FROM grants WHERE account_id = $1;
```

Export owned documents with `librepaper export DOCUMENT DIR`.

**Backup restore:** Re-run erasure requests. Keep notes (date and handle) and
replay them.

**GDPR:** answer data requests within one month; notify authorities of breaches
within 72 hours where people are likely to be at risk.

**Verify who is asking:** A signed-in request from the account itself is easiest
proof. For other requests, require sufficient identity verification before
answering.
