//! A provider wrapper that makes chosen commits overlap.
//!
//! TypeDB validates conflicts at commit: of two transactions whose snapshots
//! both predate the other's commit, the first committer wins. A staged
//! transaction opens, runs and commits inside one `commit()` call, so racing
//! two of them only sometimes overlaps. Arming the gate for `n` commits holds
//! each of the next `n` provider commits until all `n` have run their
//! queries, which makes every one of them concurrent with the others.
#![allow(dead_code)]

use std::sync::{Arc, Mutex};

use tokio::sync::Barrier;
use type_bridge_orm::ClassifiedCommitError;
use type_bridge_orm::OrmError;
use type_bridge_orm::session::backend::{
    AnswerConsumer, BoundedAnswerLimits, BoundedAnswerStats, BoxFuture, DriverBackend,
    GivenRowsSpec, QueryResult, TransactionOps, TxType,
};
use type_bridge_orm::session::staged::TransactionOptions;

/// Commits still to hold, and the barrier they meet at.
type Armed = Option<(usize, Arc<Barrier>)>;

/// Which commits wait for each other. Clones share one gate.
#[derive(Clone, Default)]
pub struct CommitGate(Arc<Mutex<Armed>>);

impl CommitGate {
    /// Hold the next `parties` provider commits until all of them arrive.
    pub fn arm(&self, parties: usize) {
        *self.0.lock().unwrap() = Some((parties, Arc::new(Barrier::new(parties))));
    }

    fn take(&self) -> Option<Arc<Barrier>> {
        let mut armed = self.0.lock().unwrap();
        let (remaining, barrier) = armed.take()?;
        if remaining > 1 {
            *armed = Some((remaining - 1, Arc::clone(&barrier)));
        }
        Some(barrier)
    }
}

/// A backend whose transactions commit through a [`CommitGate`].
pub struct GatedBackend<B> {
    inner: B,
    gate: CommitGate,
}

impl<B> GatedBackend<B> {
    /// Wrap `inner`; commits pass straight through until `gate` is armed.
    pub fn new(inner: B, gate: CommitGate) -> Self {
        Self { inner, gate }
    }
}

impl<B: DriverBackend> DriverBackend for GatedBackend<B> {
    fn match_capabilities(&self) -> type_bridge_orm::CapabilitySet {
        self.inner.match_capabilities()
    }

    fn open_transaction(
        &self,
        database: &str,
        tx_type: TxType,
    ) -> BoxFuture<'_, Result<Box<dyn TransactionOps>, OrmError>> {
        let gate = self.gate.clone();
        let opened = self.inner.open_transaction(database, tx_type);
        Box::pin(async move {
            let inner = opened.await?;
            Ok(Box::new(GatedTransaction { inner, gate }) as Box<dyn TransactionOps>)
        })
    }

    fn open_with_options(
        &self,
        database: &str,
        tx_type: TxType,
        options: &TransactionOptions,
    ) -> BoxFuture<'_, Result<Box<dyn TransactionOps>, OrmError>> {
        let gate = self.gate.clone();
        let opened = self.inner.open_with_options(database, tx_type, options);
        Box::pin(async move {
            let inner = opened.await?;
            Ok(Box::new(GatedTransaction { inner, gate }) as Box<dyn TransactionOps>)
        })
    }

    fn is_open(&self) -> bool {
        self.inner.is_open()
    }

    fn close_connection(&self) -> Result<(), OrmError> {
        self.inner.close_connection()
    }

    fn server_version(&self) -> Option<type_bridge_core_lib::version::Version> {
        self.inner.server_version()
    }

    fn supports_given_rows(&self) -> bool {
        self.inner.supports_given_rows()
    }

    fn database_exists(&self, database: &str) -> BoxFuture<'_, Result<bool, OrmError>> {
        self.inner.database_exists(database)
    }

    fn create_database(&self, database: &str) -> BoxFuture<'_, Result<(), OrmError>> {
        self.inner.create_database(database)
    }

    fn delete_database(&self, database: &str) -> BoxFuture<'_, Result<(), OrmError>> {
        self.inner.delete_database(database)
    }

    fn schema_text(&self, database: &str) -> BoxFuture<'_, Result<String, OrmError>> {
        self.inner.schema_text(database)
    }
}

struct GatedTransaction {
    inner: Box<dyn TransactionOps>,
    gate: CommitGate,
}

impl TransactionOps for GatedTransaction {
    fn supports_given_rows(&self) -> bool {
        self.inner.supports_given_rows()
    }

    fn schema_snapshot(&mut self) -> BoxFuture<'_, Result<Option<String>, OrmError>> {
        self.inner.schema_snapshot()
    }

    fn query(&mut self, typeql: &str) -> BoxFuture<'_, Result<QueryResult, OrmError>> {
        self.inner.query(typeql)
    }

    fn query_canonical<'a>(
        &'a mut self,
        typeql: &str,
    ) -> BoxFuture<'a, Result<QueryResult, OrmError>> {
        self.inner.query_canonical(typeql)
    }

    fn query_with_rows(
        &mut self,
        typeql: &str,
        rows: GivenRowsSpec,
    ) -> BoxFuture<'_, Result<QueryResult, OrmError>> {
        self.inner.query_with_rows(typeql, rows)
    }

    fn query_with_rows_bounded<'a>(
        &'a mut self,
        typeql: &'a str,
        rows: GivenRowsSpec,
        limits: BoundedAnswerLimits,
        consumer: &'a mut dyn AnswerConsumer,
    ) -> BoxFuture<'a, Result<BoundedAnswerStats, OrmError>> {
        self.inner
            .query_with_rows_bounded(typeql, rows, limits, consumer)
    }

    fn query_materialized_bounded<'a>(
        &'a mut self,
        typeql: &'a str,
        rows: Option<GivenRowsSpec>,
        limits: type_bridge_orm::session::backend::AnswerLimits,
        canonical: bool,
    ) -> BoxFuture<'a, Result<QueryResult, OrmError>> {
        self.inner
            .query_materialized_bounded(typeql, rows, limits, canonical)
    }

    fn query_bounded<'a>(
        &'a mut self,
        typeql: &'a str,
        limits: BoundedAnswerLimits,
        consumer: &'a mut dyn AnswerConsumer,
    ) -> BoxFuture<'a, Result<BoundedAnswerStats, OrmError>> {
        self.inner.query_bounded(typeql, limits, consumer)
    }

    fn commit(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
        let barrier = self.gate.take();
        Box::pin(async move {
            if let Some(barrier) = barrier {
                barrier.wait().await;
            }
            self.inner.commit().await
        })
    }

    fn commit_classified(&mut self) -> BoxFuture<'_, Result<(), ClassifiedCommitError>> {
        let barrier = self.gate.take();
        Box::pin(async move {
            if let Some(barrier) = barrier {
                barrier.wait().await;
            }
            self.inner.commit_classified().await
        })
    }

    fn rollback(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
        self.inner.rollback()
    }

    fn close(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
        self.inner.close()
    }
}
