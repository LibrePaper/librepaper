//! Background loops with one way to stop them.
//!
//! A [`Tasks`] owns every long-lived task of one process part. Each task
//! learns of shutdown through a [`Shutdown`] token, and [`Tasks::stop`]
//! cancels the token and waits a bounded time for the tasks to exit.

use std::future::Future;
use std::time::Duration;
use tokio::task::{JoinHandle, JoinSet};
use tokio::time::MissedTickBehavior;
use tokio_util::sync::CancellationToken;

/// A cancellation signal shared by clones. Once cancelled it stays cancelled.
#[derive(Clone, Debug, Default)]
pub struct Shutdown(CancellationToken);

impl Shutdown {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.cancel();
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.is_cancelled()
    }

    /// Resolves once the token is cancelled, at once if it already is.
    pub async fn cancelled(&self) {
        self.0.cancelled().await;
    }
}

/// Aborts the task it holds when dropped, so a body that outlives its
/// supervising task is never left detached.
struct AbortOnDrop(JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Runs `body` as its own task and logs a panic under `name`.
async fn guarded<Fut>(name: &'static str, body: Fut)
where
    Fut: Future<Output = ()> + Send + 'static,
{
    let mut running = AbortOnDrop(tokio::spawn(body));
    if let Err(error) = (&mut running.0).await {
        if error.is_panic() {
            tracing::error!("background task {name} panicked: {error}");
        }
    }
}

pub struct Tasks {
    shutdown: Shutdown,
    set: JoinSet<&'static str>,
    names: Vec<&'static str>,
}

impl Default for Tasks {
    fn default() -> Self {
        Self::new()
    }
}

impl Tasks {
    pub fn new() -> Self {
        Self {
            shutdown: Shutdown::new(),
            set: JoinSet::new(),
            names: Vec::new(),
        }
    }

    /// The token this set cancels on [`Tasks::stop`].
    pub fn shutdown(&self) -> Shutdown {
        self.shutdown.clone()
    }

    /// Runs `f` once now and then every `period`. A tick in progress when
    /// shutdown arrives finishes; no further tick starts. A tick that panics
    /// is logged under `name` and the loop runs again on its next period.
    pub fn every<F, Fut>(
        &mut self,
        name: &'static str,
        period: Duration,
        missed: MissedTickBehavior,
        mut f: F,
    ) where
        F: FnMut() -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let shutdown = self.shutdown.clone();
        self.names.push(name);
        self.set.spawn(async move {
            let mut ticker = tokio::time::interval(period);
            ticker.set_missed_tick_behavior(missed);
            loop {
                tokio::select! {
                    biased;
                    _ = shutdown.cancelled() => break,
                    _ = ticker.tick() => {}
                }
                guarded(name, f()).await;
            }
            name
        });
    }

    /// Runs `f` once with the shutdown token. A panic is logged under `name`
    /// and the task is not restarted.
    pub fn spawn<F, Fut>(&mut self, name: &'static str, f: F)
    where
        F: FnOnce(Shutdown) -> Fut,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let body = f(self.shutdown.clone());
        self.names.push(name);
        self.set.spawn(async move {
            guarded(name, body).await;
            name
        });
    }

    /// Cancels the token and waits up to `grace` for the tasks to exit.
    /// Returns the names of those that did not; they are aborted.
    pub async fn stop(mut self, grace: Duration) -> Vec<&'static str> {
        self.shutdown.cancel();
        let drain = async {
            while let Some(done) = self.set.join_next().await {
                if let Ok(name) = done {
                    if let Some(at) = self.names.iter().position(|n| *n == name) {
                        self.names.swap_remove(at);
                    }
                }
            }
        };
        let _ = tokio::time::timeout(grace, drain).await;
        self.set.abort_all();
        self.names
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    #[tokio::test(start_paused = true)]
    async fn a_periodic_task_runs_every_period() {
        let count = Arc::new(AtomicUsize::new(0));
        let mut tasks = Tasks::new();
        let seen = count.clone();
        tasks.every(
            "count",
            Duration::from_secs(10),
            MissedTickBehavior::Delay,
            move || {
                let seen = seen.clone();
                async move {
                    seen.fetch_add(1, Ordering::SeqCst);
                }
            },
        );
        tokio::time::sleep(Duration::from_secs(35)).await;
        assert!(count.load(Ordering::SeqCst) >= 3);
        assert!(tasks.stop(Duration::from_secs(1)).await.is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn a_panicking_tick_runs_again_next_period() {
        let count = Arc::new(AtomicUsize::new(0));
        let mut tasks = Tasks::new();
        let seen = count.clone();
        tasks.every(
            "panics",
            Duration::from_secs(10),
            MissedTickBehavior::Delay,
            move || {
                let seen = seen.clone();
                async move {
                    if seen.fetch_add(1, Ordering::SeqCst) == 0 {
                        panic!("first tick fails");
                    }
                }
            },
        );
        tokio::time::sleep(Duration::from_secs(25)).await;
        assert!(count.load(Ordering::SeqCst) >= 2);
        assert!(tasks.stop(Duration::from_secs(1)).await.is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn stop_names_the_tasks_that_ignore_the_token() {
        let mut tasks = Tasks::new();
        tasks.spawn("stubborn", |_shutdown| async {
            std::future::pending::<()>().await;
        });
        tasks.spawn("polite", |shutdown| async move {
            shutdown.cancelled().await;
        });
        let late = tasks.stop(Duration::from_secs(1)).await;
        assert_eq!(late, vec!["stubborn"]);
    }

    #[tokio::test(start_paused = true)]
    async fn stop_returns_nothing_when_every_task_exits() {
        let mut tasks = Tasks::new();
        tasks.spawn("one", |shutdown| async move {
            shutdown.cancelled().await;
        });
        tasks.every(
            "two",
            Duration::from_secs(5),
            MissedTickBehavior::Skip,
            || async {},
        );
        assert!(tasks.stop(Duration::from_secs(1)).await.is_empty());
    }
}
