---
title: "Privacy"
---

This notice describes the public LibrePaper deployment. If you run another
instance, its operator controls the database, object storage, logs, backups,
retention, and legal obligations. Consult that operator's notice. The server
can read document contents and account data in plaintext.

An operator's reverse proxy, backups and hosting provider keep their own logs.

## Security and encryption

Content is not end-to-end encrypted. The server and database can read drafts, comments,
identities and presence in the clear.

## What a deployment stores

- **Accounts** store the provider, identifier, handle, display name, email (Google only),
  and created and last-seen times
- **Documents** store the title, format, owner, timestamps, and every file
- **Annotations** are comments, highlights and suggestions, stored with author and passage
- **Checkpoints** hold collaborative editing state
- **Shares** record who documents are shared with and store hashed share links

The application does not log visitor requests or persist IP addresses. Its
monitoring endpoint exports aggregate counts without user or document identifiers.

The server keeps aggregate operational numbers (request counts, latency, memory,
disk, CPU, socket and document counts) in one SQLite file on the data volume for
400 days. The numbers carry no user, document or network identifiers. The file is
not in backups. It is readable only with the admin password on the admin origin,
which the operator may leave unconfigured.
These controls describe this deployment's data handling. They do not by themselves
determine an operator's legal obligations.

Handles are visible only to their owner. GitHub handles are logins. Google
handles are verified emails, shown to no one else.

## Cookies and browser storage

Three cookies are set:
- `librepaper_session` is signed, lasts 30 days, and is HttpOnly and SameSite. Over HTTPS, it is also Secure and uses the __Host- prefix.
- `librepaper_state` ties sign-in to the browser that started it.
- `librepaper_visitor` is a signed visitor credential that keeps legacy visitor activity tied to the same browser.

Browser storage is never sent to the server:
- `librepaper-viewed` and `librepaper-favorites` store documents.
- `librepaper-layout`, `librepaper-source-side` and `librepaper-panel` store layout.
- `librepaper-keymap` stores editor settings.

Browser storage is sent to the deployment:
- `librepaper-keys` stores share secrets, which are sent as the X-LibrePaper-Key header.

These browser-storage descriptions cover the application keys and purposes
listed above. They are not a general statement about third-party content
embedded in a document.

## Reading and visibility

Signed-in readers can use their account for comments and presence. While open,
the collaboration layer broadcasts presence and cursor position to other viewers
of the document. Edit links require sign-in. Comment links also require sign-in
unless the deployment allows anonymous comments. Read links work for anyone holding them.

Authorized readers receive a projected copy of shared source text, main-file
metadata, and assets in order to render the document in the browser. They do not
join source CRDT synchronization or receive its operation history, but shared
source is readable in the browser and should not be treated as private. Keep
confidential inputs outside the shared project. A local companion folder can
provide local build inputs without uploading that folder.

Pseudonymous comments hide identity from other readers, not the operator.

## What leaves the deployment

- **Sign-in:** GitHub or Google sees the request and returns identity. No documents are sent.
- **LaTeX mirror:** Compilers and packages come from the project mirror, which sees
  browser address and files requested. It never sees source. Package choices are
  fingerprintable. Operators can avoid this by [hosting their own mirror](https://github.com/LibrePaper/librepaper/blob/main/docs/dev/asset-mirrors.md#hosting-your-own-copy).
- **Local companion:** The writing assistant runs on your computer. The server relays
  messages. When you start an agent from an open document page, the browser silently
  renews a document- and link-scoped five-minute token. The headless CLI uses the
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

- **Documents** are kept until deleted or the configured `[retention].expire_after` period passes. The period is measured from the last update or creation. See [storage, limits, and backup](./host/advanced.html#backups).
- **Checkpoints and edit history** are retained by default while the document
  exists. An owner can explicitly trim history, which permanently removes
  older history and named versions. See [storage and history](./architecture/document.html#storage-and-history).
- **Sessions:** session credentials expire after 30 days. Signing out clears the
  current browser's cookie. It does not invalidate a copied credential. Account
  erasure revokes the account's sessions.
- **Backups:** the Docker kit can encrypt and schedule database/object recovery
  points when `resticprofile.toml` contains a `[resticprofile]` table.
  Snapshots can retain erased data until retention removes it. Operators must
  review and reapply deletion requests after recovery. Session keys are included
  so restored sessions and share URLs remain valid. Backup access, repository
  credentials and off-host copies are the operator's responsibility.

Deleting a document deletes files, comments, replies, checkpoints and share links.

## Erasing an account

Choose **Settings > Account > Erase this account** and confirm with your handle:

1. Account sessions are revoked. The account cannot sign in again. Erasure is irreversible from the browser
2. Owned documents are marked for deletion and purged after seven days
3. Comments, replies and labels on others' documents remain as "Deleted user"
4. The account record is deleted after its owned documents are purged

Account and document deletion affects the live service according to the
retention periods above. Separate backup copies may still contain deleted data
until the operator's backup-retention process removes them. Restoring a backup
does not automatically replay an independent deletion ledger. Operators must
review and reapply applicable deletion requests after restore. See the
[operator privacy guide](https://github.com/LibrePaper/librepaper/blob/main/docs/dev/privacy-operators.md).
