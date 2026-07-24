//! Tokio runtime ownership for synchronous language bindings.

use std::future::Future;
use std::io;

use tokio::runtime::Runtime;

/// Owns the private Tokio runtime used by a synchronous language binding.
///
/// Tokio's ordinary [`Runtime`] destructor performs a blocking shutdown and
/// therefore panics when the final owner is released from an asynchronous
/// Tokio context. Python embedders can invoke the native extension from such a
/// context, and binding handles can be finalized there as well. This wrapper
/// preserves the ordinary blocking shutdown outside Tokio while selecting
/// Tokio's non-blocking shutdown path when destruction happens inside any
/// active runtime.
pub struct ProviderRuntimeOwner {
    runtime: Option<Runtime>,
}

impl ProviderRuntimeOwner {
    /// Create a new multi-thread Tokio runtime owned by the binding.
    pub fn new() -> io::Result<Self> {
        Runtime::new().map(|runtime| Self {
            runtime: Some(runtime),
        })
    }

    /// Drive one provider future to completion on the owned runtime.
    pub fn block_on<F>(&self, future: F) -> F::Output
    where
        F: Future,
    {
        self.runtime().block_on(future)
    }

    /// Schedule one owned provider future on the binding-private executor.
    ///
    /// Async language bindings use this path instead of occupying their host
    /// runtime's shared blocking-worker pool for the lifetime of provider I/O.
    pub fn spawn<F>(&self, future: F) -> tokio::task::JoinHandle<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        self.runtime().spawn(future)
    }

    fn runtime(&self) -> &Runtime {
        match self.runtime.as_ref() {
            Some(runtime) => runtime,
            None => unreachable!("the runtime is present until ProviderRuntimeOwner::drop"),
        }
    }
}

impl Drop for ProviderRuntimeOwner {
    fn drop(&mut self) {
        let Some(runtime) = self.runtime.take() else {
            return;
        };

        if tokio::runtime::Handle::try_current().is_ok() {
            // This consumes the Runtime without waiting for its blocking pool,
            // which Tokio explicitly supports when dropping inside another
            // runtime. In particular, do not shuttle this value through a
            // fallible thread spawn: spawn failure would drop the captured
            // Runtime in the forbidden context before an error can be handled.
            runtime.shutdown_background();
        } else {
            drop(runtime);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::future::pending;
    use std::sync::Arc;
    use std::sync::mpsc;
    use std::time::Duration;

    use super::ProviderRuntimeOwner;

    #[test]
    fn final_owner_drop_is_safe_inside_current_thread_runtime() {
        let outer = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("current-thread host runtime should start");

        outer.block_on(async {
            let provider =
                Arc::new(ProviderRuntimeOwner::new().expect("provider runtime should start"));
            drop(provider);
        });
    }

    #[test]
    fn final_owner_drop_is_safe_inside_multi_thread_runtime() {
        let outer = tokio::runtime::Runtime::new().expect("multi-thread host runtime should start");

        outer.block_on(async {
            let provider =
                Arc::new(ProviderRuntimeOwner::new().expect("provider runtime should start"));
            drop(provider);
        });
    }

    #[test]
    fn stalled_provider_task_leaves_host_blocking_pool_available() {
        let provider =
            Arc::new(ProviderRuntimeOwner::new().expect("provider runtime should start"));
        let (provider_started, provider_ready) = mpsc::channel();
        let stalled = provider.spawn(async move {
            provider_started.send(()).expect("provider start signal");
            pending::<()>().await;
        });
        provider_ready
            .recv_timeout(Duration::from_secs(1))
            .expect("provider task should start");

        // Model the single shared blocking slot that an embedding runtime (in
        // Node, libuv) must retain for unrelated filesystem/crypto work. The
        // indefinitely pending provider future above runs elsewhere.
        let host = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .max_blocking_threads(1)
            .build()
            .expect("host runtime should start");
        let (host_work, host_ready) = mpsc::channel();
        host.spawn_blocking(move || host_work.send(7_u8).expect("host work result"));
        assert_eq!(
            host_ready
                .recv_timeout(Duration::from_secs(1))
                .expect("host blocking slot remains available"),
            7,
        );

        stalled.abort();
    }
}
