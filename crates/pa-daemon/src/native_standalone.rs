//! Native substrate lifecycle for local print/JSON/RPC engines.
use pa_core::session_engine::engine::SessionEngine;
use serde_json::json;
use std::sync::Arc;

fn local_event_fact(event: &serde_json::Value) -> Option<serde_json::Value> {
    crate::native_telemetry::event_fact(&json!({"type":"session_event","event":event}))
}

pub struct Observer {
    queue: Arc<std::sync::Mutex<Option<crate::native_telemetry::Telemetry>>>,
    subscription: Option<pa_agent::agent::Subscription>,
}

impl Observer {
    /// Subscribe to local lifecycle events when native policy is enabled.
    ///
    /// # Panics
    /// The subscription callback panics if the telemetry queue mutex is poisoned.
    pub async fn attach(engine: &Arc<SessionEngine>) -> Self {
        let queue: Arc<std::sync::Mutex<Option<crate::native_telemetry::Telemetry>>> =
            Arc::new(std::sync::Mutex::new(None));
        let subscription = if pa_ai::native_policy::enabled() {
            let queue = Arc::clone(&queue);
            Some(
                engine
                    .session
                    .agent()
                    .subscribe(move |event, _| {
                        let queue = Arc::clone(&queue);
                        Box::pin(async move {
                            if let Some(frame) =
                                pa_core::session_engine::session_events::agent_event_json(&event)
                            {
                                if let Some(fact) = local_event_fact(&frame) {
                                    if let Some(queue) = queue.lock().unwrap().as_mut() {
                                        queue.event(&fact);
                                    }
                                }
                            }
                            Ok(())
                        })
                    })
                    .await,
            )
        } else {
            None
        };
        Self {
            queue,
            subscription,
        }
    }

    /// Called only at the trusted operator prompt boundary, before admission.
    ///
    /// # Errors
    /// Returns an error if the session is busy, policy re-arming fails, or the
    /// telemetry runtime is unavailable.
    ///
    /// # Panics
    /// Panics if the telemetry queue mutex is poisoned.
    pub async fn begin(
        &self,
        engine: &Arc<SessionEngine>,
        prompt: &str,
        image_count: usize,
    ) -> Result<(), String> {
        if !pa_ai::native_policy::enabled() {
            return Ok(());
        }
        let state = engine.session.agent().state().await;
        if state.is_streaming {
            return Err(
                "substrate operator prompt requires an idle session; use steer or follow_up".into(),
            );
        }
        let persistence = engine.session.shared_persistence();
        let manager = persistence.lock().await;
        if manager
            .get_header()
            .and_then(|header| header.rlm_depth)
            .unwrap_or(0)
            != 0
        {
            return Ok(());
        }
        pa_ai::native_policy::rearm_operator_prompt().await?;
        let binding = json!({
            "sessionId":manager.get_session_id(),"activeSessionId":manager.get_session_id(),
            "sessionFile":manager.get_session_file(),"ephemeral":manager.get_session_file().is_none(),"cwd":manager.get_cwd(),
            "provider":state.model.provider,"model":state.model.id,"thinkingLevel":state.thinking_level
        });
        let mut queue = self.queue.lock().unwrap();
        if queue.is_none() {
            *queue = crate::native_telemetry::Telemetry::new();
        }
        queue
            .as_mut()
            .ok_or("substrate telemetry runtime unavailable")?
            .begin(binding, prompt, image_count);
        Ok(())
    }

    /// Unsubscribe and drain the local telemetry queue.
    ///
    /// # Panics
    /// Panics if the telemetry queue mutex is poisoned.
    pub async fn close(&mut self) {
        if let Some(subscription) = self.subscription.take() {
            subscription.unsubscribe().await;
        }
        let queue = self.queue.lock().unwrap().take();
        if let Some(queue) = queue {
            queue.close().await;
        }
    }
}

/// A launch owns one immutable root tuple. Replacements require a new launch.
#[must_use]
pub fn authority_change(command: &str) -> bool {
    matches!(
        command,
        "new_session"
            | "switch_session"
            | "fork"
            | "set_model"
            | "cycle_model"
            | "set_thinking_level"
            | "cycle_thinking_level"
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn raw_local_events_preserve_terminal_usage_without_transcript() {
        let fact = super::local_event_fact(&serde_json::json!({"type":"agent_end","messages":[{"role":"assistant","stopReason":"stop","usage":{"input":3},"content":"private"}]})).unwrap();
        assert_eq!(fact["messages"][0]["usage"]["input"], 3);
        assert_eq!(fact["messages"][0]["stopReason"], "stop");
        assert!(fact["messages"][0].get("content").is_none());
        assert!(super::local_event_fact(&serde_json::json!({"type":"message_update"})).is_none());
    }
    #[test]
    fn immutable_launch_rejects_identity_changes_but_supports_operator_lifecycle() {
        for command in [
            "new_session",
            "switch_session",
            "fork",
            "set_model",
            "cycle_model",
            "set_thinking_level",
            "cycle_thinking_level",
        ] {
            assert!(super::authority_change(command));
        }
        for command in ["prompt", "get_state", "abort", "steer", "follow_up"] {
            assert!(!super::authority_change(command));
        }
    }
}
