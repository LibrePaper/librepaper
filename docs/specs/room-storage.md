# SPEC: Room and storage

Remaining room and storage work.

## Left open

- Type `CatalogError::Conflict` so `CatalogError::refusal` stops classifying
  the catalogue's conflict prose by substring. One pinned function, about a
  hundred construction sites.
- Give `storage/backup.rs` a `Catalog` snapshot and verify API so it stops
  opening its own SQLite connection outside the execution boundary and
  shutdown.

## References

- [Catalogue](../../crates/librepaper/src/storage/catalog/mod.rs) -- `CatalogError`, `catalog_entries`, `documents_page`.
- [Backup](../../crates/librepaper/src/storage/backup.rs) -- the connection opened outside the execution boundary.
