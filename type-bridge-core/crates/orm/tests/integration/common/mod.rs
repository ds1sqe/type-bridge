pub mod dynamic_crud;
pub mod rust_binding;
pub mod typedb;

static INTEGRATION_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

pub async fn integration_test_guard() -> tokio::sync::MutexGuard<'static, ()> {
    INTEGRATION_LOCK.lock().await
}
