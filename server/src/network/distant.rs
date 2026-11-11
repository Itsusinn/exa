use crate::voxel::{distant, VoxelWorld};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tokio::{sync::mpsc, task::JoinHandle};

struct Job {
    handle: JoinHandle<()>,
    receiver: mpsc::Receiver<Vec<distant::Output>>,
    cancelled: Arc<AtomicBool>,
}

impl Drop for Job {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Relaxed);
        self.handle.abort();
        // Already running blocking work owns only a <=4-column snapshot. It
        // checks cancellation between columns and cannot touch the world.
    }
}

#[derive(Default)]
pub(super) struct Service {
    job: Option<Job>,
}

impl Service {
    /// Only try_recv here: never await a preview worker on the authority path.
    pub(super) fn poll(&mut self, world: &mut VoxelWorld) {
        if let Some(job) = &mut self.job {
            match job.receiver.try_recv() {
                Ok(outputs) => {
                    world.apply_distant(outputs);
                    self.job = None;
                }
                Err(mpsc::error::TryRecvError::Disconnected) => {
                    tracing::warn!("distant worker exited without a result");
                    self.job = None;
                }
                Err(mpsc::error::TryRecvError::Empty) => {}
            }
        }
        world.stream_distant();
        if self.job.is_some() {
            return;
        }
        let inputs = world.distant_input();
        if inputs.is_empty() {
            return;
        }
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancelled = Arc::clone(&cancelled);
        let (sender, receiver) = mpsc::channel(1);
        let handle = tokio::task::spawn_blocking(move || {
            let outputs = distant::build_batch(inputs, &worker_cancelled);
            let _ = sender.try_send(outputs);
        });
        self.job = Some(Job {
            handle,
            receiver,
            cancelled,
        });
    }
}
