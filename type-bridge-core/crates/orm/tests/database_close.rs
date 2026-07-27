use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use type_bridge_orm::session::backend::{BoxFuture, DriverBackend, TransactionOps};
use type_bridge_orm::{Database, OrmError, TxType};

struct CloseTrackingBackend {
    close_calls: Arc<AtomicUsize>,
}

impl DriverBackend for CloseTrackingBackend {
    fn open_transaction(
        &self,
        _database: &str,
        _tx_type: TxType,
    ) -> BoxFuture<'_, Result<Box<dyn TransactionOps>, OrmError>> {
        Box::pin(async { Err(OrmError::Transaction("not used by close test".to_owned())) })
    }

    fn is_open(&self) -> bool {
        true
    }

    fn close_connection(&self) -> Result<(), OrmError> {
        self.close_calls.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[test]
fn database_close_delegates_and_remains_repeatable() {
    let close_calls = Arc::new(AtomicUsize::new(0));
    let database = Database::with_backend(
        Box::new(CloseTrackingBackend {
            close_calls: Arc::clone(&close_calls),
        }),
        "close-test",
    );

    database.close().expect("first close should delegate");
    database
        .close()
        .expect("repeated close should delegate safely");

    assert_eq!(close_calls.load(Ordering::SeqCst), 2);
}
