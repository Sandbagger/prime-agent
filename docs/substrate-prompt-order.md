# Spawn prompt admission precedes parent follow-ups

A child `spawn` returns its handle before its initial `[task from parent]` prompt is sent. The prompt normally waits for the parent's current turn boundary, while a parent `agent_message.send` could previously take the direct peer or supervisor fallback immediately. In a real run, the follow-up row and assistant response therefore appeared before the task prompt.

The parent-to-own-child send path now releases the existing detached kickoff via the collect notification, then waits on a retained watch-state result from initial prompt admission before usage re-arming and transport. The watch closes the lost-wakeup window for concurrent senders and already-completed prompt admission. Admission errors and parent close reject waiting senders; a late prompt success cannot overwrite a closure rejection. Explicit bounded collect continues to release the same kickoff.

This fixes only parent-to-own-child follow-up ordering. It does not establish ordering for sibling or external senders, nor change spawn's deferred return contract.

Regression coverage uses a gated fake supervisor and no real provider. The ordering case starts real deferred spawn, polls concurrent sends to their first suspension, checks their admission subscriptions, and observes kickoff before releasing its ACK. It also resolves a replacement live ID through the durable family join. Producer-path cases reject waiting and later sends on prompt failure and closure during an in-flight ACK. These are bounded deterministic scheduling cases, not general sibling/external ordering coverage.

Use the task's build environment (Rust 1.98.1; shared target; dev/test debug=0, incremental=0, jobs=2). Run:

```sh
cargo test -p pa-daemon --lib prompt_admission
```

The reusable rule: when a handle is returned before detached initial work is admitted, later operations that assume that work exists must wait on a retained admission outcome; do not infer admission from task scheduling or a one-shot wake alone.

## Verification evidence

- RED: with the production admission wait removed, `cargo test -p pa-daemon --lib prompt_admission_orders_spawn_and_concurrent_replacement_followups` executed 1 test and exited 101: admission receiver count was 0 instead of 2. The barrier was then restored.
- GREEN: restored barrier; `cargo test -p pa-daemon --lib prompt_admission` exited 0, 7 passed (3 new regression tests and 4 existing tests). Touched-file rustfmt check and `git diff --check` passed.
- GREEN: all 7 selected tests passed, including the 3 new producer-path regressions.
- The recorded daemon verifier finished with 961 passing, 21 failing and 8 ignored tests. Failures included a stale shared Python runtime and long/symlinked macOS temporary socket paths; this failed historical outcome is preserved. External qualification uses a matching isolated runtime and `/private/tmp`. Workspace checks and runtime installation remain separate gates.
- Limits: close producer path is covered; delete uses the same retained rejection but has no separate new gated test. Direct transport is gated before ticket acquisition; successful delivery coverage takes the supervisor fallback. Existing ambiguous-prompt durable-file arbitration remains unchanged.
- Ownership: internal pa-daemon change only; no new public API or dependency, no cross-crate re-export. The crate-private family-alias seam binds replacement live IDs to the same durable registry record.
- `cargo fmt --all --check` passes after a separate module-order correction in `crates/pa-cli/src/lib.rs`.


## Parity-diff evidence

The actual installed TypeScript 0.9.8 CLI reproduces the same defect with an
in-process faux provider, real Python kernel, and isolated HOME/socket. The
same parent action (spawn, immediate message, then collect) yielded child
transcript rows 6 follow-up, 7 assistant, 8 initial task, 9 assistant. CLI exit
was zero and the root observed settled collection. This correction intentionally
diverges from that demonstrated defect; it does not claim identical behavior.
No external inference or real credentials were used.

Artifacts: `/private/tmp/ts-prompt-parity-ugrzsplq/parity-result.json`.
TS `dist/cli.js` SHA256:
`626e6d9115d06e944543a71a60bf3893248fbcf9b47fdfbeabcdb688e4803b8d`.
Adapter: `/private/tmp/ts-prompt-parity.py`.

## Ownership

Internal lifecycle correction within pa-daemon child supervision and messaging.
No new dependencies, cross-crate exports, wire schema, or public API. The live-ID
alias method is crate-private. The user-visible change is parent-to-own-child
message ordering; sibling and external sender ordering is outside this contract.


## Local macOS qualification prerequisites

Use a canonical short temporary directory (`TMPDIR=/private/tmp`) for AF_UNIX
fixtures and path equality. The live-kernel abort test resolves
`$HOME/.prime/agent/kernel-venv/bin/python` directly, so setting only
`PRIME_AGENT_KERNEL_PYTHON` is insufficient. Use a disposable HOME with that
path wrapping an already qualified interpreter, and set both
`PRIME_AGENT_KERNEL_PYTHON` and `PA_E2E_KERNEL_PYTHON` to that interpreter.
Verify the exact `RUNTIME_READY_CHECK` before running the suite. Keep explicit
`CARGO_HOME`, `RUSTUP_HOME`, and Rust 1.98.1 while isolating HOME; use actual
binary directories on PATH instead of HOME-dependent version-manager shims.
Never overwrite a shared runtime to satisfy tests or rewrite an earlier failed
workflow outcome after fixing these prerequisites.

The isolated interpreter must also import the matching built-in Python skills,
including `goal`; a `.pth` entry pointing at another site-packages directory does
not recursively process that directory's editable hooks. Package those skills
and verify their import origins as described in `substrate-local-install.md`.

Terminal fixtures need a consistent baseline: `TERM=xterm-256color`, with
`TERM_PROGRAM`, `KITTY_WINDOW_ID`, `GHOSTTY_RESOURCES_DIR`, `WEZTERM_PANE`, `TMUX`,
`STY`, `ZELLIJ`, `SSH_CONNECTION`, and `SSH_TTY` absent unless a case sets them.
Inherited `TERM=dumb` suppressed capability queries and caused false failures.
The legacy CLI differential fixture also requires GNU `timeout` and an explicit
`PA_TS_BINARY` plus its actual Node interpreter on PATH. A missing `timeout`
fails before either CLI starts. The installed TS 0.9.8 comparison subsequently
reported 28 of 199 corpus differences, including stdout/stderr routing and
changed options; this is not a passing full-CLI parity result.

On a storage-constrained host, retain the workspace dependency feature set when
selecting tests (`cargo test --workspace --test <target>`). Switching to a
single-package build can duplicate native libraries. After ENOSPC, verify test
artifacts are executable Mach-O files: rust-objcopy can leave a small data file
while Cargo reports a stripping warning. Never count an unexecuted target as a
passing test or increase timeouts to conceal failed process observation.


## Candidate end-to-end result

The built native candidate passed all six real offline daemon/kernel cases:
`followup-child`, `child`, `sync-child`, `retained-child`, `allowed`, `denied`.
The new ordering case accounted five provider calls and its persisted child
transcript satisfied initial task before follow-up and first assistant response.
Receipts correlated to traces; denied admission made no provider call.
Receipt and binary digest: `/private/tmp/native-canary-c2lxoyhm/qualification.json`.
This qualifies the built candidate's exercised paths; it does not replace the
full workspace gates or claim installation.

## Follow-on qualification: socket close and test isolation

The CLI now configures its fixed read poll timeout once at connection creation.
On Darwin, resetting it after peer closure can raise EINVAL before buffered
closing frames are consumed. The deterministic buffered-close regression fails
with the fix removed; all five CLI and thirteen TUI daemon-client tests pass
with it restored.

The TS daemon-command fixture supplies a deterministic local faux provider,
dummy credential and explicit model, including saved-session wake. All five
command cases pass. The fork fixture drains PTY output while awaiting exit;
both cases pass in 3.36 seconds. All sixteen supervisor integration cases pass.

The serial workspace attempt passed all 41 interactive-daemon cases, including
terminal close and streaming, but did not complete: 50 completed targets
reported 904 passes, eight failures and four ignores; two terminal executables
hung during child cleanup and were terminated before the overall run was
stopped. These are partial counts, not a passing merge gate. Remaining blockers
include inherited TS CLI differences, terminal lifecycle tests and a packaging
fixture that expects Linux paths on macOS. No release/default switch occurred.

All six substrate canaries passed again after these corrections, with verified
cleanup and zero provider calls for denied admission. Receipt:
 /private/tmp/native-canary-pp_ge3js/qualification.json.
Tested debug binary SHA-256:
 f6556068af41e4530d761511dbb25bc011ddc6b7b4909b529429094f2a169927.

Reusable fixture discipline: supply provider identity for success-path tests,
consume PTY output through exit, and check artifact executability after any
out-of-space stripping warning. Cargo can finish after producing an unusable
stripped artifact; do not classify the resulting launch errors as code failures.
