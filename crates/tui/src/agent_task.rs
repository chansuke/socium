use crate::app::App;
use crate::transcript::Speaker;
use color_eyre::{Result, eyre::eyre};
use socium_core::{AgentProgress, AgentReply, AgentService};
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

pub(crate) struct SubmitOutcome {
    pub(crate) agent: AgentService,
    pub(crate) reply: Result<AgentReply, String>,
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

        tokio::spawn(async move {
            let reply = agent
                .submit_with_progress(user_input, |event| {
                    let _ = progress_tx.send(event);
                })
                .await
                .map_err(|err| err.to_string());
            let _ = result_tx.send(SubmitOutcome {
                agent,
                reply,
                latency_ms: started_at.elapsed().as_millis(),
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
            Ok(reply) => {
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
            Err(err) => {
                self.push_message(Speaker::System, format!("Request failed: {err}"));
                self.push_activity(format!("Request failed: {err}"));
                self.status = Some("Failed".to_string());
            }
        }
    }
}
