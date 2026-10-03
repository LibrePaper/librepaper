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

Each document's log is capped by `log_quota_mb` (advanced config, default 32 MB). New edits are refused when the limit is reached. Deployments whose limits cannot accept work they cannot save are refused at startup.

## Storage and history

PostgreSQL holds accounts, documents, grants, share links, annotations, replies, update logs, labels, proposals, hunk decisions, and storage accounting. The object store holds immutable blobs: collaboration bases, source archives, document assets and published files, keyed by content digest or never-reused names.

Document state is persisted as a compressed base plus an ordered log in Postgres, compacted periodically. The base is full operation history compressed with zstd (a million keystrokes costs a few hundred kilobytes). There is no retention window on edit history.

A checkpoint records the whole directory as a canonical tree. Checkpoints are written when somebody labels a moment, restores an earlier version, commits from the command line, or a proposal is accepted. The manifest is served to the browser. Checkpoints can be labelled and are kept until the document is deleted.

Source archives are produced on request (keyed by tree digest), not at checkpoint time. They are content-addressed and the fastest way to retrieve a document at a point in time.

Document assets are content-addressed; identical uploads are stored once. Uploads reserve quota before the blob write begins and release it if cancelled.

Storage limits:
- `--publisher-storage-limit`
- `--deployment-storage-limit`
- `--publisher-upload-limit`
- `log_quota_mb` (advanced config)

Owners can set softer history budgets and retention thresholds but cannot raise deployment hard limits. Documents may expire based on creation or last edit.

`librepaper admin backup` writes a snapshot-consistent Postgres dump plus referenced objects and verifies it. `restore` restores into a fresh directory.

Maintenance (compaction, archiving, deletion) runs on an in-process bounded queue. An idle deployment issues no maintenance queries. Reclamation releases storage only after physical deletion. Superseded objects become eligible after a grace period.
