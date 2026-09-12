# SPEC: Room and storage

Work nobody has asked for yet, and limitations kept on purpose. Each item
says which.

## Left open

- Type `CatalogError::Conflict` so `CatalogError::refusal` stops classifying
  the catalogue's conflict prose by substring. One pinned function, about a
  hundred construction sites.
- Give `storage/backup.rs` a `Catalog` snapshot and verify API so it stops
  opening its own SQLite connection outside the execution boundary and
  shutdown.
- Paginate `catalog_entries` and `documents()`, which are unbounded reads.
  `Catalog::documents_page` already exists as the keyset primitive and has
  no callers yet.
- A room fenced as `FenceReason::Oversized` stays read-only until its
  instance is evicted, and eviction refuses a dirty session, so an oversized
  write leaves no way back short of a restart.

## References

- [Catalogue](../../crates/librepaper/src/storage/catalog/mod.rs) -- `CatalogError`, `catalog_entries`, `documents_page`.
- [Backup](../../crates/librepaper/src/storage/backup.rs) -- the connection opened outside the execution boundary.
- [Room errors](../../crates/librepaper/src/room/error.rs) -- `FenceReason`.
