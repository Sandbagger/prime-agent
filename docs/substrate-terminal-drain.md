# Flush terminal events before worker exit

A worker's shutdown response previously waited only for the requesting
supervisor connection. Direct TUI clients received events on independent
fan-out tasks. The process could exit after the supervisor's response flushed
but before those clients received turn cancellation and `session_closed`,
leaving a generic uncertain-connection error instead of the terminal outcome.

The worker now records weak references to its connection sinks. After closing
the listener, the terminal exit path captures the event sequence and waits for
all live sinks to reach it or close. One aggregate one-second deadline bounds
the entire drain. An unresponsive peer cannot hold shutdown indefinitely; a
timeout reports incomplete flushing and proceeds. This is bounded delivery,
not an assertion that a remote application consumed the frames. Normal prompt
acknowledgments and startup do not wait for this drain.

## Verification

The actual isolated killed-turn TUI regression failed in 62.78 seconds before
the correction and passed in 2.70 seconds afterward. The shutdown/replacement
close-reason regression failed in 60.23 seconds before the correction and
passed in 2.07 seconds afterward. No UI assertion or timeout was relaxed.
Logs: `/private/tmp/tui-killed-exact.log`,
`/private/tmp/tui-close-reason-exact.log`,
`/private/tmp/prime-terminal-drain-killed.log`, and
`/private/tmp/prime-terminal-drain-close-reason.log`.

Three deterministic paused-clock unit cases cover a flushed supervisor with
an unflushed direct peer, closed/dropped peers, and multiple blocked peers
sharing one deadline. The pending case explicitly polls the future before
releasing the peer, rather than assuming a spawned task ran after a yield.
`/private/tmp/prime-terminal-drain-unit-explicit.log` records all three passing.

This is internal pa-daemon lifecycle/transport behavior: no public API, wire
schema, dependency, or cross-crate export changes. The weak registry does not
keep disconnected peers alive. Full workspace gates remain separate.

The reusable rule: broadcasting a terminal event and flushing the requester's
response does not prove independent subscribers received that event. Before
process exit, drain the relevant fan-out sinks with one bounded deadline and
report incomplete delivery honestly.
