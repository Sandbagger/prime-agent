# Proposed native goal.wait contract (0.10)

This is an explicit proposed upstream API divergence, not existing TS parity.
The TS 0.10 goal skill exposes get/create/complete and has no wait host route.
Expected TS behavior: goal.wait is absent; a goal.wait host request is unknown.
Expected native behavior: await goal.wait(reason) returns the ordinary serialized
paused goal after durable pause and awaited queued-work withdrawal. No CLI or
user pause/resume syntax changes. Upstream approval and paired TS/native parity
evidence are required before merge; this note does not waive that gate.

The reason is trimmed and must contain 1–2000 Unicode characters. This new bound
keeps operator-facing explanations small without truncating input. Paused calls
preserve all goal state, including manual pause reasons and updated_at, but retry
withdrawal. Idle, complete, error and budget-limited goals reject wait. Manual
pause still says Paused by user. No automatic resume or approval inference exists.
Only /goal resume reactivates the goal, subject to its budget. Resume is not
permission to perform privileged work.

An embedded persisted manager commits Paused before withdrawing work. The daemon
uses a nonpersisted core manager; its awaited goal_state_change_hook commits the
canonical SessionFile state under the admission lock. Live admission is gated
before the write for safety; success requires both canonical persistence and the
recovery checkpoint. A failed write can leave durable state uncertain or older;
retry wait even when already paused. Do not rely on a failed call across restart.
Callers release driver/session locks before the hook. Already executing work
cannot be retracted. Failure reports an error and leaves the live goal paused;
embedded committed
state remains paused, but an embedding's failed canonical write is not a durable
pause guarantee. Retry can finish persistence and withdrawal.
This suppresses goal-owned continuation only, not unrelated heartbeats, autonomous
mode, agent messages or user input, and does not abort the current tool batch.

The optional camelCase continuationEpoch field is a shared pa-types wire addition.
Legacy absence deserializes as None. New ownership transitions (goal ID/status)
get fresh UUID epochs when a goal ID exists; clear removes the epoch. Accounting
and continuation minting retain the epoch.
Recovery uses the persisted identity/epoch to reject stale work without dropping
valid, already-charged work. Legacy queued wake prompts without provenance cannot
be safely identified or automatically removed; no text-marker matching is used.
This is a migration limit, not a claim that every pre-upgrade queued fire is revoked.
A display reason is never an ownership discriminator.

Ownership: pa-types owns shared state; pa-core owns lifecycle/host bridge/skill;
pa-daemon owns queue admission, withdrawal and recovery. No new dependencies.
The internal manager append/persist modules provide a crate-private durable
custom append so failed goal persistence rolls back before index adoption;
ordinary buffered append behavior remains unchanged. Critical ID/status ownership
transitions use private-file copy + sync + atomic rename under the existing
single-writer contract (daemon canonical lease; embedded exclusive manager).
This costs O(session-file bytes) per ownership transition, not per accounting or
mint update. It adds no guarantee for noncooperative external writers. The cache
ownership marker is not a runtime lease and is not used to reject embedded calls.
The schema-v2 additive goal_wait adoption event carries only fixed outcome:
paused or withdrawal_failed. It never carries reason, objective, IDs or content.
Delivery uses the existing fire-and-forget telemetry client.

Regression seams use retained TempDir session stores and direct handlers/driver;
no kernel or provider is required. See operator_wait_* and
 goal_wait_registry_awaits_withdrawal_after_persistence tests plus daemon queue
provenance/recovery tests. Existing numeric backoff constants are unchanged.
