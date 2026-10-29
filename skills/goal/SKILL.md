---
name: goal
description: Manage the persistent thread goal from the Python REPL. Use to read goal status and budget usage, to start a goal when the user explicitly asks for one, to wait for missing operator input, or to mark the active goal complete once its objective is fully achieved.
---

# Goal

The thread goal is a persistent objective the harness keeps re-prompting you to
pursue across turns until it is complete. Goal state (status, token budget,
usage accounting) lives in the host; this skill is the kernel-side interface to
it. Call it directly from the Python REPL:

```python
await goal.get()
await goal.create("ship the release notes")
# Only pass token_budget when the user explicitly asks for one:
# await goal.create("ship the release notes", token_budget=200000)
await goal.complete()
# If blocked on operator input instead of finished:
# await goal.wait("operator approval to deploy")
```

## API

- `await goal.get()` — current goal as a dict: `goal` (or `None` when no goal
  is set), `remaining_tokens`, and `completion_budget_report`. The `goal` dict
  carries `objective`, `status`, `token_budget`, `tokens_used`,
  `time_used_seconds`, and timestamps.
- `await goal.create(objective, token_budget=None)` — start a new active goal.
  Fails while a goal is still pending (active, paused, or budget-limited); a
  completed or errored goal is replaced by the new one. Only create a goal when
  the user or system/developer instructions explicitly ask for a persistent
  long-running goal; do not infer goals from ordinary tasks. Set `token_budget`
  only when an explicit token budget is requested.
- `await goal.complete()` — mark the existing goal achieved. Use only when the
  objective has actually been achieved and no required work remains; do not
  call it merely because the budget is nearly exhausted or because you are
  stopping work. When the result includes a `completion_budget_report`, report
  that final usage to the user.

- `await goal.wait(reason)` — pause automatic goal continuation for missing
  operator input. The trimmed reason must contain 1–2000 Unicode characters;
  oversized input is rejected, never truncated. Only active goals can enter
  wait; a paused goal is unchanged, including a manual pause's reason and time.
  Other inactive states are rejected. The response uses the normal goal shape.
  Finish the current response with the missing input once, then stop. Only the
  operator's `/goal resume` resumes; messages, heartbeat fires and evidence do
  not resume the goal. Resume never grants authorization for privileged work.
  This gates future goal-owned turns, not the current tool batch or unrelated
  autonomous/heartbeat/user inputs. If ownership synchronization fails, the call
  reports an error and the live goal stays paused. Durable state may not yet
  reflect the pause; retry wait to finish synchronization before relying on it.

## Rules

- `goal.wait` is the only agent-callable pause. Resume, clear and budget-limit
  transitions remain controlled by the user and the host.
- When an active goal is actually complete, call `await goal.complete()`; do
  not merely say it is done — the harness keeps continuing the goal until the
  completion call arrives.
