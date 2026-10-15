//! Initial spawn admission gates own-child follow-ups before either transport.
use super::*;
use crate::agent_messaging::LinkAgentMessageController;
use crate::protocol::{response_failure, response_success};
use pa_core::session_engine::agent_messaging::{AgentMessageController, AgentMessageSendInput};
use pa_types::platform::transport::bind_transport;
use std::future::Future;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, oneshot};

struct Request {
    command: Value,
    answer: oneshot::Sender<bool>,
}

async fn fixture() -> (
    tempfile::TempDir,
    Arc<SupervisorChildSessions>,
    LinkAgentMessageController,
    mpsc::UnboundedReceiver<Request>,
) {
    let dir = tempfile::TempDir::new().unwrap();
    let socket = dir.path().join("supervisor.sock");
    let listener = bind_transport(&socket).await.unwrap();
    let (tx, rx) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        loop {
            let Ok(stream) = listener.accept().await else {
                return;
            };
            let tx = tx.clone();
            tokio::spawn(async move {
                let (reader, mut writer) = stream.split();
                writer.write_all(b"{\"type\":\"daemon_hello\",\"protocol\":{\"name\":\"prime-agent.daemon\",\"version\":7}}\n").await.unwrap();
                let mut reader = BufReader::new(reader);
                let mut line = String::new();
                if reader.read_line(&mut line).await.unwrap() == 0 {
                    return;
                }
                let envelope: Value = serde_json::from_str(&line).unwrap();
                let command = envelope["command"].clone();
                let kind = command["type"].as_str().unwrap();
                let id = envelope["id"].as_str().unwrap();
                let (answer, wait) = oneshot::channel();
                tx.send(Request {
                    command: command.clone(),
                    answer,
                })
                .unwrap();
                let Ok(success) = wait.await else {
                    return;
                };
                let data = match kind {
                    "create" => {
                        json!({"activeSessionId":"child-live", "sessionId":"child-file", "sessionName":"kid"})
                    }
                    "list_agent_peers" => {
                        json!({"peers":[{"activeSessionId":"replacement", "sessionId":"child-file", "sessionName":"kid"}]})
                    }
                    "send_message" => {
                        json!({"id":"receipt", "target":{"activeSessionId":"child-live"}, "deliveryStatus":"queued"})
                    }
                    _ => Value::Null,
                };
                let response = if success {
                    response_success(Some(id), kind, Some(data))
                } else {
                    response_failure(Some(id), kind, "fake refused", None)
                };
                let mut line = serde_json::to_string(&response).unwrap();
                line.push('\n');
                let _ = writer.write_all(line.as_bytes()).await;
            });
        }
    });
    let link = Arc::new(crate::supervisor_link::SupervisorLink::new(socket));
    let sessions = Arc::new(SupervisorChildSessions::new(
        Arc::clone(&link),
        dir.path().to_owned(),
        "parent".to_string(),
        Arc::new(crate::model_allowlist::ModelRefusalTelemetry::new(
            dir.path().to_owned(),
            /*telemetry_disabled*/ true,
        )),
    ));
    sessions.set_identity(ParentIdentity {
        model: Some("mock/mock-1".to_string()),
        cwd: Some(dir.path().to_string_lossy().into_owned()),
        ..ParentIdentity::with_default_depth()
    });
    let controller = LinkAgentMessageController::new(
        link,
        "parent".to_string(),
        "token".to_string(),
        Arc::new(std::sync::Mutex::new(None)),
        Some(Arc::clone(&sessions)),
    );
    (dir, sessions, controller, rx)
}

async fn registered(
    sessions: &Arc<SupervisorChildSessions>,
    rx: &mut mpsc::UnboundedReceiver<Request>,
) -> RlmSpawnHandle {
    let host = Arc::clone(sessions);
    let spawn = tokio::spawn(async move {
        host.spawn(RlmSpawnRequest {
            route: None,
            prompt: "initial task".to_string(),
            name: Some("kid".to_string()),
            model: None,
            thinking: None,
            cell_source_code: None,
            spawned_by_request_id: None,
        })
        .await
        .unwrap()
    });
    let create = rx.recv().await.unwrap();
    assert_eq!(create.command["type"], "create");
    create.answer.send(true).unwrap();
    spawn.await.unwrap()
}

fn followup(target: &str) -> AgentMessageSendInput {
    AgentMessageSendInput {
        target: target.to_string(),
        message: "follow up".to_string(),
        receiver_role: None,
    }
}

#[tokio::test]
async fn prompt_admission_orders_spawn_and_concurrent_replacement_followups() {
    tokio::time::timeout(Duration::from_secs(10), async {
        for replacement in [false, true] {
            let (_dir, sessions, controller, mut rx) = fixture().await;
            let handle = registered(&sessions, &mut rx).await;
            let target = if replacement {
                let family = controller.family();
                tokio::pin!(family);
                let row = tokio::select! { row = rx.recv() => row.unwrap(), result = &mut family => panic!("family returned early: {result:?}") };
                assert_eq!(row.command["type"], "list_agent_peers");
                row.answer.send(true).unwrap();
                family.await.unwrap().into_iter().find(|row| row.id == "replacement").unwrap().id
            } else { handle.rlm_child_id.clone() };
            let first = controller.send_agent_message(followup(&target));
            let second = controller.send_agent_message(followup("child-file"));
            tokio::pin!(first, second);
            // Poll each caller to its first suspension. The retained admission
            // receivers prove it stopped at the barrier, not at socket I/O.
            std::future::poll_fn(|cx| { assert!(first.as_mut().poll(cx).is_pending()); assert!(second.as_mut().poll(cx).is_pending()); std::task::Poll::Ready(()) }).await;
            let record = sessions.inner.find_record(&handle.rlm_child_id).await.unwrap();
            assert_eq!(record.lock().await.prompt_admission.receiver_count(), 2, "senders must wait at admission before transport");
            let prompt = rx.recv().await.unwrap();
            assert_eq!(prompt.command["type"], "prompt", "same-turn followup must request kickoff before transport");
            assert!(rx.try_recv().is_err(), "transport escaped before prompt ACK");
            prompt.answer.send(true).unwrap();
            let serve = async {
                while let Some(request) = rx.recv().await {
                    // Deny peer ticket to exercise fallback; watcher requests
                    // may interleave, but neither transport precedes ACK.
                    request.answer.send(request.command["type"] != "get_worker_peer_transport").ok();
                }
            };
            tokio::pin!(serve);
            tokio::select! { result = async { (first.await.unwrap(), second.await.unwrap()) } => { assert_eq!(result.0.message, "follow up"); }, () = &mut serve => panic!("fake closed") }
        }
    }).await.unwrap();
}

#[tokio::test]
async fn prompt_admission_close_rejects_waiters_and_late_ack() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let (_dir, sessions, controller, mut rx) = fixture().await;
        let handle = registered(&sessions, &mut rx).await;
        let send = controller.send_agent_message(followup(&handle.rlm_child_id));
        tokio::pin!(send);
        std::future::poll_fn(|cx| { assert!(send.as_mut().poll(cx).is_pending()); std::task::Poll::Ready(()) }).await;
        let prompt = rx.recv().await.unwrap();
        assert_eq!(prompt.command["type"], "prompt");
        let record = sessions.inner.find_record(&handle.rlm_child_id).await.unwrap();
        let close = sessions.inner.close_children_inner(ChildCloseReason::Killed);
        tokio::pin!(close);
        let kill = tokio::select! { request = rx.recv() => request.unwrap(), result = &mut close => panic!("close returned early: {result:?}") };
        assert_eq!(kill.command["type"], "kill");
        assert!(send.await.is_err());
        let mut admission = record.lock().await.prompt_admission.subscribe();
        prompt.answer.send(true).unwrap();
        admission.changed().await.unwrap();
        assert!(controller.send_agent_message(followup(&handle.rlm_child_id)).await.is_err());
        kill.answer.send(true).unwrap();
        close.await.unwrap();
        // Admission success must not replace the closure's durable rejection.
        assert!(record.lock().await.prompt_admission.borrow().as_ref().unwrap().is_err());
        assert!(rx.try_recv().is_err());
    }).await.unwrap();
}

#[tokio::test]
async fn prompt_admission_prompt_failure_rejects_waiting_and_late_senders() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let (_dir, sessions, controller, mut rx) = fixture().await;
        let handle = registered(&sessions, &mut rx).await;
        let send = controller.send_agent_message(followup(&handle.rlm_child_id));
        tokio::pin!(send);
        std::future::poll_fn(|cx| {
            assert!(send.as_mut().poll(cx).is_pending());
            std::task::Poll::Ready(())
        })
        .await;
        for _ in 0..2 {
            let prompt = rx.recv().await.unwrap();
            assert_eq!(prompt.command["type"], "prompt");
            prompt.answer.send(false).unwrap();
        }
        assert!(send.await.is_err());
        assert!(controller
            .send_agent_message(followup(&handle.rlm_child_id))
            .await
            .is_err());
        let kill = rx.recv().await.unwrap();
        assert_eq!(
            kill.command["type"], "kill",
            "no transport after failed prompt"
        );
        kill.answer.send(true).unwrap();
    })
    .await
    .unwrap();
}
