# Privacy duties for operators

- Publish a notice with the operator's identity and contact, configured
  document retention, storage and hosting providers, and proxy-log handling.
- The app stores accounts and document data in plaintext. It does not log
  visitor requests or persist visitor IP addresses; reverse proxies, CDNs, and
  hosting providers may keep their own logs.
- See the [public privacy notice](../privacy.md) for the application's data
  flows.

- For a data request, verify the requester and follow the obligations that
  apply to your deployment.
- `librepaper export ID DIR` exports one accessible project at a time; it is
  not an account-wide export.
- Database review may be needed for account fields, authored annotations and
  replies, checkpoints, and grants. The read-only queries below are examples,
  not a complete export; bind the account UUID as `$1` using your database
  client.

```sql
-- Account record
SELECT id, provider, provider_subject, handle, display_name, email,
       created_at, last_seen_at
FROM accounts WHERE id = $1;

-- Projects owned by the account
SELECT slug, title, created_at, updated_at
FROM documents WHERE owner_id = $1;

-- Authored annotations and replies
SELECT d.slug, a.kind, a.body, a.created_at
FROM annotations a JOIN documents d ON d.id = a.document_id
WHERE a.author_account_id = $1 ORDER BY a.created_at;

SELECT d.slug, r.body, r.created_at
FROM replies r JOIN annotations a ON a.id = r.annotation_id
JOIN documents d ON d.id = a.document_id
WHERE r.author_account_id = $1 ORDER BY r.created_at;

-- Checkpoints and access grants
SELECT d.slug, l.label, l.reason, l.created_at
FROM document_labels l JOIN documents d ON d.id = l.document_id
WHERE l.author_account_id = $1 ORDER BY l.created_at;

SELECT d.slug, g.role, g.created_at
FROM grants g JOIN documents d ON d.id = g.document_id
WHERE g.account_id = $1 ORDER BY g.created_at;
```

- The signed-in account starts erasure in **Settings → Account → Erase this
  account**. Sessions are revoked immediately; owned projects are scheduled
  for deletion after seven days.
- A restore can reinstate data deleted after the backup was taken. Track
  deletion requests outside the backup set and review them after a restore;
  see [account-erasure details](../privacy.md#erasing-an-account).
- If Docker backups are enabled, include the session key and both
  `librepaper.toml` and `resticprofile.toml` in recovery planning. Restrict
  repository credentials and keep independent off-host copies of the files
  needed to decrypt and locate snapshots.
