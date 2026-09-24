//! Background jobs. Godot's main thread never computes terrain: work runs on a
//! worker thread and reports back through a channel that the owning Node polls
//! every frame, so signals are always emitted on the main thread.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};

/// Messages from a worker.
pub enum JobMsg<T> {
    Progress(f32),
    Done(Result<T, String>),
}

/// A running job. Dropping or cancelling it asks the worker to stop.
pub struct Job<T> {
    pub generation: i64,
    rx: Receiver<JobMsg<T>>,
    cancel: Arc<AtomicBool>,
}

impl<T: Send + 'static> Job<T> {
    /// Start `work` on a new thread. `work` gets a cancel flag and a progress sender.
    pub fn spawn<F>(generation: i64, work: F) -> Self
    where
        F: FnOnce(&AtomicBool, &(dyn Fn(f32) + Sync)) -> Result<T, String> + Send + 'static,
    {
        let (tx, rx) = channel::<JobMsg<T>>();
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        std::thread::Builder::new()
            .name(format!("terrain-job-{generation}"))
            .spawn(move || {
                let progress_tx: std::sync::Mutex<Sender<JobMsg<T>>> = std::sync::Mutex::new(tx.clone());
                let progress = move |f: f32| {
                    if let Ok(t) = progress_tx.lock() {
                        let _ = t.send(JobMsg::Progress(f));
                    }
                };
                let result =
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| work(&flag, &progress)))
                        .unwrap_or_else(|_| Err("internal error: the terrain engine panicked".into()));
                let _ = tx.send(JobMsg::Done(result));
            })
            .expect("failed to start worker thread");
        Self {
            generation,
            rx,
            cancel,
        }
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    /// Collect everything the worker sent since the last poll.
    /// Returns the latest progress (if any) and the final result (if finished).
    pub fn poll(&self) -> (Option<f32>, Option<Result<T, String>>) {
        let mut progress = None;
        loop {
            match self.rx.try_recv() {
                Ok(JobMsg::Progress(p)) => progress = Some(p),
                Ok(JobMsg::Done(r)) => return (progress, Some(r)),
                Err(TryRecvError::Empty) => return (progress, None),
                Err(TryRecvError::Disconnected) => {
                    return (progress, Some(Err("worker stopped unexpectedly".into())));
                }
            }
        }
    }
}

impl<T> Drop for Job<T> {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}
