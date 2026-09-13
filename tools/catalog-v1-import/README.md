# catalog-v1-import

`catalog-v1-import` is the deliberately separate, offline converter described
in section 20 of `docs/specs/sql-schema-v2.md`. It opens the source
`catalog.db` read-only and takes the existing `state/writer.lock`; it writes a
fresh v2 root only after source inspection succeeds.

The normal invocation is:

```text
catalog-v1-import --source-data OLD --target-data NEW
```

`--dry-run` performs schema, SQLite integrity, source-work, key metadata,
timestamp, root, and object checks and prints a JSON report without creating a
target. `--resume` requires the target's external `conversion-manifest.json`
to match the source catalog digest, source schema fingerprint, source identity,
target identity, and converter version. Physical object writes use deterministic
IDs recorded by the manifest and are content checked before every retry.

Partial conversion requires one or more explicit `--document STORAGE_ID`
arguments. The completion manifest records every excluded document and remains
incomplete if any selected document or dependency cannot be verified. A
successful conversion is checked against the v1 DDL fixture at
`fixtures/catalog-v1.sql` and initializes from the checked-in v2 DDL artifact;
neither file is loaded from a runtime working directory or copied into the
destination deployment.

The crate is intentionally standalone and outside the production server
runtime. It does not participate in server startup or v1 writes.

## Section 20 implementation checklist

The converter's supported input contract is the checked-in `fixtures/catalog-v1.sql`
plus the two reviewed runtime tables allowed by the schema fingerprint. It validates
the complete normalized v1 schema, SQLite integrity/FKs, timestamps, path components,
formats, title ownership, key metadata, and source identity before creating a target.
The source writer lock is held for the whole operation. Every regular source file is
hashed into `source_physical_digest` (alongside the catalog digest), so `--resume`
rejects changed catalogs, object bytes, journal bytes, publications, or secrets.

The conversion phases are represented by the external manifest: static accounts and
sharing, per-document checkpoint/source closure and journal recovery, publications,
annotations/replies, reconciliation, and final verification. Object writes use a
same-directory temporary inode, fsync, and no-replace linking. A retry verifies an
existing object byte-for-byte. Per-document mappings, counts, object bytes, checkpoint
closures, account totals, server totals, target FKs, and target physical digests are
checked before completion. The target receives fresh deterministic session generations;
provider-qualified v1 account IDs are split at the matching `provider:` prefix while
retaining the v1 account ID as the target account identity. Preferences revisions,
bookmarks, onboarding entries, simple retention policy state, current publications,
deduplicated publication assets, and secret files are copied with their integrity data.

The journal decoder accepts only the reviewed framed KJBS/KJNL formats and v1 manifest
descriptors. It validates descriptor and fragment checksums, sequence/epoch coverage,
then applies the complete base plus every contiguous Yrs update before writing the
self-contained v2 journal base. Source recipes are decoded only as LPREC001, each
declared physical chunk is independently located, decompressed, sized, and hashed, and
v2 recipes use the runtime FastCDC/Zstandard profile. Checkpoint asset references are
matched against the actual tree before they are flattened into v2 closure rows.

The converter refuses unresolved recovery, catalog, key-rotation, maintenance,
deletion, erasure, publication, source-history, account-erasure, or unexpired-agent
work; deleting documents require an explicit partial allowlist. It also refuses
ambiguous active-key metadata without `--active-link-key`, malformed or unsupported
selectors/policies, missing committed roots, incomplete journal ranges, symlink
ancestors, oversized payloads, and onboarding entries that refer to excluded documents.
Runtime-only v1 ledgers and derived totals are intentionally settled into v2 counters or
removed after their inputs are verified; they are not copied as opaque rows. An
explicit partial conversion records every excluded document and remains identifiable as
partial in the completion manifest. The Rust tests in this crate cover the fixture
conversion, CRDT replay, multi-chunk source history, publication/annotation/reply and
secret conversion, dry-run, interrupted object staging, duplicate publication assets,
and changed-source resume rejection; the root agent runs them centrally.
