use crate::approval::{Grant, grants_cover};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use std::time::{Duration, Instant};

/// The user's answer to one approval request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalAnswer {
    AllowOnce,
    /// Allow, and allow what the request offered until Socium exits.
    AllowForSession,
    Deny,
}

#[derive(Default)]
struct ControlState {
    cancelled: AtomicBool,
    interactive: AtomicBool,
    next_id: AtomicU64,
    approval: Mutex<Option<(u64, Option<ApprovalAnswer>)>>,
    /// What the user allowed for the session; kept across turns.
    grants: Mutex<Vec<Grant>>,
}

/// Per-task cancellation and human approval. Model arguments never grant
/// authority. Headless callers deny what needs approval unless they install
/// a UI.
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
    pub(crate) fn is_interactive(&self) -> bool {
        self.0.interactive.load(Ordering::Acquire)
    }
    /// Allow once (`true`) or deny (`false`).
    pub fn answer_approval(&self, id: u64, allow: bool) -> bool {
        self.answer(
            id,
            if allow {
                ApprovalAnswer::AllowOnce
            } else {
                ApprovalAnswer::Deny
            },
        )
    }
    pub fn answer(&self, id: u64, answer: ApprovalAnswer) -> bool {
        let mut pending = self.0.approval.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((current, slot)) = pending.as_mut()
            && *current == id
            && slot.is_none()
        {
            *slot = Some(answer);
            return true;
        }
        false
    }
    /// Whether the user allowed `wanted` for this session.
    pub(crate) fn is_granted(&self, wanted: &Grant) -> bool {
        grants_cover(
            &self.0.grants.lock().unwrap_or_else(|e| e.into_inner()),
            wanted,
        )
    }
    /// Asks the user and waits for the answer or a cancellation; there is
    /// no deadline, so a user who stepped away is not taken as a refusal.
    pub(crate) fn approve(
        &self,
        tool: &str,
        input: &serde_json::Value,
        grant: Option<Grant>,
        progress: &mut dyn FnMut(crate::AgentProgress),
    ) -> bool {
        self.approve_with_timeout(tool, input, grant, progress, None)
    }
    fn approve_with_timeout(
        &self,
        tool: &str,
        input: &serde_json::Value,
        grant: Option<Grant>,
        progress: &mut dyn FnMut(crate::AgentProgress),
        timeout: Option<Duration>,
    ) -> bool {
        if !self.is_interactive() || self.is_cancelled() {
            return false;
        }
        let id = self.0.next_id.fetch_add(1, Ordering::Relaxed);
        *self.0.approval.lock().unwrap_or_else(|e| e.into_inner()) = Some((id, None));
        progress(crate::AgentProgress::ApprovalRequested {
            request_id: id,
            tool: tool.into(),
            summary: crate::activity::summarize_tool_input(tool, &input["arguments"]),
            input: format!(
                "Why: {}\nWorkspace: {}\nTool call: {}\nPermissions: {}\nArguments:\n{}\n{}",
                input["reason"]
                    .as_str()
                    .unwrap_or("This call needs your approval."),
                input["workspace"],
                input["tool_call_id"],
                input["permissions"]
                    .as_str()
                    .unwrap_or("No execution authority"),
                serde_json::to_string_pretty(&input["arguments"]).unwrap_or_default(),
                input["review"].as_str().unwrap_or("")
            ),
            requested_by: None,
            session_option: grant.as_ref().map(Grant::label),
        });
        let start = Instant::now();
        loop {
            let mut pending = self.0.approval.lock().unwrap_or_else(|e| e.into_inner());
            let answer = pending.as_ref().and_then(|(_, a)| *a);
            let expired = timeout.is_some_and(|limit| start.elapsed() >= limit);
            if self.is_cancelled() || expired || answer.is_some() {
                *pending = None;
                drop(pending);
                let approved = !self.is_cancelled()
                    && !expired
                    && matches!(
                        answer,
                        Some(ApprovalAnswer::AllowOnce | ApprovalAnswer::AllowForSession)
                    );
                if approved
                    && answer == Some(ApprovalAnswer::AllowForSession)
                    && let Some(grant) = grant
                {
                    self.0
                        .grants
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .push(grant);
                }
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
            None,
            &mut |e| {
                if let crate::AgentProgress::ApprovalRequested { request_id, .. } = e {
                    c.answer_approval(request_id, true);
                }
            },
            Some(Duration::ZERO)
        ));
        assert!(!c.answer_approval(0, true));
    }
    #[test]
    fn approval_requests_carry_a_one_line_summary_of_the_arguments() {
        let c = TaskControl::default();
        c.enable_interactive_approval();
        let mut summary = None;
        c.approve_with_timeout(
            "run_command",
            &serde_json::json!({"arguments": {"command": ["cargo", "test"]}}),
            None,
            &mut |e| {
                if let crate::AgentProgress::ApprovalRequested { summary: s, .. } = e {
                    summary = Some(s);
                }
            },
            Some(Duration::ZERO),
        );
        assert_eq!(summary.as_deref(), Some("cargo test"));
    }
    #[test]
    fn a_session_answer_grants_what_was_offered_and_nothing_else() {
        let c = TaskControl::default();
        c.enable_interactive_approval();
        let offered = Grant::Command(vec!["cargo".into(), "test".into()]);
        let mut label = None;
        assert!(c.approve(
            "run_command",
            &serde_json::json!({}),
            Some(offered.clone()),
            &mut |e| {
                if let crate::AgentProgress::ApprovalRequested {
                    request_id,
                    session_option,
                    ..
                } = e
                {
                    label = session_option;
                    c.answer(request_id, ApprovalAnswer::AllowForSession);
                }
            },
        ));
        assert!(label.unwrap().contains("cargo test"));
        assert!(c.is_granted(&Grant::Command(vec![
            "cargo".into(),
            "test".into(),
            "-q".into()
        ])));
        assert!(!c.is_granted(&Grant::Command(vec!["cargo".into(), "build".into()])));
    }
    #[test]
    fn a_pending_request_waits_past_the_old_deadline_until_answered() {
        let c = TaskControl::default();
        c.enable_interactive_approval();
        let answering = c.clone();
        let started = Instant::now();
        let approved = c.approve("run_command", &serde_json::json!({}), None, &mut |e| {
            if let crate::AgentProgress::ApprovalRequested { request_id, .. } = e {
                let answering = answering.clone();
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_millis(150));
                    answering.answer(request_id, ApprovalAnswer::Deny);
                });
            }
        });
        assert!(!approved);
        assert!(started.elapsed() >= Duration::from_millis(150));
    }
}
