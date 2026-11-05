//! A separate, single-slot design worker. Cancellation never releases the slot
//! until the blocking calculation has actually returned.
use exa_protocol::{
    DistillationCommand, DistillationReply, DistillationResult, DistillationStatus, ServerMessage,
};
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};
use tokio::{sync::mpsc, task::JoinHandle};

struct Session {
    // The world owns the strong sender. A design session must not keep a player
    // stream alive after authority-side removal (for example a slow consumer).
    tx: mpsc::WeakSender<ServerMessage>,
    connection: quinn::Connection,
    latest: Option<DistillationReply>,
}
struct Job {
    player: u32,
    request: u64,
    cancelled: Arc<AtomicBool>,
    worker: JoinHandle<Result<DistillationResult, String>>,
}
#[derive(Default)]
pub(super) struct DesignService {
    sessions: HashMap<u32, Session>,
    job: Option<Job>,
}
impl DesignService {
    pub(super) fn register(
        &mut self,
        player: u32,
        tx: mpsc::Sender<ServerMessage>,
        connection: quinn::Connection,
    ) {
        self.sessions.insert(
            player,
            Session {
                tx: tx.downgrade(),
                connection,
                latest: None,
            },
        );
    }
    pub(super) fn unregister(&mut self, player: u32) {
        self.sessions.remove(&player);
        if let Some(job) = &self.job {
            if job.player == player {
                job.cancelled.store(true, Ordering::Relaxed);
            }
        }
    }
    fn send(&self, player: u32, reply: DistillationReply) {
        if let Some(session) = self.sessions.get(&player) {
            if session
                .tx
                .upgrade()
                .is_none_or(|tx| tx.try_send(ServerMessage::Distillation(reply)).is_err())
            {
                session
                    .connection
                    .close(0u32.into(), b"design reply queue full");
            }
        }
    }
    fn publish(&mut self, player: u32, reply: DistillationReply) {
        if let Some(session) = self.sessions.get_mut(&player) {
            session.latest = Some(reply.clone());
        }
        self.send(player, reply);
    }
    pub(super) fn command(&mut self, player: u32, command: DistillationCommand) {
        let request = match &command {
            DistillationCommand::Calculate { request_id, .. }
            | DistillationCommand::Cancel { request_id } => *request_id,
        };
        if request == 0 || request > i64::MAX as u64 {
            return;
        }
        let Some(session) = self.sessions.get(&player) else {
            return;
        };
        let previous = session.latest.clone();
        match command {
            DistillationCommand::Cancel { .. } => {
                if let Some(last) = previous.filter(|last| last.request_id == request) {
                    if last.status != DistillationStatus::Running {
                        self.send(player, last);
                        return;
                    }
                    if let Some(job) = &self.job {
                        if job.player == player && job.request == request {
                            job.cancelled.store(true, Ordering::Relaxed);
                        }
                    }
                    self.publish(
                        player,
                        reply(request, DistillationStatus::Cancelled, "计算已取消。"),
                    );
                } else {
                    self.send(
                        player,
                        reply(
                            request,
                            DistillationStatus::Rejected,
                            "取消请求已过期或不存在。",
                        ),
                    );
                }
            }
            DistillationCommand::Calculate { spec, .. } => {
                if let Some(last) = previous {
                    if request == last.request_id {
                        self.send(player, last);
                        return;
                    }
                    if request < last.request_id {
                        self.send(
                            player,
                            reply(request, DistillationStatus::Rejected, "设计请求已过期。"),
                        );
                        return;
                    }
                }
                // A newer request supersedes the previous one, but its worker
                // still occupies the only slot until cooperative cancellation ends.
                if let Some(job) = &self.job {
                    if job.player == player {
                        job.cancelled.store(true, Ordering::Relaxed);
                    }
                }
                if !spec.valid() {
                    self.publish(
                        player,
                        reply(
                            request,
                            DistillationStatus::Rejected,
                            "参数超出二元蒸馏设计器范围。",
                        ),
                    );
                    return;
                }
                if self.job.is_some() {
                    self.publish(
                        player,
                        reply(
                            request,
                            DistillationStatus::Rejected,
                            "设计计算器忙，请稍后重新计算。",
                        ),
                    );
                    return;
                }
                let cancelled = Arc::new(AtomicBool::new(false));
                let cancel = cancelled.clone();
                let worker = tokio::task::spawn_blocking(move || {
                    std::panic::catch_unwind(|| crate::distillation::solve(&spec, &cancel))
                        .unwrap_or_else(|_| Err("设计计算异常，未发布任何结果。".into()))
                });
                self.job = Some(Job {
                    player,
                    request,
                    cancelled,
                    worker,
                });
                self.publish(
                    player,
                    reply(
                        request,
                        DistillationStatus::Running,
                        "正在计算稳态二元蒸馏设计…",
                    ),
                );
            }
        }
    }
    /// Called on the server tick, but only awaits an already-finished handle.
    pub(super) async fn poll(&mut self) {
        if !self
            .job
            .as_ref()
            .is_some_and(|job| job.worker.is_finished())
        {
            return;
        }
        let job = self.job.take().unwrap();
        let result = job
            .worker
            .await
            .unwrap_or_else(|_| Err("设计后台任务失败。".into()));
        let Some(latest) = self
            .sessions
            .get(&job.player)
            .and_then(|session| session.latest.as_ref())
        else {
            return;
        };
        if latest.request_id != job.request || latest.status != DistillationStatus::Running {
            return;
        }
        let answer = if job.cancelled.load(Ordering::Relaxed) {
            reply(job.request, DistillationStatus::Cancelled, "计算已取消。")
        } else {
            match result {
                Ok(result) if result.valid() => DistillationReply {
                    request_id: job.request,
                    status: DistillationStatus::Completed,
                    message: "计算完成，组分、能量与相平衡校验通过。".into(),
                    result: Some(result),
                },
                Ok(_) => reply(
                    job.request,
                    DistillationStatus::Failed,
                    "计算结果超出允许范围，未发布。",
                ),
                Err(error) => reply(job.request, DistillationStatus::Failed, &error),
            }
        };
        self.publish(job.player, answer);
    }
    pub(super) async fn shutdown(&mut self) {
        self.sessions.clear();
        if let Some(job) = self.job.take() {
            job.cancelled.store(true, Ordering::Relaxed);
            let _ = job.worker.await;
        }
    }
}
fn reply(request_id: u64, status: DistillationStatus, message: &str) -> DistillationReply {
    let mut message = message.to_owned();
    while message.len() > 512 {
        message.pop();
    }
    DistillationReply {
        request_id,
        status,
        message,
        result: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn cancelled_worker_keeps_its_slot_until_return_and_shutdown_joins_it() {
        let gate = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
        let wait_gate = gate.clone();
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker = tokio::task::spawn_blocking(move || {
            let (lock, changed) = &*wait_gate;
            let guard = lock.lock().unwrap();
            let _guard = changed.wait_while(guard, |released| !*released).unwrap();
            Err("test worker finished".to_owned())
        });
        let mut service = DesignService {
            sessions: HashMap::new(),
            job: Some(Job {
                player: 9,
                request: 1,
                cancelled: cancelled.clone(),
                worker,
            }),
        };
        service.unregister(9);
        assert!(cancelled.load(Ordering::Relaxed));
        service.poll().await;
        assert!(
            service.job.is_some(),
            "cancel cannot free a still-running blocking slot"
        );
        *gate.0.lock().unwrap() = true;
        gate.1.notify_all();
        tokio::time::timeout(std::time::Duration::from_secs(2), service.shutdown())
            .await
            .unwrap();
        assert!(service.job.is_none());
    }
}
