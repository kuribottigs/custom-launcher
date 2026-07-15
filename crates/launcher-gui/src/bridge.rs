//! Bridge between gpui's executor and a tokio runtime.
//!
//! launcher-core is written against tokio (reqwest needs a tokio reactor),
//! while gpui drives its own executor. Futures are shipped to a shared
//! tokio runtime and the result is awaited through a oneshot channel.

use std::future::Future;

use once_cell::sync::Lazy;

static RUNTIME: Lazy<tokio::runtime::Runtime> = Lazy::new(|| {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("failed to build tokio runtime")
});

/// Run `fut` on the tokio runtime; await the result from any executor.
pub fn run_bg<T, F>(fut: F) -> impl Future<Output = T>
where
    F: Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    let (tx, rx) = futures::channel::oneshot::channel();
    RUNTIME.spawn(async move {
        let _ = tx.send(fut.await);
    });
    async move { rx.await.expect("background task panicked") }
}
