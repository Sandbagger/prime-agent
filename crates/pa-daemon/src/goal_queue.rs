//! Goal-owned queue provenance and durable recovery validation.

use crate::worker::{SessionCore, TurnSettle};
use pa_core::goals::{GoalState, GoalStatus};

const PREFIX: &str = "goal-owned:v1:";

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Provenance {
    goal_id: String,
    epoch: Option<String>,
    purpose: Purpose,
}

#[derive(PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum Purpose {
    Continuation,
    BudgetWrapUp,
}

fn decode(key: &str) -> Option<Provenance> {
    serde_json::from_str(key.strip_prefix(PREFIX)?).ok()
}

pub(crate) fn provenance_key(state: &GoalState) -> Option<String> {
    let purpose = match state.status {
        GoalStatus::Active => Purpose::Continuation,
        GoalStatus::BudgetLimited => Purpose::BudgetWrapUp,
        GoalStatus::Idle | GoalStatus::Paused | GoalStatus::Complete | GoalStatus::Error => {
            return None
        }
    };
    let provenance = Provenance {
        goal_id: state.goal_id.clone()?,
        epoch: state.continuation_epoch.clone(),
        purpose,
    };
    Some(format!(
        "{PREFIX}{}",
        serde_json::to_string(&provenance).expect("goal provenance serializes")
    ))
}

pub(crate) fn is_reserved_key(key: &str) -> bool {
    key.starts_with(PREFIX)
}

pub(crate) fn is_goal_key(key: &str) -> bool {
    decode(key).is_some()
}

pub(crate) fn is_current(key: &str, state: Option<&GoalState>) -> bool {
    let (Some(provenance), Some(state)) = (decode(key), state) else {
        return false;
    };
    let status_matches = match provenance.purpose {
        Purpose::Continuation => state.status == GoalStatus::Active,
        Purpose::BudgetWrapUp => state.status == GoalStatus::BudgetLimited,
    };
    status_matches
        && state.goal_id.as_ref() == Some(&provenance.goal_id)
        && state.continuation_epoch == provenance.epoch
}

pub(crate) fn retain_current(core: &mut SessionCore) {
    let state = core.goal_queue_state.as_ref();
    for lane in [&mut core.steering, &mut core.follow_up] {
        let mut kept = std::collections::VecDeque::new();
        while let Some(mut item) = lane.pop_front() {
            let is_obsolete = item
                .queue_key
                .as_deref()
                .is_some_and(|key| is_goal_key(key) && !is_current(key, state));
            if is_obsolete {
                if let Some(done) = item.done.take() {
                    let _ = done.send(TurnSettle::Withdrawn(
                        "Goal ownership changed before delivery.".to_string(),
                    ));
                }
            } else {
                kept.push_back(item);
            }
        }
        *lane = kept;
    }
}

pub(crate) fn synchronize(core: &mut SessionCore, state: Option<GoalState>) {
    core.goal_queue_state = state;
    retain_current(core);
}

pub(crate) fn commit_worker_state(core: &mut SessionCore, state: &GoalState) -> anyhow::Result<()> {
    synchronize(core, Some(state.clone()));
    if let Some(store) = core.store.as_mut() {
        store.persist_entry("custom", serde_json::json!({
            "customType": pa_core::goals::GOAL_STATE_CUSTOM_TYPE, "data": state
        })).map_err(|error| anyhow::anyhow!("Goal is paused in memory but durability is uncertain; inspect and retry synchronization: {error:#}"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pa_core::goals::{GoalState, GoalStatus};

    #[test]
    fn goal_queue_epoch_survives_recovery_but_not_pause_resume() {
        let active = GoalState {
            active: true,
            status: GoalStatus::Active,
            goal_id: Some("goal-a".to_string()),
            continuation_epoch: Some("epoch-a".to_string()),
            ..GoalState::default()
        };
        let key = provenance_key(&active).unwrap();
        let recovered: GoalState =
            serde_json::from_str(&serde_json::to_string(&active).unwrap()).unwrap();
        assert!(is_current(&key, Some(&recovered)));
        let paused = GoalState {
            active: false,
            status: GoalStatus::Paused,
            ..active.clone()
        };
        assert!(!is_current(&key, Some(&paused)));
        let resumed = GoalState {
            continuation_epoch: Some("epoch-b".to_string()),
            ..active
        };
        assert!(!is_current(&key, Some(&resumed)));
        assert!(!is_current(&key, None));
    }

    #[test]
    fn goal_queue_legacy_active_is_preserved_until_ownership_changes() {
        let legacy = GoalState {
            active: true,
            status: GoalStatus::Active,
            goal_id: Some("legacy".to_string()),
            ..GoalState::default()
        };
        let key = provenance_key(&legacy).unwrap();
        assert!(is_current(&key, Some(&legacy)));
        let resumed = GoalState {
            continuation_epoch: Some("new".to_string()),
            ..legacy
        };
        assert!(!is_current(&key, Some(&resumed)));
        assert!(!is_goal_key("goal-backoff-wake"));
        assert!(!is_goal_key("user says goal-owned:{broken}"));
    }

    fn item(message: &str, key: Option<String>) -> crate::worker::QueuedItem {
        crate::worker::QueuedItem {
            message: message.to_string(),
            priority: crate::worker::QueuePriority::Background,
            preview: None,
            custom_message: None,
            agent_message: None,
            queue_key: key,
            admission_id: None,
            images: Vec::new(),
            done: None,
            queue_visible: true,
            policy: crate::worker::TurnPolicy::Injected,
            forced_batch: false,
        }
    }

    #[test]
    fn goal_queue_invalidation_preserves_independent_inputs_and_rejects_late_admission() {
        let active = GoalState {
            active: true,
            status: GoalStatus::Active,
            goal_id: Some("goal".to_string()),
            continuation_epoch: Some("old".to_string()),
            ..GoalState::default()
        };
        let key = provenance_key(&active).unwrap();
        let core = std::sync::Arc::new(std::sync::Mutex::new(
            crate::worker::SessionCore::test_core(None, "/tmp".to_string()),
        ));
        {
            let mut core = core.lock().unwrap();
            synchronize(&mut core, Some(active.clone()));
            core.follow_up
                .push_back(item("charged continuation", Some(key.clone())));
            core.follow_up.push_back(item("user input", None));
            core.steering.push_back(item(
                "independent heartbeat",
                Some("heartbeat:independent".to_string()),
            ));
        }
        // The fire/mint captured the old key before pause; deliver only after withdrawal.
        let ready = std::sync::Arc::new(std::sync::Barrier::new(2));
        let deliver = std::sync::Arc::new(std::sync::Barrier::new(2));
        let task = {
            let core = core.clone();
            let ready = ready.clone();
            let deliver = deliver.clone();
            std::thread::spawn(move || {
                ready.wait();
                deliver.wait();
                let mut core = core.lock().unwrap();
                if is_current(&key, core.goal_queue_state.as_ref()) {
                    core.follow_up.push_back(item("late fire", Some(key)));
                }
            })
        };
        ready.wait();
        {
            let mut core = core.lock().unwrap();
            synchronize(
                &mut core,
                Some(GoalState {
                    active: false,
                    status: GoalStatus::Paused,
                    continuation_epoch: Some("paused".to_string()),
                    ..active
                }),
            );
        }
        deliver.wait();
        task.join().unwrap();
        let core = core.lock().unwrap();
        assert_eq!(
            core.follow_up
                .iter()
                .map(|item| item.message.as_str())
                .collect::<Vec<_>>(),
            ["user input"]
        );
        assert_eq!(
            core.steering
                .iter()
                .map(|item| item.message.as_str())
                .collect::<Vec<_>>(),
            ["independent heartbeat"]
        );
    }

    #[test]
    fn goal_queue_budget_wrap_up_is_distinct_from_active_continuation() {
        let active = GoalState {
            active: true,
            status: GoalStatus::Active,
            goal_id: Some("goal".to_string()),
            continuation_epoch: Some("active".to_string()),
            ..GoalState::default()
        };
        let active_key = provenance_key(&active).unwrap();
        let budget = GoalState {
            active: false,
            status: GoalStatus::BudgetLimited,
            continuation_epoch: Some("budget".to_string()),
            ..active
        };
        let budget_key = provenance_key(&budget).unwrap();
        assert!(is_current(&budget_key, Some(&budget)));
        assert!(!is_current(&active_key, Some(&budget)));
        let paused = GoalState {
            status: GoalStatus::Paused,
            ..budget
        };
        assert!(!is_current(&active_key, Some(&paused)));
        assert!(!is_current(&budget_key, Some(&paused)));
    }

    #[test]
    fn goal_queue_recovery_preserves_current_wake_and_withdraws_paused_epoch() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("recovery.jsonl");
        let active = GoalState {
            active: true,
            status: GoalStatus::Active,
            goal_id: Some("goal".to_string()),
            continuation_epoch: Some("epoch".to_string()),
            ..GoalState::default()
        };
        let key = provenance_key(&active).unwrap();
        let mut core =
            crate::worker::SessionCore::test_core(None, dir.path().to_string_lossy().to_string());
        synchronize(&mut core, Some(active.clone()));
        core.follow_up.push_back(item("only owed wake", Some(key)));
        core.follow_up.push_back(item("user", None));
        let lanes = crate::worker::queue_lanes(&core);
        let mut journal = crate::journal::WorkerRecoveryJournal::open(&path).unwrap();
        journal
            .record_queue_snapshot("session", &lanes.steering, &lanes.follow_up)
            .unwrap();
        drop(journal);
        let reopened = crate::journal::WorkerRecoveryJournal::open(&path).unwrap();
        let (steering, follow_up) = crate::worker::restore_queue_snapshot(&reopened, "session");
        core.steering = steering;
        core.follow_up = follow_up;
        synchronize(&mut core, Some(active.clone()));
        assert_eq!(
            core.follow_up.len(),
            2,
            "same-epoch restart must not lose the only owed wake"
        );
        // Simulate crash after durable pause but before the queue checkpoint.
        synchronize(
            &mut core,
            Some(GoalState {
                active: false,
                status: GoalStatus::Paused,
                continuation_epoch: Some("paused".to_string()),
                ..active
            }),
        );
        assert_eq!(
            core.follow_up
                .iter()
                .map(|item| item.message.as_str())
                .collect::<Vec<_>>(),
            ["user"]
        );
        let lanes = crate::worker::queue_lanes(&core);
        let mut journal = crate::journal::WorkerRecoveryJournal::open(&path).unwrap();
        journal
            .record_queue_snapshot("session", &lanes.steering, &lanes.follow_up)
            .unwrap();
        drop(journal);
        let journal = crate::journal::WorkerRecoveryJournal::open(&path).unwrap();
        let (_, follow_up) = crate::worker::restore_queue_snapshot(&journal, "session");
        assert_eq!(
            follow_up
                .iter()
                .map(|item| item.message.as_str())
                .collect::<Vec<_>>(),
            ["user"]
        );
    }

    #[test]
    fn goal_queue_worker_persist_failure_is_fail_closed_and_explicitly_uncertain() {
        let dir = tempfile::TempDir::new().unwrap();
        let active = GoalState {
            active: true,
            status: GoalStatus::Active,
            goal_id: Some("goal".to_string()),
            continuation_epoch: Some("active".to_string()),
            ..GoalState::default()
        };
        let key = provenance_key(&active).unwrap();
        let mut store = crate::session_store::SessionFile::create("/tmp", None, 0);
        let path = dir.path().join("session.jsonl");
        store.set_path(path.clone());
        store.persist_entry("custom", serde_json::json!({"customType": pa_core::goals::GOAL_STATE_CUSTOM_TYPE, "data": active})).unwrap();
        let original = std::fs::read(&path).unwrap();
        // An invalid parent forces the canonical worker append to fail before any write.
        let blocker = dir.path().join("not-a-directory");
        std::fs::write(&blocker, "block").unwrap();
        store.set_path(blocker.join("session.jsonl"));
        let mut core = crate::worker::SessionCore::test_core(Some(store), "/tmp".to_string());
        synchronize(&mut core, Some(active.clone()));
        core.follow_up
            .push_back(item("stale continuation", Some(key.clone())));
        let paused = GoalState {
            active: false,
            status: GoalStatus::Paused,
            continuation_epoch: Some("paused".to_string()),
            ..active.clone()
        };
        let error = commit_worker_state(&mut core, &paused).unwrap_err();
        assert!(error.to_string().contains("durability is uncertain"));
        assert!(core.follow_up.is_empty());
        assert!(!is_current(&key, core.goal_queue_state.as_ref()));
        assert_eq!(std::fs::read(&path).unwrap(), original);
        // No successful wait may be claimed: a restart still sees the prior Active goal.
        let restored = crate::session_store::SessionFile::open(&path).unwrap();
        assert_eq!(
            crate::goal_state_persist::goal_state_in_session_file(&restored),
            Some(active)
        );
        // Retry after restoring a writable canonical path must write even though live state is equal.
        core.store.as_mut().unwrap().set_path(path.clone());
        let paused = core.goal_queue_state.clone().unwrap();
        commit_worker_state(&mut core, &paused).unwrap();
        let restored = crate::session_store::SessionFile::open(&path).unwrap();
        assert_eq!(
            crate::goal_state_persist::goal_state_in_session_file(&restored),
            Some(paused)
        );
    }

    #[test]
    fn goal_queue_checkpoint_failure_reports_error_after_canonical_pause() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("journal.jsonl");
        let journal = crate::journal::WorkerRecoveryJournal::open(&path).unwrap();
        // Journal append must fail deterministically; the path becomes a directory.
        if path.exists() {
            std::fs::remove_file(&path).unwrap();
        }
        std::fs::create_dir(&path).unwrap();
        let recovery = std::sync::Mutex::new(Some(journal));
        let paused = GoalState {
            active: false,
            status: GoalStatus::Paused,
            goal_id: Some("goal".to_string()),
            continuation_epoch: Some("paused".to_string()),
            ..GoalState::default()
        };
        let mut store = crate::session_store::SessionFile::create("/tmp", None, 0);
        store.set_path(dir.path().join("session.jsonl"));
        let core = std::sync::Mutex::new(crate::worker::SessionCore::test_core(
            Some(store),
            "/tmp".to_string(),
        ));
        commit_worker_state(&mut core.lock().unwrap(), &paused).unwrap();
        assert!(crate::worker::checkpoint_queue_recovery_checked(
            &recovery,
            &core,
            crate::worker::QueueCheckpoint::Settle {
                operation: "queue_purged"
            }
        )
        .is_err());
        let restored =
            crate::session_store::SessionFile::open(&dir.path().join("session.jsonl")).unwrap();
        assert_eq!(
            crate::goal_state_persist::goal_state_in_session_file(&restored),
            Some(paused)
        );
    }

    #[test]
    fn post_compaction_admission_rejects_stale_mint_after_pause_barrier() {
        let core = std::sync::Arc::new(std::sync::Mutex::new(
            crate::worker::SessionCore::test_core(None, "/tmp".to_string()),
        ));
        let active = GoalState {
            active: true,
            status: GoalStatus::Active,
            goal_id: Some("goal".to_string()),
            continuation_epoch: Some("before".to_string()),
            ..GoalState::default()
        };
        let pending = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        let minted = crate::engine::GoalContinuation {
            request: crate::engine::PromptRequest {
                batch: Vec::new(),
                message: "charged mint".to_string(),
                images: Vec::new(),
                source: "user".to_string(),
                agent_message_id: None,
                custom_message: None,
            },
            goal_update: Some(serde_json::to_value(&active).unwrap()),
            pending_handle: Some(pending.clone()),
            queue_key: provenance_key(&active),
        };
        let gate = std::sync::Arc::new(std::sync::Barrier::new(2));
        let task = {
            let core = core.clone();
            let gate = gate.clone();
            std::thread::spawn(move || {
                gate.wait();
                crate::worker::admit_post_compaction_goal(
                    &std::sync::Mutex::new(None),
                    &core,
                    &std::sync::Arc::new(crate::worker::EventPump::new()),
                    &std::sync::Arc::new(tokio::sync::Notify::new()),
                    minted,
                );
            })
        };
        synchronize(
            &mut core.lock().unwrap(),
            Some(GoalState {
                active: false,
                status: GoalStatus::Paused,
                continuation_epoch: Some("paused".to_string()),
                ..active
            }),
        );
        gate.wait();
        task.join().unwrap();
        assert!(core.lock().unwrap().follow_up.is_empty());
        assert!(!pending.load(std::sync::atomic::Ordering::SeqCst));
    }
}
