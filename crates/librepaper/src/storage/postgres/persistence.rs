//! Own the pooled connection for the entire lifetime of persistence scratch.
//!
//! Transactions borrow this guard. On cancellation they drop first, then this
//! guard destroys the connection synchronously, including any unsent Bind
//! buffer. Successful writes shrink the buffers before returning to the pool.
use std::sync::Arc;

use sqlx::{pool::PoolConnection, Connection, PgConnection, Postgres, Transaction};

use super::{meter, PostgresCatalog, Result};
use crate::log::pending::Reservation;

pub(crate) struct PersistenceConnection {
    connection: Option<PoolConnection<Postgres>>,
    // Dropped after the connection, including when its transaction is cancelled.
    _scratch: Reservation,
    reusable: bool,
    attempt: Option<meter::Attempt>,
    meter: Arc<meter::PoolMeter>,
}

impl PostgresCatalog {
    pub(crate) async fn persistence_connection(
        &self,
        scratch: Reservation,
    ) -> Result<PersistenceConnection> {
        let contended = self.pool.num_idle() == 0 && self.pool.size() >= self.max_connections;
        let attempt = meter::PoolMeter::attempt(contended);
        let connection = match self.pool.acquire().await {
            Ok(connection) => connection,
            Err(error) => {
                self.meter.record(attempt, false);
                return Err(error.into());
            }
        };
        let mut connection = connection;
        connection.shrink_buffers();
        Ok(PersistenceConnection {
            connection: Some(connection),
            _scratch: scratch,
            reusable: false,
            attempt: Some(attempt),
            meter: self.meter.clone(),
        })
    }
}

impl PersistenceConnection {
    pub(crate) async fn begin(&mut self) -> Result<Transaction<'_, Postgres>> {
        let begun = self
            .connection
            .as_mut()
            .expect("persistence owns its connection")
            .begin()
            .await;
        if let Some(attempt) = self.attempt.take() {
            self.meter.record(attempt, begun.is_ok());
        }
        Ok(begun?)
    }

    /// Called only after the transaction has committed or rolled back. It has
    /// no awaits: cancellation cannot fall between shrinking and marking safe.
    pub(crate) fn complete(&mut self) {
        let connection = self
            .connection
            .as_mut()
            .expect("persistence owns its connection");
        if !connection.is_in_transaction() && !connection.should_flush() {
            connection.shrink_buffers();
            self.reusable = true;
        }
    }
}

impl Drop for PersistenceConnection {
    fn drop(&mut self) {
        if let Some(attempt) = self.attempt.take() {
            self.meter.record(attempt, false);
        }
        if let Some(connection) = self.connection.take() {
            if self.reusable {
                drop(connection);
            } else {
                // PoolConnection::drop schedules asynchronous cleanup. Detach
                // and drop the raw connection instead, so its buffers cannot
                // outlive this reservation. PostgreSQL rolls back on EOF.
                let raw: PgConnection = connection.detach();
                drop(raw);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::log::pending::{scratch_for, PendingBudget};
    use crate::storage::postgres::PostgresOptions;
    use std::time::Duration;

    async fn catalog() -> PostgresCatalog {
        let url = std::env::var("LIBREPAPER_TEST_POSTGRES_URL").expect("test PostgreSQL URL");
        let mut options = PostgresOptions::new(url);
        options.max_connections = 1;
        PostgresCatalog::connect(options).await.expect("connect")
    }

    #[tokio::test]
    #[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
    async fn completed_writes_release_scratch_and_reuse_the_connection() {
        let catalog = catalog().await;
        let budget = PendingBudget::new(1, scratch_for(2 * 1024 * 1024));
        let mut connection = catalog
            .persistence_connection(budget.try_scratch(budget.scratch_limit()).expect("scratch"))
            .await
            .expect("connection");
        let mut tx = connection.begin().await.expect("begin");
        let bytes = vec![7u8; 2 * 1024 * 1024];
        let pid: i32 =
            sqlx::query_scalar("SELECT pg_backend_pid() FROM (SELECT octet_length($1::bytea)) q")
                .bind(&bytes)
                .fetch_one(&mut *tx)
                .await
                .expect("large bind");
        tx.commit().await.expect("commit");
        assert!(budget.scratch_used() > 0);
        connection.complete();
        assert!(connection.reusable);
        drop(connection);
        assert_eq!(budget.scratch_used(), 0);
        let reused: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(catalog.pool())
            .await
            .expect("reacquire");
        assert_eq!(
            pid, reused,
            "a successful write should preserve connection reuse"
        );
        catalog.close().await;
    }

    #[tokio::test]
    #[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
    async fn cancelling_a_write_destroys_its_connection_before_releasing_scratch() {
        let catalog = catalog().await;
        let budget = PendingBudget::new(1, scratch_for(2 * 1024 * 1024));
        let mut connection = catalog
            .persistence_connection(budget.try_scratch(budget.scratch_limit()).expect("scratch"))
            .await
            .expect("connection");
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut **connection.connection.as_mut().expect("connection"))
            .await
            .expect("pid");
        let url = std::env::var("LIBREPAPER_TEST_POSTGRES_URL").expect("URL");
        let mut observer = PgConnection::connect(&url).await.expect("observer");
        let writing = tokio::spawn(async move {
            let mut tx = connection.begin().await.expect("begin");
            let bytes = vec![7u8; 2 * 1024 * 1024];
            sqlx::query("SELECT pg_sleep(10), octet_length($1::bytea)")
                .bind(&bytes)
                .execute(&mut *tx)
                .await
                .expect("query");
            tx.commit().await.expect("commit");
            connection.complete();
        });
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let sleeping: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE pid=$1 AND wait_event='PgSleep')").bind(pid).fetch_one(&mut observer).await.expect("activity");
                if sleeping { break; }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }).await.expect("query reached PostgreSQL");
        assert!(budget.scratch_used() > 0);
        writing.abort();
        assert!(writing.await.expect_err("cancelled").is_cancelled());
        assert_eq!(budget.scratch_used(), 0);
        assert_eq!(catalog.pool().size(), 0, "cancelled connection must be destroyed, not returned asynchronously with retained buffers");
        let replacement: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(catalog.pool())
            .await
            .expect("replacement");
        assert_ne!(pid, replacement);
        observer.close().await.expect("close observer");
        catalog.close().await;
    }
}
