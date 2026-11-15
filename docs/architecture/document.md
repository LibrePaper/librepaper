---
title: "Document and storage"
---

## The document

A project is one Loro CRDT document. Its roots are:

| root | keys | values |
| --- | --- | --- |
| `files` | file id | the text of the file |
| `paths` | file id | the path the text is known by |
| `assets` | path | digest of the bytes |
| `meta` | `latex.engine`, other settings | document settings |

Plus `changes` for pending [track changes](review.html#track-changes) and `anchors` for [comment](review.html#comments) records.

Text is stored under an opaque id, and paths point at ids. A rename moves a string in `paths` and leaves the text alone, so comments, which name files by id, survive renames. Agents rename through `document_files`, and sync pushes send renames, so they keep the id too. Figure bytes sit in the object store under their digest, and identical uploads are stored once. The editable source is authoritative.

Offsets in the API and in the browser count UTF-16 code units, as browsers do. Loro cursors and the track-change planner count Unicode code points, and the server converts between the two. Diff information never crosses the network: each side computes it from its own state, and both are pinned by tests.

The log of each document is capped by `[limits].log_quota_mib`, which defaults to 32 MiB. New edits are refused at the limit. The server refuses to start if its limits cannot accept work it could not save.

## Storage and history

PostgreSQL holds metadata, update logs and review state. The object store holds immutable blobs, keyed by digest or by names that are never reused.

Document state is a compressed base plus an ordered log in Postgres, compacted periodically. History is kept by default. An owner can trim older edit history and named versions, and trimmed history cannot be recovered from the live document. When the owner trims, project chat is deleted too. An editor's trim leaves chat alone.

Project chat is stored with the document, apart from the edit log. It counts toward the owner's storage and has no message cap.

A checkpoint records the whole directory as a canonical tree. Checkpoints are written when somebody labels a moment or restores an earlier version. Source archives, keyed by tree digest, are built on request rather than at checkpoint time.

Uploads reserve quota before the blob write starts and release it if cancelled. Limits:

- `[limits].publisher_storage_mib`
- `[limits].deployment_storage_mib`
- `[limits].publisher_uploads_per_hour`
- `[limits].log_quota_mib`

Owners can set softer history budgets and retention, under `[retention]`, but cannot raise the deployment's hard limits. Documents can expire based on creation or last edit.

`librepaper admin backup` writes a snapshot-consistent Postgres dump, the referenced objects and a manifest, and verifies them. It does not encrypt the backup or schedule copies. `librepaper admin restore` restores into a new directory and an empty PostgreSQL database.

Maintenance (compaction, archiving, deletion) runs on a bounded in-process queue. Reclaimed storage is released only after physical deletion. Superseded objects become eligible after a grace period.
