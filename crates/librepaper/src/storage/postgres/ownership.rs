use std::sync::atomic::Ordering;

use sqlx::pool::PoolConnection;
use sqlx::{Acquire, PgConnection, Postgres, Row};

use super::{Error, PostgresCatalog, Result};

/// Deployment-wide advisory lock id ("LPDOCOWN").  It is deliberately
/// distinct from the short-lived migration lock.
const WRITER_LOCK: i64 = 0x4c_50_44_4f_43_4f_57_4e_u64 as i64;

/// The dedicated PostgreSQL session whose lifetime admits this process as the
/// sole writer. Dropping it releases the advisory lock. Every document commit
/// additionally checks `epoch`, so a connection that has become stale cannot
/// commit through another pooled session after takeover.
pub struct WriterLease {
    connection: Option<PoolConnection<Postgres>>,
    epoch: i64,
}

impl Drop for WriterLease {
    fn drop(&mut self) {
        // Never return a session-scoped advisory lock to the pool. Detaching
        // makes dropping the underlying connection close that PostgreSQL
        // session, which releases the lock even when shutdown cannot await.
        if let Some(connection) = self.connection.take() {
            drop(connection.detach());
        }
    }
}

impl WriterLease {
    pub fn epoch(&self) -> i64 {
        self.epoch
    }

    /// Verify that the ownership session still exists. A failed probe consumes
    /// the connection, making the process unready before another command can
    /// be admitted.
    pub async fn verify(&mut self) -> Result<()> {
        let connection = self
            .connection
            .as_mut()
            .ok_or_else(|| Error::Ownership("writer ownership was lost".into()))?;
        sqlx::query("SELECT 1")
            .execute(&mut **connection)
            .await
            .map_err(|error| {
                self.connection.take();
                Error::Ownership(format!("writer ownership was lost: {error}"))
            })?;
        Ok(())
    }
}

impl PostgresCatalog {
    /// Refuses a write from a process that is no longer the writer.
    ///
    /// The fence is the durable `deployment_writer.epoch` compared against
    /// the one this process claimed. `FOR SHARE` holds it for the caller's
    /// transaction, so a successor that bumps the epoch waits behind this
    /// write rather than racing it.
    ///
    /// Takes the caller's transaction rather than opening one: the check
    /// only means anything inside the transaction the write happens in, and
    /// four places had written the same four lines against theirs.
    pub(crate) async fn check_writer_epoch(&self, tx: &mut PgConnection) -> Result<()> {
        let expected = self.writer_epoch()?;
        let durable: i64 =
            sqlx::query_scalar("SELECT epoch FROM deployment_writer WHERE singleton FOR SHARE")
                .fetch_one(tx)
                .await?;
        if durable != expected {
            return Err(Error::Ownership("writer epoch was superseded".into()));
        }
        Ok(())
    }

    pub(crate) async fn begin_writer_transaction(&self) -> Result<sqlx::Transaction<'_, Postgres>> {
        let mut tx = self.begin_metered().await?;
        self.check_writer_epoch(&mut tx).await?;
        Ok(tx)
    }

    /// Try to become the deployment writer. This never waits behind another
    /// process: only the holder may become ready and accept live commands.
    pub async fn claim_writer(&self) -> Result<WriterLease> {
        let mut connection = self.pool.acquire().await?;
        let held: bool = sqlx::query("SELECT pg_try_advisory_lock($1)")
            .bind(WRITER_LOCK)
            .fetch_one(&mut *connection)
            .await?
            .try_get(0)?;
        if !held {
            return Err(Error::Ownership(
                "another backend owns document writes".into(),
            ));
        }

        let mut tx = connection.begin().await?;
        let epoch: i64 = sqlx::query(
            "UPDATE deployment_writer SET epoch=epoch+1,activated_at=now() \
             WHERE singleton=true RETURNING epoch",
        )
        .fetch_one(&mut *tx)
        .await?
        .try_get(0)?;
        tx.commit().await?;
        self.writer_epoch.store(epoch, Ordering::Release);
        Ok(WriterLease {
            connection: Some(connection),
            epoch,
        })
    }

    pub fn writer_epoch(&self) -> Result<i64> {
        let epoch = self.writer_epoch.load(Ordering::Acquire);
        if epoch <= 0 {
            Err(Error::Ownership(
                "this process does not own document writes".into(),
            ))
        } else {
            Ok(epoch)
        }
    }
}
