//! Dedicated engine thread for presage's `!Send` futures (#642 U9, KTD-8).
//!
//! Parts of presage-store-sqlite's futures are `!Send`, so the Manager
//! cannot live on siggy's multi-threaded runtime. The spike (#639) proved
//! the workable shape, the same one gurk uses: a dedicated OS thread running
//! a current-thread Tokio runtime driving a `LocalSet` that owns the
//! Manager, talking to the main loop over ordinary channels (which the
//! `Backend` trait's mpsc vocabulary already models).
//!
//! U9 lands the shim itself; U10-U12 put the Manager on it.

use std::thread;

use tokio::task::LocalSet;

/// Handle to the engine thread. Dropping it does not stop the thread; call
/// [`join`](EngineThread::join) for an orderly shutdown after the future
/// completes (U11's supervisor owns lifecycle beyond that).
pub struct EngineThread<T> {
    handle: thread::JoinHandle<T>,
}

impl<T: Send + 'static> EngineThread<T> {
    /// Spawn the dedicated thread and drive `make_future`'s output to
    /// completion on a `LocalSet`. The closure runs ON the engine thread,
    /// so the future it builds may be `!Send` (that is the whole point);
    /// only the closure itself and the result cross threads.
    pub fn spawn<F, Fut>(name: &str, make_future: F) -> std::io::Result<Self>
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: std::future::Future<Output = T>,
    {
        let handle = thread::Builder::new()
            .name(name.to_string())
            .spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("engine thread: tokio current-thread runtime");
                let local = LocalSet::new();
                local.block_on(&rt, make_future())
            })?;
        Ok(Self { handle })
    }

    /// Block until the engine future completes and return its output.
    pub fn join(self) -> thread::Result<T> {
        self.handle.join()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::rc::Rc;

    /// The reason this shim exists: a `!Send` value (`Rc`) held across an
    /// await point compiles and runs here, which `tokio::spawn` on the main
    /// runtime would reject at compile time.
    #[test]
    fn engine_thread_drives_non_send_futures() {
        let engine = EngineThread::spawn("test-engine", || async {
            let counter = Rc::new(std::cell::Cell::new(0));
            let c2 = Rc::clone(&counter);
            tokio::task::spawn_local(async move {
                c2.set(c2.get() + 41);
            })
            .await
            .unwrap();
            tokio::task::yield_now().await;
            counter.set(counter.get() + 1);
            counter.get()
        })
        .unwrap();
        assert_eq!(engine.join().unwrap(), 42);
    }

    #[test]
    fn engine_thread_returns_results_across_the_boundary() {
        let engine =
            EngineThread::spawn("test-engine-2", || async { String::from("engine result") })
                .unwrap();
        assert_eq!(engine.join().unwrap(), "engine result");
    }
}
