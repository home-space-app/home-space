//! The tokio runtime the app's tasks and subscriptions run on.
//!
//! iced's default runtime starts a worker per core. The workers only drive async I/O: sync
//! transfers share one task, and hashing, file removal and PDF rendering run on tokio's blocking
//! pool or their own threads. A few workers are enough, and each one costs an allocator arena.

use std::io;

/// Workers that drive the app's futures.
const WORKERS: usize = 4;

/// A multi-threaded tokio runtime with [`WORKERS`] workers.
pub struct Executor(tokio::runtime::Runtime);

impl iced::Executor for Executor {
    fn new() -> Result<Self, io::Error> {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(WORKERS)
            .enable_all()
            .build()
            .map(Self)
    }

    fn spawn(&self, future: impl Future<Output = ()> + Send + 'static) {
        drop(self.0.spawn(future));
    }

    fn block_on<T>(&self, future: impl Future<Output = T>) -> T {
        self.0.block_on(future)
    }

    fn enter<R>(&self, f: impl FnOnce() -> R) -> R {
        let _guard = self.0.enter();
        f()
    }
}
