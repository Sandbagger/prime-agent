//! Headless clients use the owned daemon so their root shares its child runtime.
use crate::daemon_client::DaemonClient;
use crate::mode::{AppMode, RunOptions};
use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use std::io::Write;

fn command(kind: &str, session: &str, fields: Value) -> Result<pa_types::daemon::DaemonCommand> {
    let mut value = fields;
    value["type"] = json!(kind);
    value["activeSessionId"] = json!(session);
    Ok(serde_json::from_value(value)?)
}

fn accepted(response: pa_types::daemon::DaemonResponse) -> Result<Value> {
    if !response.success {
        return Err(anyhow!(response
            .error
            .unwrap_or_else(|| "daemon command failed".into())));
    }
    Ok(response.data.unwrap_or(Value::Null))
}

pub(crate) fn run(options: &RunOptions) -> Result<i32> {
    if options.config.api_key.is_some() {
        anyhow::bail!("--api-key is unsupported for an owned daemon headless session; configure provider authentication outside the persisted create config");
    }
    let socket = crate::config::resolve_daemon_socket_path(options.daemon_socket.as_deref());
    // Never spawn or fall back here: this socket is the wrapper's owned runtime.
    let mut client = DaemonClient::connect(&socket)?;
    let (_, create) = crate::print_runtime::daemon_acp_create(options).map_err(|e| anyhow!(e))?;
    let created = accepted(client.request(create)?)?;
    let session = created
        .get("activeSessionId")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| anyhow!("daemon create omitted activeSessionId"))?
        .to_string();
    accepted(client.request(command("attach", &session, json!({}))?)?)?;
    if options.config.initial_goal.is_some()
        && options.initial_message.is_none()
        && options.messages.is_empty()
    {
        // This read forces and awaits core initialization without admitting inference.
        // In particular, --goal alone must persist its seed before the owner closes.
        accepted(client.request(command("get_system_prompt", &session, json!({}))?)?)?;
    }
    let json_mode = options.app_mode == AppMode::Json;
    let mut output = std::io::stdout().lock();
    if json_mode {
        let header =
            accepted(client.request(command("get_session_header", &session, json!({}))?)?)?;
        if let Some(header) = header.get("header").filter(|header| !header.is_null()) {
            serde_json::to_writer(&mut output, header)?;
            writeln!(output)?;
            output.flush()?;
        }
    }
    let timeout_ms = options
        .config
        .autonomous
        .as_ref()
        .and_then(|config| config.timeout_ms)
        .unwrap_or(3_600_000)
        .saturating_add(30_000)
        .min(604_800_000);
    let mut emit = |frame: &Value| -> Result<()> {
        if json_mode
            && frame.get("type").and_then(Value::as_str) == Some("session_event")
            && frame.get("activeSessionId").and_then(Value::as_str) == Some(session.as_str())
        {
            if let Some(event) = frame.get("event") {
                serde_json::to_writer(&mut output, event)?;
                writeln!(output)?;
                output.flush()?;
            }
        }
        Ok(())
    };
    // Admission is sequential; each wait includes goal/autonomous continuations
    // and descendant settlement, so a later CLI prompt cannot race the first.
    let mut status = Value::Null;
    for (prompt, images) in options
        .initial_message
        .iter()
        .map(|prompt| (prompt, options.initial_images.clone()))
        .chain(options.messages.iter().map(|prompt| (prompt, Vec::new())))
    {
        accepted(client.request_with_events(
            command(
                "prompt_and_wait",
                &session,
                json!({"message": prompt, "images": images}),
            )?,
            timeout_ms,
            &mut emit,
        )?)?;
        status = accepted(client.request_with_events(
            command(
                "wait_for_headless_completion",
                &session,
                json!({"waitForRlmQuiescence": true}),
            )?,
            timeout_ms,
            &mut emit,
        )?)?;
    }
    let data = accepted(client.request(command("get_messages", &session, json!({}))?)?)?;
    let mut wire_messages = data.get("messages").cloned().unwrap_or_else(|| json!([]));
    // Durable custom rows retain their ISO entry timestamp; the terminal
    // selector consumes the core message shape with epoch milliseconds.
    // Only normalize this private projection, never the streamed JSON ABI.
    if let Some(messages) = wire_messages.as_array_mut() {
        for message in messages {
            if let Some(timestamp) = message.get("timestamp").and_then(Value::as_str) {
                message["timestamp"] = json!(pa_core::session::timestamp_to_millis(timestamp));
            }
        }
    }
    let messages: Vec<pa_types::session::AgentMessage> = serde_json::from_value(wire_messages)?;
    let result = pa_core::session_engine::headless::select_headless_terminal_result(&messages);
    let mut exit_code = 0;
    if let Some(primary) = result.primary {
        if let Some(text) = primary.stderr_text(&mut exit_code) {
            eprintln!("{text}");
        }
        if !json_mode && exit_code == 0 {
            if let Some(text) = primary.stdout_text() {
                writeln!(output, "{text}")?;
            }
        }
    }
    for outcome in result.compaction_outcomes {
        if !json_mode {
            eprintln!("{}", outcome.content);
        }
        if outcome.outcome == "failed" {
            exit_code = 1;
        }
    }
    if !status.is_null() {
        use pa_core::autonomous::{
            autonomous_limit_reason_of_status, describe_autonomous_limit,
            latest_autonomous_gate_attempt, now_millis,
        };
        let status: pa_core::autonomous::AgentAutonomousStatus = serde_json::from_value(status)?;
        let now = now_millis();
        let limit = autonomous_limit_reason_of_status(&status, now);
        if let Some(failure) = status
            .last_gate_failure
            .as_ref()
            .filter(|_| status.enabled && !status.gates.commands.is_empty())
        {
            eprintln!(
                "Autonomous quality gate still failing after attempt {}/{}: {}",
                latest_autonomous_gate_attempt(&status),
                status.gates.max_retries,
                failure.exit_text
            );
            exit_code = 1;
        } else if status.enabled && status.gates.commands.is_empty() {
            if let Some(reason) = limit {
                eprintln!(
                    "Autonomous run stopped before terminal evidence; {}",
                    describe_autonomous_limit(&status, reason, now)
                );
                exit_code = 1;
            }
        }
    }
    output.flush()?;
    Ok(exit_code)
}
