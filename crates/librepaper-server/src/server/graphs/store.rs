//! The SQLite layer under the graphs: one file, one writer, many readers.
//!
//! The schema is two tables. `samples` is `WITHOUT ROWID` with the
//! `(series, ts)` key, so a range read is an index scan and a row stays small.
//! Nothing here knows what a series means; `mod.rs` owns the catalog.

use std::path::Path;

use rusqlite::{named_params, params, Connection, OpenFlags, OptionalExtension};

use super::{Bucket, SERIES};

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS series (
  id   INTEGER PRIMARY KEY,
  name TEXT NOT NULL UNIQUE
);
CREATE TABLE IF NOT EXISTS samples (
  series INTEGER NOT NULL REFERENCES series(id),
  ts     INTEGER NOT NULL,
  value  REAL,
  PRIMARY KEY (series, ts)
) WITHOUT ROWID;
";

/// How long a connection waits on a lock, such as a checkpoint, before it
/// gives up.
const BUSY_TIMEOUT_MS: i64 = 1000;

/// The writer: one connection, and the id of each catalog series in catalog
/// order.
pub(super) struct Store {
    conn: Connection,
    ids: [i64; SERIES.len()],
}

impl Store {
    /// Open or create the file, set the pragmas, make the schema and read the
    /// catalog's ids back. A file that is not a database fails here.
    pub(super) fn open(path: &Path) -> rusqlite::Result<Store> {
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "busy_timeout", BUSY_TIMEOUT_MS)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.execute_batch(SCHEMA)?;
        let mut ids = [0; SERIES.len()];
        for (slot, series) in ids.iter_mut().zip(SERIES.iter()) {
            conn.execute(
                "INSERT OR IGNORE INTO series (name) VALUES (?1)",
                params![series.name],
            )?;
            *slot = conn.query_row(
                "SELECT id FROM series WHERE name = ?1",
                params![series.name],
                |row| row.get(0),
            )?;
        }
        Ok(Store { conn, ids })
    }

    /// Whether `PRAGMA quick_check` finds the file sound. It reads the whole
    /// file, so it belongs off the request path.
    pub(super) fn quick_check(&self) -> rusqlite::Result<bool> {
        let first: String = self
            .conn
            .pragma_query_value(None, "quick_check", |row| row.get(0))?;
        Ok(first == "ok")
    }

    /// One row per series for one second, in one transaction. A second that is
    /// already recorded is left as it was. A value that is not finite is
    /// stored as no value.
    pub(super) fn insert(
        &mut self,
        ts: i64,
        values: &[Option<f64>; SERIES.len()],
    ) -> rusqlite::Result<()> {
        let tx = self.conn.transaction()?;
        {
            let mut statement = tx.prepare_cached(
                "INSERT OR IGNORE INTO samples (series, ts, value) VALUES (?1, ?2, ?3)",
            )?;
            for (id, value) in self.ids.iter().zip(values.iter()) {
                let value = value.filter(|value| value.is_finite());
                statement.execute(params![id, ts, value])?;
            }
        }
        tx.commit()
    }

    /// Delete every sample older than `cutoff`, one statement per series so
    /// each walks the primary key. Series the catalog no longer lists are
    /// trimmed too.
    pub(super) fn trim(&mut self, cutoff: i64) -> rusqlite::Result<()> {
        let ids: Vec<i64> = {
            let mut statement = self.conn.prepare("SELECT id FROM series")?;
            let rows = statement.query_map([], |row| row.get::<_, i64>(0))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        let tx = self.conn.transaction()?;
        for id in ids {
            tx.execute(
                "DELETE FROM samples WHERE series = ?1 AND ts < ?2",
                params![id, cutoff],
            )?;
        }
        tx.commit()
    }
}

/// A read-only connection for one request. WAL lets it read while the writer
/// works.
pub(super) fn read_only(path: &Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    conn.pragma_update(None, "busy_timeout", BUSY_TIMEOUT_MS)?;
    Ok(conn)
}

/// Every catalog series bucketed onto `points` cells across `[from, to)`, in
/// catalog order. Cell `i` holds the series' bucket function over the samples
/// that fall in it, or `None` when it has none. Needs `from < to` and
/// `points > 0`.
pub(super) fn read_range(
    path: &Path,
    from: i64,
    to: i64,
    points: u32,
) -> rusqlite::Result<Vec<Vec<Option<f64>>>> {
    let conn = read_only(path)?;
    let cells = points as usize;
    let mut all = Vec::with_capacity(SERIES.len());
    for series in SERIES.iter() {
        let id: Option<i64> = conn
            .query_row(
                "SELECT id FROM series WHERE name = ?1",
                params![series.name],
                |row| row.get(0),
            )
            .optional()?;
        let mut values = vec![None; cells];
        if let Some(id) = id {
            let function = match series.bucket {
                Bucket::Mean => "AVG",
                Bucket::Max => "MAX",
            };
            let mut statement = conn.prepare(&format!(
                "SELECT CAST((ts - :from) * :points / (:to - :from) AS INTEGER) AS bucket, \
                 {function}(value) FROM samples \
                 WHERE series = :id AND ts >= :from AND ts < :to AND value IS NOT NULL \
                 GROUP BY bucket ORDER BY bucket"
            ))?;
            let rows = statement.query_map(
                named_params! {":from": from, ":to": to, ":points": i64::from(points), ":id": id},
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, Option<f64>>(1)?)),
            )?;
            for row in rows {
                let (bucket, value) = row?;
                if let Some(cell) = usize::try_from(bucket)
                    .ok()
                    .and_then(|bucket| values.get_mut(bucket))
                {
                    *cell = value.filter(|value| value.is_finite());
                }
            }
        }
        all.push(values);
    }
    Ok(all)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("metrics.sqlite");
        (dir, path)
    }

    /// Twelve values where only the first two series carry one: series 0 is a
    /// mean series and series 1 a max series.
    fn pair(mean: f64, max: f64) -> [Option<f64>; SERIES.len()] {
        let mut values = [None; SERIES.len()];
        values[0] = Some(mean);
        values[1] = Some(max);
        values
    }

    fn count(path: &Path) -> i64 {
        read_only(path)
            .unwrap()
            .query_row("SELECT COUNT(*) FROM samples", [], |row| row.get(0))
            .unwrap()
    }

    #[test]
    fn open_creates_the_schema_in_wal_mode() {
        let (_dir, path) = temp();
        let store = Store::open(&path).unwrap();
        let mode: String = store
            .conn
            .pragma_query_value(None, "journal_mode", |row| row.get(0))
            .unwrap();
        assert_eq!(mode, "wal");
        let names: i64 = store
            .conn
            .query_row("SELECT COUNT(*) FROM series", [], |row| row.get(0))
            .unwrap();
        assert_eq!(names, SERIES.len() as i64);
        assert!(store.quick_check().unwrap());
        let sql: String = store
            .conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE name = 'samples'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(sql.contains("WITHOUT ROWID"));
    }

    #[test]
    fn opening_again_keeps_the_ids() {
        let (_dir, path) = temp();
        let first = Store::open(&path).unwrap().ids;
        let second = Store::open(&path).unwrap().ids;
        assert_eq!(first, second);
        assert_eq!(first.len(), 12);
    }

    #[test]
    fn insert_then_read_back_on_the_grid() {
        let (_dir, path) = temp();
        let mut store = Store::open(&path).unwrap();
        store.insert(1_000, &pair(3.0, 5.0)).unwrap();
        store.insert(1_060, &pair(4.0, 6.0)).unwrap();
        // Two cells of 60 seconds each.
        let read = read_range(&path, 1_000, 1_120, 2).unwrap();
        assert_eq!(read.len(), SERIES.len());
        assert_eq!(read[0], vec![Some(3.0), Some(4.0)]);
        assert_eq!(read[1], vec![Some(5.0), Some(6.0)]);
        assert_eq!(read[2], vec![None, None]);
    }

    #[test]
    fn a_repeated_second_is_ignored() {
        let (_dir, path) = temp();
        let mut store = Store::open(&path).unwrap();
        store.insert(1_000, &pair(3.0, 5.0)).unwrap();
        store.insert(1_000, &pair(99.0, 99.0)).unwrap();
        assert_eq!(count(&path), 12);
        let read = read_range(&path, 1_000, 1_060, 1).unwrap();
        assert_eq!(read[0], vec![Some(3.0)]);
    }

    #[test]
    fn a_null_round_trips_and_is_not_a_zero() {
        let (_dir, path) = temp();
        let mut store = Store::open(&path).unwrap();
        let mut values = pair(3.0, 5.0);
        values[0] = None;
        store.insert(1_000, &values).unwrap();
        let nulls: i64 = store
            .conn
            .query_row(
                "SELECT COUNT(*) FROM samples WHERE value IS NULL AND ts = 1000",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(nulls, 11);
        let read = read_range(&path, 1_000, 1_060, 1).unwrap();
        assert_eq!(read[0], vec![None]);
        assert_eq!(read[1], vec![Some(5.0)]);
    }

    #[test]
    fn a_value_that_is_not_finite_becomes_no_value() {
        let (_dir, path) = temp();
        let mut store = Store::open(&path).unwrap();
        let mut values = pair(f64::NAN, f64::INFINITY);
        values[2] = Some(f64::NEG_INFINITY);
        store.insert(1_000, &values).unwrap();
        let read = read_range(&path, 1_000, 1_060, 1).unwrap();
        assert_eq!(read[0], vec![None]);
        assert_eq!(read[1], vec![None]);
        assert_eq!(read[2], vec![None]);
    }

    #[test]
    fn trim_keeps_exactly_the_rows_at_or_after_the_cutoff() {
        let (_dir, path) = temp();
        let mut store = Store::open(&path).unwrap();
        for ts in [100, 200, 300, 400] {
            store.insert(ts, &pair(ts as f64, ts as f64)).unwrap();
        }
        store.trim(300).unwrap();
        assert_eq!(count(&path), 24);
        let read = read_range(&path, 0, 500, 5).unwrap();
        assert_eq!(read[0], vec![None, None, None, Some(300.0), Some(400.0)]);
    }

    #[test]
    fn trim_also_removes_a_series_the_catalog_dropped() {
        let (_dir, path) = temp();
        let mut store = Store::open(&path).unwrap();
        store
            .conn
            .execute("INSERT INTO series (name) VALUES ('retired')", [])
            .unwrap();
        let id = store.conn.last_insert_rowid();
        for ts in [100, 500] {
            store
                .conn
                .execute(
                    "INSERT INTO samples (series, ts, value) VALUES (?1, ?2, 1.0)",
                    params![id, ts],
                )
                .unwrap();
        }
        store.trim(300).unwrap();
        let left: Vec<i64> = {
            let mut statement = store
                .conn
                .prepare("SELECT ts FROM samples WHERE series = ?1")
                .unwrap();
            let rows = statement.query_map(params![id], |row| row.get(0)).unwrap();
            rows.map(Result::unwrap).collect()
        };
        assert_eq!(left, vec![500]);
    }

    #[test]
    fn bucketing_is_the_mean_for_a_mean_series_and_the_max_for_a_max_series() {
        let (_dir, path) = temp();
        assert_eq!(SERIES[0].bucket, Bucket::Mean);
        assert_eq!(SERIES[1].bucket, Bucket::Max);
        let mut store = Store::open(&path).unwrap();
        // Three samples in the first of two cells, none in the second.
        store.insert(0, &pair(1.0, 1.0)).unwrap();
        store.insert(60, &pair(2.0, 9.0)).unwrap();
        store.insert(120, &pair(6.0, 2.0)).unwrap();
        let read = read_range(&path, 0, 360, 2).unwrap();
        assert_eq!(read[0], vec![Some(3.0), None]);
        assert_eq!(read[1], vec![Some(9.0), None]);
    }

    #[test]
    fn the_first_second_of_a_cell_belongs_to_it_and_the_end_does_not() {
        let (_dir, path) = temp();
        let mut store = Store::open(&path).unwrap();
        store.insert(99, &pair(1.0, 1.0)).unwrap();
        store.insert(100, &pair(2.0, 2.0)).unwrap();
        store.insert(150, &pair(3.0, 3.0)).unwrap();
        store.insert(200, &pair(4.0, 4.0)).unwrap();
        let read = read_range(&path, 100, 200, 2).unwrap();
        assert_eq!(read[0], vec![Some(2.0), Some(3.0)]);
    }

    #[test]
    fn a_read_only_connection_cannot_write() {
        let (_dir, path) = temp();
        let _writer = Store::open(&path).unwrap();
        let conn = read_only(&path).unwrap();
        let attempt = conn.execute(
            "INSERT INTO samples (series, ts, value) VALUES (1, 1, 1.0)",
            [],
        );
        assert!(attempt.is_err());
        assert!(conn.execute("DELETE FROM samples", []).is_err());
    }

    #[test]
    fn a_garbage_file_does_not_open() {
        let (_dir, path) = temp();
        std::fs::write(&path, vec![0x5a; 4096]).unwrap();
        assert!(Store::open(&path).is_err());
    }
}
