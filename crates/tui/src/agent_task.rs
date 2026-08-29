use crate::app::App;
use crate::transcript::Speaker;
use color_eyre::{Result, eyre::eyre};
use socium_core::{AgentError, AgentProgress, AgentReply, AgentService, CompactionReport};
use std::time::Instant;
use tokio::sync::{
    mpsc::{UnboundedReceiver, unbounded_channel},
    oneshot,
};

pub(crate) struct PendingSubmission {
    pub(crate) user_input: String,
    pub(crate) started_at: Instant,
}

pub(crate) struct PendingTask {
    pub(crate) progress_rx: UnboundedReceiver<AgentProgress>,
    pub(crate) result_rx: oneshot::Receiver<SubmitOutcome>,
    pub(crate) streamed_assistant_text: bool,
}

pub(crate) enum TaskReply {
    Submission(Result<AgentReply, String>),
    Compaction(Result<CompactionReport, AgentError>),
}

pub(crate) struct SubmitOutcome {
    pub(crate) agent: AgentService,
    pub(crate) reply: TaskReply,
    pub(crate) latency_ms: u128,
}

impl App {
    pub(crate) fn start_submit_task(&mut self, pending: PendingSubmission) -> Result<()> {
        let Some(mut agent) = self.agent.take() else {
            return Err(eyre!("agent is unavailable while a request is running"));
        };
        self.context_metrics = agent
            .context_metrics(&pending.user_input)
            .unwrap_or_default();
        let (progress_tx, progress_rx) = unbounded_channel();
        let (result_tx, result_rx) = oneshot::channel();
        let PendingSubmission {
            user_input,
            started_at,
        } = pending;

        // Run the submission on a dedicated OS thread with its own
        // current-thread runtime: the tool runtime is synchronous and
        // blocking (std::process, std::fs), so it must not occupy a worker
        // of the UI's tokio runtime.
        std::thread::spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(err) => {
                    let _ = result_tx.send(SubmitOutcome {
                        agent,
                        reply: TaskReply::Submission(Err(err.to_string())),
                        latency_ms: started_at.elapsed().as_millis(),
                    });
                    return;
                }
            };
            runtime.block_on(async move {
                let reply = agent
                    .submit_with_progress(user_input, |event| {
                        let _ = progress_tx.send(event);
                    })
                    .await
                    .map_err(|err| err.to_string());
                let _ = result_tx.send(SubmitOutcome {
                    agent,
                    reply: TaskReply::Submission(reply),
                    latency_ms: started_at.elapsed().as_millis(),
                });
            });
        });

        self.pending_task = Some(PendingTask {
            progress_rx,
            result_rx,
            streamed_assistant_text: false,
        });
        Ok(())
    }

    /// Run a forced compaction on the agent thread, mirroring the
    /// submission flow: the agent moves to a dedicated thread and comes
    /// back through the oneshot with the report.
    pub(crate) fn start_compact_task(&mut self) -> Result<()> {
        let Some(mut agent) = self.agent.take() else {
            return Err(eyre!("agent is unavailable while a request is running"));
        };
        let started_at = Instant::now();
        let (_progress_tx, progress_rx) = unbounded_channel();
        let (result_tx, result_rx) = oneshot::channel();

        std::thread::spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(err) => {
                    let _ = result_tx.send(SubmitOutcome {
                        agent,
                        reply: TaskReply::Compaction(Err(AgentError::State(
                            std::io::Error::other(err.to_string()),
                        ))),
                        latency_ms: started_at.elapsed().as_millis(),
                    });
                    return;
                }
            };
            runtime.block_on(async move {
                let report = agent.compact().await;
                let _ = result_tx.send(SubmitOutcome {
                    agent,
                    reply: TaskReply::Compaction(report),
                    latency_ms: started_at.elapsed().as_millis(),
                });
            });
        });

        self.pending_task = Some(PendingTask {
            progress_rx,
            result_rx,
            streamed_assistant_text: false,
        });
        Ok(())
    }

    pub(crate) fn poll_pending_task(&mut self) {
        let mut progress_events = Vec::new();
        let mut streamed_assistant_text = None;
        let mut completed = None;

        if let Some(task) = self.pending_task.as_mut() {
            while let Ok(event) = task.progress_rx.try_recv() {
                if matches!(event, AgentProgress::AssistantText { .. }) {
                    task.streamed_assistant_text = true;
                }
                progress_events.push(event);
            }
            streamed_assistant_text = Some(task.streamed_assistant_text);
            if let Ok(outcome) = task.result_rx.try_recv() {
                completed = Some(outcome);
            }
        }

        for event in progress_events {
            self.apply_progress(event);
        }

        let Some(outcome) = completed else {
            return;
        };
        let streamed_assistant_text = streamed_assistant_text.unwrap_or(false);
        self.pending_task = None;
        self.agent = Some(outcome.agent);
        self.refresh_context_metrics();
        self.last_latency_ms = Some(outcome.latency_ms);
        self.pop_thinking_message();

        match outcome.reply {
            TaskReply::Submission(Ok(reply)) => {
                if !streamed_assistant_text {
                    if reply.text.trim().is_empty() {
                        self.push_message(Speaker::Socium, "(no text response)");
                    } else {
                        self.push_message(Speaker::Socium, reply.text);
                    }
                }
                self.push_activity("Request completed");
                self.status = Some("Done".to_string());
            }
            TaskReply::Submission(Err(err)) => {
                self.push_message(Speaker::System, format!("Request failed: {err}"));
                self.push_activity(format!("Request failed: {err}"));
                self.status = Some("Failed".to_string());
            }
            TaskReply::Compaction(Ok(report)) => {
                let summarizer = if report.used_llm {
                    "model summary"
                } else {
                    "deterministic fallback"
                };
                let message = format!(
                    "Context compacted: {} messages summarized, {} retained ({summarizer}), {}% context left",
                    report.covered_messages,
                    report.retained_messages,
                    report.metrics.context_left_percentage,
                );
                self.push_message(Speaker::System, message.clone());
                self.push_activity(message);
                self.status = Some("Compacted".to_string());
            }
            TaskReply::Compaction(Err(AgentError::NothingToCompact)) => {
                self.push_message(
                    Speaker::System,
                    "Nothing to compact: conversation already fits in the retained window."
                        .to_string(),
                );
                self.status = Some("Nothing to compact".to_string());
            }
            TaskReply::Compaction(Err(err)) => {
                self.push_message(Speaker::System, format!("Compaction failed: {err}"));
                self.push_activity(format!("Compaction failed: {err}"));
                self.status = Some("Compaction failed".to_string());
            }
        }
    }
}
