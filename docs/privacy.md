---
title: "Privacy"
---

This notice describes the public LibrePaper deployment. If you run another
instance, its operator controls the database, object storage, logs, backups,
retention, and legal obligations; consult that operator's notice. The server
can read document contents and account data in plaintext.

An operator's reverse proxy, backups and hosting provider keep their own logs.

## Security and encryption

No end-to-end encryption. The server and database can read drafts, comments,
identities and presence in the clear.

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

These browser-storage descriptions concern the application keys and purposes
listed above; they are not a general statement about third-party content
embedded in a document.

## Reading and visibility

Signed-in readers can use their account for comments and presence. While open,
the collaboration layer broadcasts presence and cursor position to other viewers
of the document. Read, comment, and edit links require sign-in.

Authorized readers receive a projected copy of shared source text, main-file
metadata, and assets in order to render the document in the browser. They do not
join source CRDT synchronization or receive its operation history, but shared
source is readable in the browser and should not be treated as private. Keep
confidential inputs outside the shared project; a local companion folder can
provide local build inputs without uploading that folder.

Pseudonymous comments hide identity from other readers, not the operator.

## What leaves the deployment

- **Sign-in:** GitHub or Google sees the request, returns identity. No documents sent.
- **LaTeX mirror:** Compilers and packages come from the project mirror, which sees
  browser address and files requested (not source). Package choices are fingerprintable.
  Operators can avoid this by [hosting their own mirror](https://github.com/LibrePaper/librepaper/blob/main/docs/dev/asset-mirrors.md#hosting-your-own-copy).
- **Local companion:** The writing assistant runs on your computer; the server relays
  messages. When you start an agent from an open document page, the browser silently
  renews a document- and link-scoped five-minute token; the headless CLI uses the
  separate `librepaper login` flow. Other companion requests begin when you use
  local execution, choose a local build tool, look up Zotero items, open Local
  settings, or enable account backups. While already paired and signed in, the
  page also polls the companion for backup status; it does not probe for a
  companion on first visit just for backups.
- **Local backups:** When paired and signed in, the browser polls the companion for
  account-wide backup status and sends the account ID with backup settings and run
  requests. The companion uses its CLI login to fetch projects available to the
  account, including shared projects, and writes ZIPs into the folder you chose
  with its native picker. The browser never receives that folder's absolute
  path. ZIP files remain on your computer
  when backups are disabled or a project is deleted. See [local backups](backups.html).
- **Embedded resources:** Published documents fetch images and data from any host
  named, which learns your address, browser and open time. To avoid requests to
  those hosts, use PDFs or `embed-resources: true` in Quarto.

Hosting, database and object-storage providers see what they store.

## Retention

- **Documents:** kept until deleted or the configured `[retention].expire_after` period passes
  (measured from last update or creation). See [storage, limits, and backup](./host.html#storage-and-backup).
- **Checkpoints and edit history:** retained by default while the document
  exists; an owner can explicitly trim history, which permanently removes
  older history and named versions. See [history and revisions](./collaborate/history.html).
- **Sessions:** session credentials expire after 30 days. Signing out clears the
  current browser's cookie; it does not invalidate a copied credential. Account
  erasure revokes the account's sessions.
- **Backups:** the Docker kit can encrypt and schedule database/object recovery
  points when `resticprofile.toml` contains a `[resticprofile]` table.
  Snapshots can retain erased data
  until retention removes it; operators must review and reapply deletion requests
  after recovery. Session keys are included so restored sessions and share URLs
  remain valid. Backup access, repository credentials and off-host copies are
  the operator's responsibility.

Deleting a document deletes files, comments, replies, checkpoints and share links.

## Erasing an account

**Settings > Account > Erase this account** (confirm with your handle):

1. Account sessions are revoked; the account cannot sign in again. Erasure is irreversible from the browser
2. Owned documents are marked for deletion and purged after seven days
3. Comments, replies and labels on others' documents remain as "Deleted user"
4. The account record is deleted after its owned documents are purged

Account and document deletion affects the live service according to the
retention periods above. Separate backup copies may still contain deleted data
until the operator's backup-retention process removes them. Restoring a backup
does not automatically replay an independent deletion ledger; operators must
review and reapply applicable deletion requests after restore. See the
[operator privacy guide](https://github.com/LibrePaper/librepaper/blob/main/docs/dev/privacy-operators.md).
