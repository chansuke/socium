use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use std::time::{Duration, Instant};

#[derive(Default)]
struct ControlState {
    cancelled: AtomicBool,
    interactive: AtomicBool,
    next_id: AtomicU64,
    approval: Mutex<Option<(u64, Option<bool>)>>,
}

/// Per-task cancellation and one-shot human approval. Model arguments never
/// grant authority. Headless callers deny commands unless they install a UI.
#[derive(Clone, Default)]
pub struct TaskControl(Arc<ControlState>);
impl TaskControl {
    pub fn cancel(&self) {
        self.0.cancelled.store(true, Ordering::Release);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.cancelled.load(Ordering::Acquire)
    }
    pub fn enable_interactive_approval(&self) {
        self.0.interactive.store(true, Ordering::Release);
    }
    pub fn answer_approval(&self, id: u64, allow: bool) -> bool {
        let mut pending = self.0.approval.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((current, answer)) = pending.as_mut()
            && *current == id
            && answer.is_none()
        {
            *answer = Some(allow);
            return true;
        }
        false
    }
    pub(crate) fn approve(
        &self,
        tool: &str,
        input: &serde_json::Value,
        progress: &mut dyn FnMut(crate::AgentProgress),
    ) -> bool {
        self.approve_with_timeout(tool, input, progress, Duration::from_secs(120))
    }
    fn approve_with_timeout(
        &self,
        tool: &str,
        input: &serde_json::Value,
        progress: &mut dyn FnMut(crate::AgentProgress),
        timeout: Duration,
    ) -> bool {
        if !self.0.interactive.load(Ordering::Acquire) || self.is_cancelled() {
            return false;
        }
        let id = self.0.next_id.fetch_add(1, Ordering::Relaxed);
        *self.0.approval.lock().unwrap_or_else(|e| e.into_inner()) = Some((id, None));
        progress(crate::AgentProgress::ApprovalRequested {
            request_id: id,
            tool: tool.into(),
            input: format!(
                "Workspace: {}\nTool call: {}\nPermissions: {}\nArguments:\n{}\n{}",
                input["workspace"],
                input["tool_call_id"],
                input["permissions"]
                    .as_str()
                    .unwrap_or("No execution authority"),
                serde_json::to_string_pretty(&input["arguments"]).unwrap_or_default(),
                input["review"].as_str().unwrap_or("")
            ),
        });
        let start = Instant::now();
        loop {
            let mut pending = self.0.approval.lock().unwrap_or_else(|e| e.into_inner());
            let answer = pending.as_ref().and_then(|(_, a)| *a);
            if self.is_cancelled() || start.elapsed() >= timeout || answer.is_some() {
                *pending = None;
                drop(pending);
                let approved =
                    !self.is_cancelled() && start.elapsed() < timeout && answer == Some(true);
                progress(crate::AgentProgress::ApprovalResolved {
                    request_id: id,
                    approved,
                });
                return approved;
            }
            drop(pending);
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    pub fn reset_cancellation(&self) {
        self.0.cancelled.store(false, Ordering::Release);
        *self.0.approval.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
    pub(crate) async fn cancelled(&self) {
        while !self.is_cancelled() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn expired_grant_is_not_usable_even_when_answer_arrives() {
        let c = TaskControl::default();
        c.enable_interactive_approval();
        assert!(!c.approve_with_timeout(
            "run_command",
            &serde_json::json!({}),
            &mut |e| {
                if let crate::AgentProgress::ApprovalRequested { request_id, .. } = e {
                    c.answer_approval(request_id, true);
                }
            },
            Duration::ZERO
        ));
        assert!(!c.answer_approval(0, true));
    }
}
