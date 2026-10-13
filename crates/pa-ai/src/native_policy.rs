//! Opt-in substrate policy transport. Native calls fail closed when policy is enabled.

use crate::types::{
    AssistantMessageEvent, ErrorStopReason, Model, SimpleStreamOptions, StopReason,
};
use crate::{AssistantMessageEventExt, AssistantMessageEventStream};
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    OnceLock,
};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

static SESSION: OnceLock<String> = OnceLock::new();
static DURABLE_SESSION: OnceLock<String> = OnceLock::new();
static CLIENT: OnceLock<Option<Client>> = OnceLock::new();
static CHILDREN: OnceLock<std::sync::Mutex<std::collections::HashMap<String, String>>> =
    OnceLock::new();

/// Retain the policy grant until the native child reaches its terminal funnel.
pub fn remember_child(child: &str, grant: &str) {
    CHILDREN
        .get_or_init(Default::default)
        .lock()
        .expect("child grants")
        .insert(child.into(), grant.into());
}

/// Release a grant only after the native worker reports terminal settlement.
pub async fn settle_child(child: &str, quiescence_proven: bool) -> Result<(), String> {
    let grant = CHILDREN
        .get_or_init(Default::default)
        .lock()
        .map_err(|_| "child grants poisoned")?
        .remove(child);
    if let Some(grant) = grant {
        request(
            "child_settle",
            json!({"grant_id":grant,"quiescence_proven":quiescence_proven}),
        )
        .await?;
    }
    Ok(())
}

struct Client {
    socket: String,
    secret: String,
    grant: Option<String>,
    depth: Option<u64>,
    broken: AtomicBool,
    bound: tokio::sync::Mutex<bool>,
}

fn client() -> Option<&'static Client> {
    CLIENT
        .get_or_init(|| {
            std::env::var("DOTFILES_PRIME_NATIVE_SOCKET")
                .ok()
                .map(|socket| Client {
                    socket,
                    secret: std::env::var("DOTFILES_PRIME_POLICY_SECRET").unwrap_or_default(),
                    grant: std::env::var("DOTFILES_PRIME_NATIVE_GRANT").ok(),
                    depth: std::env::var("DOTFILES_PRIME_NATIVE_DEPTH")
                        .ok()
                        .map(|value| value.parse().ok())
                        .unwrap_or(Some(0)),
                    broken: AtomicBool::new(false),
                    bound: tokio::sync::Mutex::new(false),
                })
        })
        .as_ref()
}

/// Bind process identity from the host, before model-controlled execution starts.
pub fn initialize(session_id: &str) -> Result<(), String> {
    if client().is_none() {
        return Ok(());
    }
    if let Some(existing) = SESSION.get() {
        if existing != session_id {
            return Err("substrate process identity changed".into());
        }
    } else {
        SESSION
            .set(session_id.to_owned())
            .map_err(|_| "substrate identity race")?;
    }
    Ok(())
}

/// Whether this process was launched with the native policy bridge enabled.
pub fn enabled() -> bool {
    client().is_some()
}

/// Remove policy capabilities from model-controlled tool subprocesses.
pub fn scrub_tool_command(command: &mut std::process::Command) {
    for name in [
        "DOTFILES_PRIME_POLICY_SECRET",
        "DOTFILES_PRIME_NATIVE_SOCKET",
        "DOTFILES_PRIME_NATIVE_GRANT",
        "DOTFILES_PRIME_NATIVE_DEPTH",
    ] {
        command.env_remove(name);
    }
}

/// Start another operator prompt without treating retries or continuations as new budgets.
pub async fn rearm_operator_prompt() -> Result<(), String> {
    if let Some(client) = client() {
        if *client.bound.lock().await {
            request("root_rearm", json!({"operator_prompt":true})).await?;
        }
    }
    Ok(())
}

/// Record an explicit operator abort before another root prompt can re-arm it.
pub async fn cancel_operator_prompt() -> Result<(), String> {
    if let Some(client) = client() {
        if client.grant.is_none() && *client.bound.lock().await {
            request("root_cancel", json!({"reason":"user_abort"})).await?;
        }
    }
    Ok(())
}

/// Attach the durable history identity supplied by session assembly.
pub fn durable_session(session_id: &str) -> Result<(), String> {
    if !enabled() {
        return Ok(());
    }
    if SESSION.get().is_none() {
        initialize(session_id)?;
    }
    if let Some(existing) = DURABLE_SESSION.get() {
        if existing != session_id {
            return Err("substrate durable session switch denied".into());
        }
    } else {
        DURABLE_SESSION
            .set(session_id.to_owned())
            .map_err(|_| "substrate durable identity race")?;
    }
    Ok(())
}

fn validate_reply(value: Value) -> Result<Value, String> {
    if value.get("ok") == Some(&Value::Bool(true)) {
        Ok(value)
    } else {
        Err(format!(
            "substrate policy rejected request: {}",
            value
                .get("code")
                .and_then(Value::as_str)
                .unwrap_or("invalid_reply")
        ))
    }
}

fn malformed_policy(reason: &str) -> String {
    if let Some(client) = client() {
        client.broken.store(true, Ordering::SeqCst);
    }
    reason.to_owned()
}

/// Execute a policy operation using immutable process/session identity. No retries.
pub async fn request(op: &str, mut body: Value) -> Result<Value, String> {
    let Some(client) = client() else {
        return Ok(json!({"ok":true}));
    };
    if client.broken.load(Ordering::SeqCst) {
        return Err("substrate policy connection is poisoned".into());
    }
    let session = SESSION
        .get()
        .ok_or("substrate process identity is unbound")?;
    body["version"] = json!(1);
    body["op"] = json!(op);
    body["pid"] = json!(std::process::id());
    body["session_id"] = json!(session);
    body["secret"] = json!(client.secret);
    let timeout = if op == "call_check" {
        std::time::Duration::from_millis(200)
    } else {
        std::time::Duration::from_secs(30)
    };
    let result = tokio::time::timeout(timeout, async {
        let stream = tokio::net::UnixStream::connect(&client.socket)
            .await
            .map_err(|e| e.to_string())?;
        let (reader, mut writer) = stream.into_split();
        let mut wire = serde_json::to_vec(&body).map_err(|e| e.to_string())?;
        wire.push(b'\n');
        writer.write_all(&wire).await.map_err(|e| e.to_string())?;
        let mut reader = BufReader::new(reader.take(1_048_577));
        let mut reply = Vec::new();
        reader
            .read_until(b'\n', &mut reply)
            .await
            .map_err(|e| e.to_string())?;
        if reply.len() > 1_048_576 || reply.last() != Some(&b'\n') {
            return Err("invalid substrate policy frame".into());
        }
        let value: Value =
            serde_json::from_slice(&reply).map_err(|_| "invalid substrate policy JSON")?;
        if !value.get("ok").is_some_and(Value::is_boolean) {
            return Err("invalid substrate policy reply".into());
        }
        Ok(value)
    })
    .await
    .map_err(|_| "substrate policy timeout".to_string())
    .and_then(|v| v);
    if result.is_err() && !op.starts_with("telemetry_") {
        client.broken.store(true, Ordering::SeqCst);
    }
    result.and_then(validate_reply)
}

/// Telemetry uses the authenticated transport without poisoning policy on sink failure.
pub async fn request_telemetry(op: &str, body: Value) -> Result<Value, String> {
    if !op.starts_with("telemetry_") {
        return Err("invalid telemetry operation".into());
    }
    request(op, body).await
}

async fn bind(model: &Model, effort: Value) -> Result<(), String> {
    let Some(client) = client() else {
        return Ok(());
    };
    let mut bound = client.bound.lock().await;
    if !*bound {
        let mut body = json!({"selector":format!("{}/{}", model.provider, model.id), "effort":effort, "depth":client.depth.ok_or("invalid native depth")?, "durable_session_id":DURABLE_SESSION.get().ok_or("substrate durable identity unavailable")?});
        let op = if let Some(grant) = &client.grant {
            body["grant_id"] = json!(grant);
            "child_bind"
        } else {
            "root_bind"
        };
        request(op, body).await?;
        *bound = true;
    }
    Ok(())
}

/// Delay provider execution until admitted, then settle only its terminal usage.
pub(crate) fn govern<F>(
    model: Model,
    mut options: SimpleStreamOptions,
    invoke: F,
) -> AssistantMessageEventStream
where
    F: FnOnce(SimpleStreamOptions) -> AssistantMessageEventStream + Send + 'static,
{
    let (writer, output) = AssistantMessageEventStream::new();
    let cancellation = options
        .base
        .signal
        .clone()
        .unwrap_or_default()
        .child_token();
    let output = output.cancel_on_drop(cancellation.clone());
    options.base.signal = Some(cancellation.clone());
    tokio::spawn(async move {
        let run = async {
            let effort = serde_json::to_value(options.reasoning).map_err(|e| e.to_string())?;
            bind(&model, effort.clone()).await?;
            let permit = request("call_admit", json!({"selector":format!("{}/{}",model.provider,model.id),"effort":effort,"service_tier":options.base.service_tier})).await?;
            let permit_id = permit.get("permit_id").and_then(Value::as_str).filter(|id| !id.is_empty()).ok_or_else(|| malformed_policy("missing policy permit"))?.to_owned();
            options.reasoning = Some(serde_json::from_value(permit["effort"].clone()).map_err(|_| malformed_policy("invalid policy effort"))?);
            options.base.service_tier = Some(serde_json::from_value(permit["service_tier"].clone()).map_err(|_| malformed_policy("invalid policy tier"))?);
            if cancellation.is_cancelled() {
                request("call_settle", json!({"permit_id":permit_id,"message":null,"error":{"code":"call_cancelled","message":"Cancelled before provider invocation"}})).await?;
                let mut message = crate::event_stream::initial_assistant_message(&model.api,&model.provider,&model.id);
                message.stop_reason = StopReason::Aborted;
                writer.push(AssistantMessageEvent::Error { reason: ErrorStopReason::Aborted, error: message });
                return Ok(());
            }
            let mut stream = invoke(options);
            let mut checks = tokio::time::interval(std::time::Duration::from_millis(200));
            let mut policy_error: Option<String> = None;
            let drain_deadline = tokio::time::sleep(std::time::Duration::from_secs(30));
            tokio::pin!(drain_deadline);
            loop {
                tokio::select! {
                    event = stream.next_event() => {
                        let event = event.ok_or_else(|| malformed_policy("provider ended without terminal usage"))?;
                        if let Some(mut message) = event.terminal_message() {
                            let settlement = request("call_settle", json!({"permit_id":permit_id,"message":message})).await;
                            if let Err(error) = settlement {
                                message.stop_reason = StopReason::Error;
                                message.error_message = Some(error);
                                writer.push(AssistantMessageEvent::Error { reason: ErrorStopReason::Error, error: message });
                            } else { writer.push(event); }
                            return Ok::<(), String>(());
                        }
                        writer.push(event);
                    }
                    _ = checks.tick(), if policy_error.is_none() => {
                        if let Err(error) = request("call_check", json!({"permit_id":permit_id})).await {
                            cancellation.cancel();
                            policy_error = Some(error);
                            drain_deadline.as_mut().reset(tokio::time::Instant::now() + std::time::Duration::from_secs(5));
                        }
                    }
                    _ = cancellation.cancelled(), if policy_error.is_none() => {
                        policy_error = Some("provider cancellation did not settle".into());
                        drain_deadline.as_mut().reset(tokio::time::Instant::now() + std::time::Duration::from_secs(5));
                    }
                    _ = &mut drain_deadline, if policy_error.is_some() => {
                        if let Some(client) = client() { client.broken.store(true, Ordering::SeqCst); }
                        return Err(policy_error.take().unwrap_or_else(|| "provider cancellation did not settle".into()));
                    }
                }
            }
        }.await;
        if let Err(error) = run {
            cancellation.cancel();
            let mut message = crate::event_stream::initial_assistant_message(
                &model.api,
                &model.provider,
                &model.id,
            );
            message.stop_reason = StopReason::Error;
            message.error_message = Some(error);
            writer.push(AssistantMessageEvent::Error {
                reason: ErrorStopReason::Error,
                error: message,
            });
        }
    });
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_subprocess_cannot_inherit_policy_capabilities() {
        let mut command = std::process::Command::new("sh");
        for name in [
            "DOTFILES_PRIME_POLICY_SECRET",
            "DOTFILES_PRIME_NATIVE_SOCKET",
            "DOTFILES_PRIME_NATIVE_GRANT",
            "DOTFILES_PRIME_NATIVE_DEPTH",
        ] {
            command.env(name, "sensitive");
        }
        command.env("ORDINARY_TOOL_SETTING", "retained");
        scrub_tool_command(&mut command);
        let env = command
            .get_envs()
            .map(|(key, value)| {
                (
                    key.to_string_lossy().into_owned(),
                    value.map(|v| v.to_string_lossy().into_owned()),
                )
            })
            .collect::<std::collections::BTreeMap<_, _>>();
        assert_eq!(env.get("DOTFILES_PRIME_POLICY_SECRET"), Some(&None));
        assert_eq!(env.get("DOTFILES_PRIME_NATIVE_SOCKET"), Some(&None));
        assert_eq!(env.get("DOTFILES_PRIME_NATIVE_GRANT"), Some(&None));
        assert_eq!(env.get("DOTFILES_PRIME_NATIVE_DEPTH"), Some(&None));
        assert_eq!(
            env.get("ORDINARY_TOOL_SETTING"),
            Some(&Some("retained".into()))
        );
    }

    #[test]
    fn policy_rejects_malformed_or_denied_replies() {
        assert!(validate_reply(serde_json::json!({"ok": false, "code": "denied"})).is_err());
        assert!(validate_reply(serde_json::json!({"ok": "true"})).is_err());
        assert_eq!(
            validate_reply(serde_json::json!({"ok": true})).unwrap(),
            serde_json::json!({"ok": true})
        );
    }

    #[test]
    fn denied_admission_never_invokes_provider() {
        if std::env::var("PA_NATIVE_POLICY_TEST_CHILD").is_ok() {
            let runtime = tokio::runtime::Runtime::new().unwrap();
            runtime.block_on(async {
                initialize("active-test").unwrap();
                durable_session("durable-test").unwrap();
                let model = crate::models_generated::get_models("openai")[0];
                let stream = govern(
                    model.clone(),
                    SimpleStreamOptions {
                        reasoning: Some(crate::types::ModelThinkingLevel::High),
                        ..Default::default()
                    },
                    |_| panic!("denied policy invoked provider"),
                );
                let result = stream.result().await;
                assert_eq!(result.stop_reason, StopReason::Error);
                assert!(result.error_message.unwrap().contains("denied"));
            });
            return;
        }
        use std::io::{BufRead, Write};
        let socket =
            std::env::temp_dir().join(format!("pa-policy-test-{}.sock", std::process::id()));
        let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        let server = std::thread::spawn(move || {
            for expected in ["root_bind", "call_admit"] {
                let (mut stream, _) = listener.accept().unwrap();
                let mut line = String::new();
                std::io::BufReader::new(stream.try_clone().unwrap())
                    .read_line(&mut line)
                    .unwrap();
                let request: Value = serde_json::from_str(&line).unwrap();
                assert_eq!(request["op"], expected);
                assert_eq!(request["session_id"], "active-test");
                assert_eq!(request["secret"], "test-secret");
                let reply = if expected == "root_bind" {
                    "{\"ok\":true}\n"
                } else {
                    "{\"ok\":false,\"code\":\"denied\"}\n"
                };
                stream.write_all(reply.as_bytes()).unwrap();
            }
        });
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "native_policy::tests::denied_admission_never_invokes_provider",
                "--nocapture",
            ])
            .env("PA_NATIVE_POLICY_TEST_CHILD", "1")
            .env("DOTFILES_PRIME_NATIVE_SOCKET", &socket)
            .env("DOTFILES_PRIME_POLICY_SECRET", "test-secret")
            .status()
            .unwrap();
        assert!(status.success());
        server.join().unwrap();
        std::fs::remove_file(socket).unwrap();
    }

    #[test]
    fn terminal_usage_is_settled_before_completion_is_visible() {
        if std::env::var("PA_NATIVE_SETTLEMENT_CHILD").is_ok() {
            tokio::runtime::Runtime::new().unwrap().block_on(async {
                initialize("settlement-active").unwrap();
                durable_session("settlement-durable").unwrap();
                let model = crate::models_generated::get_models("openai")[0].clone();
                let message_model = model.clone();
                let stream = govern(
                    model,
                    SimpleStreamOptions {
                        reasoning: Some(crate::types::ModelThinkingLevel::High),
                        ..Default::default()
                    },
                    move |options| {
                        assert_eq!(
                            options.reasoning,
                            Some(crate::types::ModelThinkingLevel::Medium)
                        );
                        let (writer, stream) = AssistantMessageEventStream::new();
                        let mut message = crate::event_stream::initial_assistant_message(
                            &message_model.api,
                            &message_model.provider,
                            &message_model.id,
                        );
                        message.usage.total_tokens = 17;
                        tokio::spawn(async move {
                            options.base.signal.unwrap().cancelled().await;
                            message.stop_reason = StopReason::Aborted;
                            writer.push(AssistantMessageEvent::Error {
                                reason: ErrorStopReason::Aborted,
                                error: message,
                            });
                        });
                        stream
                    },
                );
                let result = stream.result().await;
                assert_eq!(result.usage.total_tokens, 17);
                assert_eq!(result.stop_reason, StopReason::Error);
            });
            return;
        }
        use std::io::{BufRead, Write};
        let socket =
            std::env::temp_dir().join(format!("pa-settlement-test-{}.sock", std::process::id()));
        let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        let server = std::thread::spawn(move || loop {
            let (mut stream, _) = listener.accept().unwrap();
            let mut line = String::new();
            std::io::BufReader::new(stream.try_clone().unwrap())
                .read_line(&mut line)
                .unwrap();
            let request: Value = serde_json::from_str(&line).unwrap();
            let op = request["op"].as_str().unwrap();
            let reply = match op {
                "call_admit" => {
                    json!({"ok":true,"permit_id":"permit-test","effort":"medium","service_tier":"default"})
                }
                "root_bind" => json!({"ok":true}),
                "call_check" => json!({"ok":false,"code":"budget_aborted"}),
                "call_settle" => {
                    assert_eq!(request["message"]["usage"]["totalTokens"], 17);
                    assert_eq!(request["permit_id"], "permit-test");
                    json!({"ok":false,"code":"token_budget_exceeded"})
                }
                _ => panic!("unexpected op: {op}"),
            };
            writeln!(stream, "{reply}").unwrap();
            if op == "call_settle" {
                break;
            }
        });
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "native_policy::tests::terminal_usage_is_settled_before_completion_is_visible",
                "--nocapture",
            ])
            .env("PA_NATIVE_SETTLEMENT_CHILD", "1")
            .env("DOTFILES_PRIME_NATIVE_SOCKET", &socket)
            .env("DOTFILES_PRIME_POLICY_SECRET", "test-secret")
            .status()
            .unwrap();
        assert!(status.success());
        server.join().unwrap();
        std::fs::remove_file(socket).unwrap();
    }
}
