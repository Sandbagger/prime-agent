# TypeScript 0.9.8 and native 0.10 CLI contracts

The differential battery executes both binaries. Unchanged cases still compare
exit code, stdout and stderr directly after installation-path and version
normalization. The versioned cases independently assert **exact expected output
for each executable**, including exit code and output channel. A difference
alone never passes. The test first requires the TS reference to report exactly
0.9.8 through its historical stderr channel; a native binary or another TS
version cannot silently become the reference.

## Reference and provenance

The former `~/prime-agent` reference checkout is absent on this machine. The
historical source is nevertheless pinned: [native port commit
39bc99a](https://github.com/PrimeIntellect-ai/prime-agent/commit/39bc99a91d102c46474c090fdc7ae7cd5037ffcf)
has the sole parent
[`7d442aa`](https://github.com/PrimeIntellect-ai/prime-agent/commit/7d442aafa985f9342134fac16c2ef41f03fb45c1),
the v0.9.8 TS release recorded in `legacy-upgrade-contract.md`. The installed
0.9.8 bundle and unbundled entrypoint agree on the observed output channels.
No alternate reference was selected to make the battery green.

Native subsequently changed its interface deliberately:

- [PR 3189, f54551d](https://github.com/PrimeIntellect-ai/prime-agent/commit/f54551d9933c8d379972d42fd2168e0ff0cdf200)
  removed the TS/JS extension system and its CLI/help surface. Package update's
  legacy flag names do not reinstate the extension runtime.
- [PR 3211, a370028](https://github.com/PrimeIntellect-ai/prime-agent/commit/a3700282ca156a3c2bca1e3453012e369e783485)
  removed tool-selection flags that did nothing.
- [PR 3243, e55ea44](https://github.com/PrimeIntellect-ai/prime-agent/commit/e55ea44498b6ac1e0a85484e6661ef15b3a8410e)
  made updates follow the native release channel, and [PR 3278,
  ba9c6a9](https://github.com/PrimeIntellect-ai/prime-agent/commit/ba9c6a9ad2caf55ebc3b49f41c90f5cd2572012f)
  added native installer rollback/archive behavior. Native help and rejection
  guidance must describe that implementation rather than the old TS updater.
- Native help also exposes the shipped telemetry, factory and prompt commands.
  The full help fixture covers their exact descriptions and alignment; the old
  normalizer that removed the prompt row has been removed.

## Explicit native behavior decisions

The 0.10 substrate qualification preserves these concrete contracts. These are
explicit native decisions, not claims that TS behaved identically:

- Successful version/help/model-list/export output goes to stdout so shell
  consumers can use it. Diagnostics remain on stderr. TS 0.9.8 `main.ts` calls
  `takeOverStdout()` before processing several noninteractive commands; its
  historical stderr output is recorded separately, not merged with stdout.
- Positive integer errors state the actual Rust field bounds. Existing
  `positive_int_rejects_zero_and_text` and `positive_int_rejects_past_the_field_range`
  unit tests already pin this behavior.
- The @file rejection describes RPC, daemon **and ACP**, all modes that reject
  it. The two tested RPC/daemon cases still fail on both implementations.
- `model list --json` fails as an unadvertised option. TS silently ignores it
  and prints the ordinary empty-model guidance; native must not pretend it
  emitted JSON.

`crates/pa-cli/tests/fixtures/cli-ts-0.9.8-native-0.10.json` contains the exact
paired observations for these 28 corpus cases plus export. Expectations were
captured from the failing real-binary run, then reviewed against the contracts
above. The independent pre-task c5 vs child-order candidate replay matched all
28 cases, establishing that they were inherited rather than caused by the
prompt-order fix. Receipts remain in
`/private/tmp/prime-cli-baseline-replay-5uav8cuu/receipt.json`.

Generated mutation checks alter stdout, stderr and exit code on **each side**
of every paired case, and alter argv so a contract cannot apply to another
command. They must reject every mutation. Contracted cases do not normalize
version numbers or combine output channels. The shared corpus, export data,
CSS and HTML checks remain active. This is a versioned executable contract,
not a mismatch exemption or a passing full-workspace result.
