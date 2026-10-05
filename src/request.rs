//! Completion proof for detached owners and non-abortable workers.
use anyhow::{anyhow, Result};
use std::sync::{Arc, Mutex};
use tokio::sync::watch;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub(crate) enum Stage {
    Admission,
    Capture,
    Execution,
    Evidence,
    Publication,
    Complete,
}
impl Stage {
    pub(crate) fn step(self) -> f64 {
        self as u8 as f64 + 1.0
    }
    pub(crate) fn message(self) -> &'static str {
        match self {
            Self::Admission => "Waiting for admission",
            Self::Capture => "Capturing inputs",
            Self::Execution => "Executing operation",
            Self::Evidence => "Collecting evidence",
            Self::Publication => "Publishing result",
            Self::Complete => "Operation completed",
        }
    }
}

#[derive(Clone, Default)]
pub(crate) struct CompletionRegistry(Arc<Mutex<Registry>>);
#[derive(Default)]
struct Registry {
    closing: bool,
    incomplete: bool,
    owners: Vec<Owner>,
}
struct Owner {
    cancellation: Option<Arc<watch::Sender<bool>>>,
    completion: watch::Receiver<Option<bool>>,
}
pub(crate) struct CompletionGuard(Option<watch::Sender<Option<bool>>>);
impl CompletionGuard {
    pub(crate) fn finish(mut self, cleanup_confirmed: bool) {
        if let Some(sender) = self.0.take() {
            sender.send_replace(Some(cleanup_confirmed));
        }
    }
}
impl Drop for CompletionGuard {
    fn drop(&mut self) {
        if let Some(sender) = self.0.take() {
            sender.send_replace(Some(false));
        }
    }
}
impl CompletionRegistry {
    pub(crate) fn register(
        &self,
        cancellation: Option<Arc<watch::Sender<bool>>>,
    ) -> Result<CompletionGuard> {
        let mut state = self.0.lock().unwrap_or_else(|error| error.into_inner());
        anyhow::ensure!(!state.closing, "server is shutting down");
        let mut incomplete = false;
        state
            .owners
            .retain(|owner| match *owner.completion.borrow() {
                None => true,
                Some(clean) => {
                    incomplete |= !clean;
                    false
                }
            });
        state.incomplete |= incomplete;
        let (sender, completion) = watch::channel(None);
        state.owners.push(Owner {
            cancellation,
            completion,
        });
        Ok(CompletionGuard(Some(sender)))
    }
    pub(crate) fn closing(&self) -> bool {
        self.0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .closing
    }
    pub(crate) async fn cancel_and_drain(&self, deadline: tokio::time::Instant) -> Result<()> {
        let (owners, incomplete) = {
            let mut state = self.0.lock().unwrap_or_else(|error| error.into_inner());
            state.closing = true;
            for owner in &state.owners {
                if let Some(sender) = &owner.cancellation {
                    sender.send_replace(true);
                }
            }
            (
                state
                    .owners
                    .iter()
                    .map(|owner| owner.completion.clone())
                    .collect::<Vec<_>>(),
                state.incomplete,
            )
        };
        let mut clean = !incomplete;
        for mut owner in owners {
            let ended = tokio::time::timeout_at(deadline, owner.wait_for(Option::is_some))
                .await
                .map_err(|_| anyhow!("owned work exceeded the shutdown cleanup deadline"))?
                .map_err(|_| anyhow!("owned work ended without completion proof"))?;
            clean &= *ended == Some(true);
        }
        anyhow::ensure!(
            clean,
            "owned work cleanup was not confirmed; recovery may be required"
        );
        Ok(())
    }
}

#[derive(Clone)]
pub(crate) struct BlockingPool {
    permits: Arc<tokio::sync::Semaphore>,
    owners: CompletionRegistry,
}

impl BlockingPool {
    pub(crate) fn new(limit: usize) -> Self {
        Self {
            permits: Arc::new(tokio::sync::Semaphore::new(limit.max(1))),
            owners: Default::default(),
        }
    }
    pub(crate) async fn run<T: Send + 'static>(
        &self,
        work: impl FnOnce() -> Result<T> + Send + 'static,
    ) -> Result<T> {
        self.run_tracked(work, None).await
    }
    pub(crate) async fn run_tracked<T: Send + 'static>(
        &self,
        work: impl FnOnce() -> Result<T> + Send + 'static,
        request: Option<CompletionRegistry>,
    ) -> Result<T> {
        self.run_admitted(work, request, None).await
    }
    pub(crate) async fn run_admitted<T: Send + 'static>(
        &self,
        work: impl FnOnce() -> Result<T> + Send + 'static,
        request: Option<CompletionRegistry>,
        deadline: Option<tokio::time::Instant>,
    ) -> Result<T> {
        let admission = self.permits.clone().acquire_owned();
        let permit = match deadline {
            Some(deadline) => tokio::time::timeout_at(deadline, admission)
                .await
                .map_err(|_| anyhow!("request_timed_out"))??,
            None => admission.await?,
        };
        let completion = self.owners.register(None)?;
        let request_completion = request.map(|request| request.register(None)).transpose()?;
        struct Completed<T> {
            value: Option<Result<T>>,
            _permit: tokio::sync::OwnedSemaphorePermit,
            completion: Option<CompletionGuard>,
            request_completion: Option<CompletionGuard>,
        }
        impl<T> Drop for Completed<T> {
            fn drop(&mut self) {
                // An abandoned result can own a lease whose destructor must
                // finish before the worker is acknowledged to shutdown.
                drop(self.value.take());
                if let Some(completion) = self.completion.take() {
                    completion.finish(true);
                }
                if let Some(completion) = self.request_completion.take() {
                    completion.finish(true);
                }
            }
        }
        let mut completed = tokio::task::spawn_blocking(move || Completed {
            value: Some(work()),
            _permit: permit,
            completion: Some(completion),
            request_completion,
        })
        .await?;
        completed.value.take().expect("worker result")
    }
    pub(crate) async fn drain(&self, deadline: tokio::time::Instant) -> Result<()> {
        self.owners.cancel_and_drain(deadline).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn owner_drain_waits_for_abandoned_blocking_work() {
        let pool = BlockingPool::new(1);
        let owners = CompletionRegistry::default();
        let (release, blocked) = std::sync::mpsc::channel();
        let (started, entered) = tokio::sync::oneshot::channel();
        let worker_pool = pool.clone();
        let request = owners.clone();
        let caller = tokio::spawn(async move {
            worker_pool
                .run_tracked(
                    move || {
                        let _ = started.send(());
                        let _ = blocked.recv();
                        Ok(())
                    },
                    Some(request),
                )
                .await
        });
        entered.await.unwrap();
        caller.abort();
        let _ = caller.await;
        assert!(
            owners
                .cancel_and_drain(
                    tokio::time::Instant::now() + std::time::Duration::from_millis(25)
                )
                .await
                .is_err(),
            "owner acknowledged a live worker"
        );
        release.send(()).unwrap();
        owners
            .cancel_and_drain(tokio::time::Instant::now() + std::time::Duration::from_secs(2))
            .await
            .unwrap();
        pool.drain(tokio::time::Instant::now() + std::time::Duration::from_secs(2))
            .await
            .unwrap();
    }
}
