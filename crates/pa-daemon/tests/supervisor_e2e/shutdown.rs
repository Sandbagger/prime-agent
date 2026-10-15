//! The supervisor process-exit paths - the shutdown command and the SIGTERM
//! graceful drain - with platform process-tree probes.

use super::*;
#[path = "../support/processes.rs"]
mod process_probes;
use process_probes::{child_pids_of, process_alive};

fn wait_child_exit(child: &mut Child, timeout: Duration) -> Option<std::process::ExitStatus> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait().expect("try_wait") {
            return Some(status);
        }
        if Instant::now() > deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The shutdown must stop every worker and exit the supervisor, cleaning
/// up its socket: a supervisor parked on its socket blocks replacement.
#[test]
fn shutdown_command_exits_the_supervisor_process() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let socket = dir.path().join("daemon.sock");
    let agent_dir = dir.path().join("agent");
    std::fs::create_dir_all(agent_dir.join("sessions")).expect("sessions dir");
    let mut daemon = spawn_daemon(&socket, &agent_dir);
    let supervisor_pid = daemon.child.id();
    let (mut client, _hello) = Client::connect(&socket);

    let script_path = dir.path().join("script.json");
    std::fs::write(
        &script_path,
        serde_json::json!({ "responses": [ { "text": "x" } ] }).to_string(),
    )
    .expect("write script");
    client.send_command(
        "c1",
        &serde_json::json!({
            "type": "create",
            "config": {
                "cwd": dir.path().to_string_lossy(),
                "sessionDir": agent_dir.join("sessions").to_string_lossy(),
                "script": script_path.to_string_lossy(),
            },
        }),
    );
    let created = client.read_response("c1");
    assert_eq!(created["success"], true, "create failed: {created}");
    // The supervisor spawned exactly one worker child for the session.
    let deadline = Instant::now() + Duration::from_secs(10);
    let worker_pids = loop {
        let children = child_pids_of(supervisor_pid);
        if !children.is_empty() {
            break children;
        }
        assert!(Instant::now() < deadline, "worker never spawned");
        std::thread::sleep(Duration::from_millis(50));
    };
    assert_eq!(worker_pids.len(), 1, "one worker per session");

    client.send_command("sd", &serde_json::json!({ "type": "shutdown" }));
    let shutdown = client.read_response("sd");
    assert_eq!(shutdown["success"], true, "shutdown failed: {shutdown}");

    let exit = wait_child_exit(&mut daemon.child, Duration::from_secs(10))
        .expect("the supervisor process exited after shutdown");
    assert!(exit.success(), "supervisor exit: {exit:?}");
    assert_socket_reusable(&socket);

    let deadline = Instant::now() + Duration::from_secs(10);
    for pid in worker_pids {
        while process_alive(pid) {
            assert!(
                Instant::now() < deadline,
                "worker {pid} leaked after shutdown"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

/// SIGTERM to the live supervisor is the graceful drain, not the default
/// signal death: the process exits 0 and cleans its socket file up. A
/// connected client rides the drain.
#[test]
fn a_sigterm_exits_the_supervisor_through_the_graceful_drain() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let socket = dir.path().join("daemon.sock");
    let agent_dir = dir.path().join("agent");
    std::fs::create_dir_all(&agent_dir).expect("agent dir");
    let mut daemon = spawn_daemon(&socket, &agent_dir);
    let (_client, hello) = Client::connect(&socket);
    assert_eq!(hello["type"], "daemon_hello");
    let _ = Command::new("kill")
        .arg("-TERM")
        .arg(daemon.child.id().to_string())
        .status()
        .expect("SIGTERM the supervisor");
    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        if let Ok(Some(status)) = daemon.child.try_wait() {
            break status;
        }
        assert!(
            Instant::now() < deadline,
            "the drained supervisor must exit"
        );
        std::thread::sleep(Duration::from_millis(20));
    };
    assert!(
        status.success(),
        "the drain must end in a clean exit 0, not a signal death: {status}"
    );
    assert_socket_reusable(&socket);
}

// Exit cleanup is conservative on BSD: verify the production rebinding path
// rather than requiring Linux's stronger stale-listener deletion proof.
fn assert_socket_reusable(socket: &std::path::Path) {
    #[cfg(target_os = "linux")]
    assert!(!socket.exists(), "the socket file is removed on exit");
    #[cfg(not(target_os = "linux"))]
    {
        assert!(std::os::unix::net::UnixStream::connect(socket).is_err());
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("socket reuse runtime")
            .block_on(pa_daemon::socket::prepare_socket_path(socket))
            .expect("prepare the stopped supervisor socket for reuse");
        let listener = std::os::unix::net::UnixListener::bind(socket).expect("rebind supervisor path");
        drop(std::os::unix::net::UnixStream::connect(socket).expect("connect rebound listener"));
        drop(listener);
        std::fs::remove_file(socket).expect("remove fixture replacement socket");
    }
}
