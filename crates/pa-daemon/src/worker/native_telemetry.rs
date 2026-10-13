//! Bounded, ordered substrate telemetry; never blocks worker event delivery.
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

pub(super) struct Telemetry {
    sender: tokio::sync::mpsc::Sender<(&'static str, Value)>,
    failed: Arc<AtomicBool>,
    generation: String,
    active_session_id: String,
    sequence: u64,
}

impl Telemetry {
    pub(super) fn new() -> Option<Self> {
        if !pa_ai::native_policy::enabled() {
            return None;
        }
        let runtime = tokio::runtime::Handle::try_current().ok()?;
        let (sender, mut receiver) = tokio::sync::mpsc::channel::<(&'static str, Value)>(256);
        let failed = Arc::new(AtomicBool::new(false));
        let failure = Arc::clone(&failed);
        runtime.spawn(async move {
            while let Some((op, body)) = receiver.recv().await {
                if failure.load(Ordering::SeqCst) {
                    let _ = pa_ai::native_policy::request_telemetry(
                        "telemetry_close",
                        json!({"completed":false,"reason":"native_queue_gap"}),
                    )
                    .await;
                    return;
                }
                if pa_ai::native_policy::request_telemetry(op, body)
                    .await
                    .is_err()
                {
                    eprintln!("substrate telemetry transport failed; turn observation interrupted");
                    failure.store(true, Ordering::SeqCst);
                    let _ = pa_ai::native_policy::request_telemetry(
                        "telemetry_close",
                        json!({"completed":false,"reason":"native_transport_error"}),
                    )
                    .await;
                    return;
                }
            }
            let _ = pa_ai::native_policy::request_telemetry(
                "telemetry_close",
                json!({"completed":false,"reason":"native_worker_closed"}),
            )
            .await;
        });
        Some(Self {
            sender,
            failed,
            generation: String::new(),
            active_session_id: String::new(),
            sequence: 0,
        })
    }

    pub(super) fn begin(&mut self, mut binding: Value, prompt: &str, image_count: usize) {
        self.generation = uuid::Uuid::new_v4().to_string();
        self.sequence = 0;
        self.active_session_id = binding["activeSessionId"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        binding["cursor"] = json!({"generation":self.generation,"sequence":0});
        self.send(
            "telemetry_begin",
            json!({"binding":binding,"prompt":prompt,"images":vec![json!({});image_count]}),
        );
    }

    pub(super) fn event(&mut self, event: Value) {
        self.sequence += 1;
        self.send("telemetry_event", json!({"frame":{"type":"session_event","activeSessionId":self.active_session_id,"event":event,"meta":{"cursor":{"generation":self.generation,"sequence":self.sequence}}}}));
    }

    pub(super) fn send(&self, op: &'static str, body: Value) {
        if !self.failed.load(Ordering::SeqCst) && self.sender.try_send((op, body)).is_err() {
            eprintln!("substrate telemetry queue overflow; turn observation interrupted");
            self.failed.store(true, Ordering::SeqCst);
        }
    }
}

/// Only lifecycle facts are copied; streamed prose and tool arguments stay private.
pub(super) fn event_fact(frame: &Value) -> Option<Value> {
    if frame.get("type").and_then(Value::as_str) != Some("session_event") {
        return None;
    }
    let event = frame.get("event")?;
    match event.get("type")?.as_str()? {
        "agent_start" => Some(json!({"type":"agent_start"})),
        "agent_end" => Some(
            json!({"type":"agent_end","messages":event.get("messages").and_then(Value::as_array).map(|messages| messages.iter().map(|message| json!({"role":message.get("role"),"usage":message.get("usage"),"stopReason":message.get("stopReason")})).collect::<Vec<_>>())}),
        ),
        "tool_execution_end" => Some(
            json!({"type":"tool_execution_end","toolCallId":event.get("toolCallId"),"result":{"isError":event.get("isError").or_else(||event.get("result").and_then(|r|r.get("isError"))).and_then(Value::as_bool).unwrap_or(false)}}),
        ),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retains_tool_error_without_content_and_ignores_streaming_text() {
        assert_eq!(
            event_fact(
                &json!({"type":"session_event","event":{"type":"tool_execution_end","toolCallId":"t","isError":true,"result":{"content":"secret"}}})
            ),
            Some(json!({"type":"tool_execution_end","toolCallId":"t","result":{"isError":true}}))
        );
        assert_eq!(
            event_fact(
                &json!({"type":"session_event","event":{"type":"message_update","message":"secret"}})
            ),
            None
        );
        assert_eq!(
            event_fact(
                &json!({"type":"side_question_event","event":{"type":"agent_end","messages":[]}})
            ),
            None
        );
    }
    #[test]
    fn usage_survives_but_transcript_content_does_not() {
        assert_eq!(
            event_fact(
                &json!({"type":"session_event","event":{"type":"agent_end","messages":[{"role":"assistant","content":"secret","stopReason":"stop","usage":{"input":3}}]}})
            ),
            Some(
                json!({"type":"agent_end","messages":[{"role":"assistant","stopReason":"stop","usage":{"input":3}}]})
            )
        );
    }

    #[test]
    fn queue_preserves_dedicated_sequence_and_detects_overflow() {
        let (sender, mut receiver) = tokio::sync::mpsc::channel(2);
        let mut telemetry = Telemetry {
            sender,
            failed: Arc::new(AtomicBool::new(false)),
            generation: String::new(),
            active_session_id: String::new(),
            sequence: 0,
        };
        telemetry.begin(json!({"activeSessionId":"root"}), "prompt", 0);
        telemetry.event(json!({"type":"agent_start"}));
        let (op, begin) = receiver.try_recv().unwrap();
        assert_eq!(op, "telemetry_begin");
        let (op, event) = receiver.try_recv().unwrap();
        assert_eq!(op, "telemetry_event");
        assert_eq!(
            event["frame"]["meta"]["cursor"],
            json!({"generation":begin["binding"]["cursor"]["generation"],"sequence":1})
        );
        telemetry.event(json!({"type":"agent_start"}));
        telemetry.event(json!({"type":"agent_start"}));
        telemetry.event(json!({"type":"agent_start"}));
        assert!(telemetry.failed.load(Ordering::SeqCst));
    }
}
