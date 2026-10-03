# Privacy duties for operators

**Publish a notice** naming yourself, contact address, expiry setting, providers and proxy log retention. Point to the [privacy page](https://librepaper.org/privacy.html).

**IP addresses** are in your proxy logs (nginx, Caddy, CDNs), not the application.

**Data requests:** `librepaper export` covers one document at a time. Query the database for complete answers:

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

**Backup restore:** Re-run erasure requests. Keep notes (date and handle) and replay them.

**GDPR:** answer data requests within one month; notify authorities of breaches within 72 hours where people are likely to be at risk.

**Verify who is asking:** A signed-in request from the account itself is easiest proof. For other requests, require sufficient identity verification before answering.
