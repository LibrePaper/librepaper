# Frugal storage inventory

This is a read-only inventory of the persistent local development deployment,
not a production sample. The target was the existing `librepaper-postgres`
container/database (`127.0.0.1:55432/librepaper`); the unrelated `lp-cap`
PostgreSQL service was not inspected. The query is [inventory.sql](inventory.sql)
and every statement ran inside `BEGIN READ ONLY`. Raw output is summarized in
[raw-dev.tsv](raw-dev.tsv). No migrations, writes, or benchmark databases were
used.

The catalogue has 6 active documents: 5 starter documents owned by one
account and one other document. Three starters have an owner `document_marks`
row with `opened_at`; two do not, so the observed never-opened fraction is
2/5 = 40%. This is one local account and is not an adoption estimate.

The five starter asset rows total 58,470 bytes physically. All five have the
same digest and are 11,694 bytes each, so the current deployment stores five
copies; one global copy would save 46,776 bytes for this account. This confirms
the implementation-level finding: asset deduplication is per document, while
the object key includes document ID and a random asset ID. The source starter
payloads total 73,502 raw bytes/account; details are in
[starter-payload.tsv](starter-payload.tsv).

The local object store contains exactly five files totaling 58,470 bytes,
matching the asset rows. There are no repo-local backup manifests or dumps.
The database is stale relative to the current repository: it has legacy
`document_bases` and no `document_snapshots`; both legacy base tables are empty,
so current/retired snapshot bytes cannot be measured here. It has 11 update rows
totaling 34,098 bytes, no archives, and 794,624 bytes of user-table relation
storage inside an 8,976,051-byte database.

The current-schema query reports known snapshot bytes and the number of rows
with unknown sizes separately. Migrated legacy retired snapshots can lack size
metadata; an object-store listing is needed to account for those bytes.

The cheapest next measurement is the same read-only query against a deliberately
identified current-schema deployment, plus object-store listing totals. For
starter savings, track `document_marks.opened_at` for starter slugs over a
representative account cohort; lazy materialization only saves bytes for the
never-opened fraction. Global asset dedup is measurable and real, but requires
a shared-object/reference schema because current asset keys are document-scoped.
