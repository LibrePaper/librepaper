---
title: "Document and storage"
---

## The document

A directory is held as four maps in one CRDT document:

| map | keys | values |
| --- | --- | --- |
| `files` | file id | text |
| `paths` | file id | path |
| `assets` | path | digest of bytes |
| `meta` | `main`, engine, markers | document settings |

Text is stored under an opaque id; paths are separate entries pointing at ids. Renames move strings in `paths` without touching text. Comments name files by id, so renames don't orphan reviews. Figure bytes sit in the object store under their digest; identical uploads are stored once. The editable source is authoritative; comment ranges are translated to source when made.

The CRDT is a single Rust library linked natively on the server and compiled to WebAssembly in the browser. Every offset counts UTF-16 code units (matching browser counts). Cursors use Unicode code points; diffs use code points (server) or UTF-16 (browser). Diff information never crosses the network; each side computes from its own state. Both are pinned by tests.

Each document's log is capped by `log_quota_mib` (advanced config, default 32 MB). New edits are refused when the limit is reached. Deployments whose limits cannot accept work they cannot save are refused at startup.

## Storage and history

PostgreSQL holds metadata, update logs and review state; the object store holds immutable blobs keyed by digest or never-reused names.

Document state is persisted as a compressed base plus an ordered log in Postgres, compacted periodically. History is retained by default, but an owner can explicitly trim older edit history and named versions; trimmed history cannot be recovered from the live document.

A checkpoint records the whole directory as a canonical tree. Checkpoints are written when somebody labels a moment, restores an earlier version, or accepts a proposal.

Source archives are produced on request (keyed by tree digest), not at checkpoint time. They are content-addressed and the fastest way to retrieve a document at a point in time.

Document assets are content-addressed; identical uploads are stored once. Uploads reserve quota before the blob write begins and release it if cancelled.

Storage limits:
- `[limits].publisher_storage_mib`
- `[limits].deployment_storage_mib`
- `[limits].publisher_uploads_per_hour`
- `[limits].log_quota_mib`

Owners can set softer history budgets and retention thresholds but cannot raise deployment hard limits. Documents may expire based on creation or last edit.

`librepaper admin backup` writes a snapshot-consistent Postgres dump plus referenced objects and verifies it. The command does not encrypt or schedule backup copies. `restore` restores into a fresh database and directory; it does not independently replay account-deletion requests.

Maintenance (compaction, archiving, deletion) runs on an in-process bounded queue. An idle deployment issues no maintenance queries. Reclamation releases storage only after physical deletion. Superseded objects become eligible after a grace period.
