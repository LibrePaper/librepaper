//! Opt-in recovery evidence for commands invoked through MCP.

use futures_util::future::BoxFuture;
use serde_json::Value;

use super::sequencer::{Command, CommandError, Evidence, Head, PreparedSource, Rung};
use crate::storage::postgres::{OperationReceipt, PostgresCatalog};

pub(crate) struct RecordedCommand<'a, C, F> {
    command: &'a mut C,
    receipt: OperationReceipt,
    outcome: F,
}

impl<'a, C, F> RecordedCommand<'a, C, F> {
    pub(crate) fn new(command: &'a mut C, receipt: OperationReceipt, outcome: F) -> Self {
        Self {
            command,
            receipt,
            outcome,
        }
    }
}

impl<C, F> Command for RecordedCommand<'_, C, F>
where
    C: Command + Send,
    F: Fn(&C::Output) -> Value + Send + Sync,
{
    type Output = C::Output;

    fn name(&self) -> &'static str {
        self.command.name()
    }
    fn authority(&self) -> Rung {
        self.command.authority()
    }
    fn replay(&mut self) -> BoxFuture<'_, Result<Option<Self::Output>, CommandError>> {
        self.command.replay()
    }
    fn load(&mut self) -> BoxFuture<'_, Result<(), CommandError>> {
        self.command.load()
    }
    fn evaluate(&mut self, head: &Head<'_>) -> Result<Option<PreparedSource>, CommandError> {
        self.command.evaluate(head)
    }
    fn transact<'a>(
        &'a mut self,
        tx: &'a mut sqlx::Transaction<'_, sqlx::Postgres>,
        evidence: &'a Evidence,
    ) -> BoxFuture<'a, Result<Self::Output, CommandError>> {
        Box::pin(async move {
            let output = self.command.transact(tx, evidence).await?;
            let outcome = (self.outcome)(&output);
            PostgresCatalog::record_operation_outcome(tx, &self.receipt, &outcome).await?;
            Ok(output)
        })
    }
}
