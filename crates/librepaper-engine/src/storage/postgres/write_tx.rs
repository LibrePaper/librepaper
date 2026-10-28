//! The one way a catalogue write transaction begins.
//!
//! `begin_write` meters the transaction, optionally checks the writer epoch
//! fence, and takes the row locks the write needs in one fixed order. Callers
//! name the fence and the locks instead of hand-writing the preamble.

use std::ops::{Deref, DerefMut};

use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use super::usage::lock_storage_usage;
use super::{PostgresCatalog, Result};

/// Whether a write must come from the process that holds the writer lease.
///
/// The epoch fence exists so that a process that lost the lease cannot commit
/// something that depends on the writer's in-memory state: the sequencer,
/// budgets and reservations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Fence {
    /// Anything a command, the sequencer or a byte reservation decides.
    Required,
    /// A plain row write that holds no writer state and that row locks
    /// already serialize. The reason is mandatory and names why.
    Exempt(&'static str),
}

/// A row lock taken at the start of a write transaction.
///
/// Locks are always acquired in the order of the variants below, whatever
/// order the caller lists them in, and a repeated lock is taken once. This
/// is the order the rest of the catalogue already follows: an account row
/// before the documents it owns, and the document row before the
/// deployment-wide accounting mutex. Every write path takes locks in this
/// one order, so two writers cannot deadlock on each other. A lock taken
/// later in a transaction, after other statements, must still respect it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Lock {
    /// The account row. Does not check that the row exists or its status.
    Account(Uuid),
    /// The document row. Does not check that the row exists or its status;
    /// each caller keeps its own status check.
    Document(Uuid),
    /// The deployment-wide accounting mutex.
    StorageUsage,
}

/// An open write transaction. Dropping it without `commit` rolls back.
pub(super) struct WriteTx<'c> {
    tx: Transaction<'c, Postgres>,
}

impl<'c> WriteTx<'c> {
    pub(super) async fn commit(self) -> sqlx::Result<()> {
        self.tx.commit().await
    }

    pub(super) async fn rollback(self) -> sqlx::Result<()> {
        self.tx.rollback().await
    }

    /// The bare transaction, for a caller that hands it across a trait seam.
    pub(super) fn into_inner(self) -> Transaction<'c, Postgres> {
        self.tx
    }
}

impl<'c> Deref for WriteTx<'c> {
    type Target = Transaction<'c, Postgres>;

    fn deref(&self) -> &Self::Target {
        &self.tx
    }
}

impl<'c> DerefMut for WriteTx<'c> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.tx
    }
}

impl PostgresCatalog {
    /// Begins a metered write transaction, checks the fence, then takes
    /// `locks` in the fixed order of `Lock`.
    pub(super) async fn begin_write(&self, fence: Fence, locks: &[Lock]) -> Result<WriteTx<'_>> {
        let mut tx = self.begin_metered().await?;
        match fence {
            Fence::Required => self.check_writer_epoch(&mut tx).await?,
            Fence::Exempt(_reason) => {}
        }
        let mut ordered = locks.to_vec();
        ordered.sort();
        ordered.dedup();
        for lock in ordered {
            match lock {
                Lock::StorageUsage => lock_storage_usage(&mut tx).await?,
                Lock::Account(id) => {
                    sqlx::query("SELECT id FROM accounts WHERE id=$1 FOR UPDATE")
                        .bind(id)
                        .fetch_optional(&mut *tx)
                        .await?;
                }
                Lock::Document(id) => {
                    sqlx::query("SELECT id FROM documents WHERE id=$1 FOR UPDATE")
                        .bind(id)
                        .fetch_optional(&mut *tx)
                        .await?;
                }
            }
        }
        Ok(WriteTx { tx })
    }
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::super::{Error, PostgresCatalog, PostgresOptions};
    use super::{Fence, Lock};

    async fn catalog() -> PostgresCatalog {
        let url = std::env::var("LIBREPAPER_TEST_POSTGRES_URL")
            .expect("set LIBREPAPER_TEST_POSTGRES_URL to a throwaway PostgreSQL database");
        let catalog = PostgresCatalog::connect(PostgresOptions::new(url))
            .await
            .expect("connect to PostgreSQL");
        catalog.migrate().await.expect("run PostgreSQL migrations");
        catalog
    }

    #[tokio::test]
    #[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
    async fn the_fence_refuses_a_superseded_writer_and_an_exempt_write_still_runs() {
        let catalog = catalog().await;
        let _writer = catalog.claim_writer().await.unwrap();
        let account = Uuid::now_v7();

        let current = catalog
            .begin_write(Fence::Required, &[Lock::Account(account)])
            .await
            .expect("the current writer passes the fence");
        current.rollback().await.unwrap();

        // A successor claiming the lease bumps the durable epoch.
        sqlx::query("UPDATE deployment_writer SET epoch=epoch+1 WHERE singleton")
            .execute(catalog.pool())
            .await
            .unwrap();

        let refused = catalog.begin_write(Fence::Required, &[]).await;
        assert!(matches!(refused, Err(Error::Ownership(_))));

        let exempt = catalog
            .begin_write(Fence::Exempt("test: a preference row write"), &[])
            .await
            .expect("an exempt write does not read the epoch");
        exempt.commit().await.unwrap();
    }

    #[test]
    fn locks_sort_into_the_documented_order() {
        let (a, b) = (Uuid::from_u128(1), Uuid::from_u128(2));
        let mut locks = vec![
            Lock::StorageUsage,
            Lock::Document(b),
            Lock::Account(b),
            Lock::Document(a),
            Lock::Account(a),
        ];
        locks.sort();
        assert_eq!(
            locks,
            vec![
                Lock::Account(a),
                Lock::Account(b),
                Lock::Document(a),
                Lock::Document(b),
                Lock::StorageUsage,
            ]
        );
    }
}
