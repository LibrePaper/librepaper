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
successful conversion includes the v1 DDL fixture at `fixtures/catalog-v1.sql`
and uses the checked-in v2 DDL artifact; neither file is loaded from a runtime
working directory.

The crate is intentionally outside the production server crate. Add
`tools/catalog-v1-import` to the workspace only when the root agent is ready to
build the converter; this tool does not participate in server startup or v1
writes.
