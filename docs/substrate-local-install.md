# Local substrate candidate installation

This procedure stages a local macOS build of Prime Agent 0.10.0. Default Prime selection remains separately controlled by Dotfile. Keep the old installation and default selection until the new candidate passes installed-runtime qualification.

## Build and provenance

Run the full repository merge gates (`make check`: formatting, all-target clippy, workspace tests, and release build) against the final source. Record the exact clean build commit, Rust toolchain, build features, and effective release-profile overrides. Use the qualified release binary from that build, including the actual `CARGO_TARGET_DIR` when overridden. Do not substitute an earlier debug canary binary. A binary hash binds bytes; it does not prove which source or compiler produced them, so retain the build evidence separately.

Install side by side under `~/.local/opt/prime-agent-substrate/0.10.0-<commit-prefix>/`, refusing an existing directory. Use APFS `cp -c` to clone the qualified release binary into `libexec/prime-agent`. Clone the Python environment from a known immutable previous candidate with `cp -cR <previous>/runtime <candidate>/runtime`; do not modify the previous environment or copy from an unqualified ambient kernel environment.

## Candidate-owned Python and skills

Replace only the cloned `runtime/lib/python3.11/site-packages/rlm` directory with tracked `prime-agent-runtime/src/rlm` files from the exact build commit. Copy tracked `skills/` files from that same commit into `<candidate>/skills`. Exclude untracked files, caches, bytecode, and symlinks. Reading the committed Git blobs makes the package independent of checkout changes during staging.

Rewrite all nine copied editable `.pth` files to absolute paths inside `<candidate>/skills/<skill>/src`:

| Editable package suffix (`_editable_impl_<suffix>.pth`) | Skill directory |
| --- | --- |
| `agent_message` | `agent-message` |
| `agent_observe` | `agent-observe` |
| `compact` | `compact` |
| `edit` | `edit` |
| `goal` | `goal` |
| `prime_agent_skill_attach_image` | `attach-image` |
| `prime_agent_skill_websearch` | `websearch` |
| `refine` | `refine` |
| `rlm_heartbeat` | `rlm-heartbeat` |

Reject unexpected editable hooks and review every other `.pth` hook. The existing `_virtualenv.pth` hook imports `_virtualenv`; it is not a skill path. Earlier candidates retained skill paths into the mutable TypeScript installation even though their `rlm` package was copied. Copying `rlm` alone therefore does not establish runtime independence.

The candidate still depends on the installed uv-managed CPython interpreter. Resolve its executable to the concrete versioned path, pin the cloned `bin/python` symlink to that resolved path, and pin `pyvenv.cfg` home to its parent directory. Record and verify the resolved executable path, SHA256, Python version and `pyvenv.cfg` SHA256. A mutable uv alias does not bind the interpreter used during qualification. Copied console-script shebangs are not used; launch Python through the candidate's explicit `runtime/bin/python`.

Create a regular executable `prime-agent` shell launcher exporting:

- `PRIME_AGENT_KERNEL_PYTHON=<absolute candidate>/runtime/bin/python`
- `PI_PACKAGE_DIR=<absolute candidate>`
- `PYTHONDONTWRITEBYTECODE=1`

Then execute `<absolute candidate>/libexec/prime-agent "$@"`. These pins are inherited by native child workers. The launcher supplies neither policy credentials nor a root identity.

## Installed qualification and selection

Verify the native version and `--substrate-protocol-version`, run the exact `RUNTIME_READY_CHECK` from the build commit's `crates/pa-core/src/kernel/bootstrap/venv/probe.rs`, and import requests, httpx, yaml, tomli, dotenv, pandas, numpy, scipy, bs4, lxml, pydantic and tyro. Assert `rlm.__file__` resolves inside the candidate and the `rlm.spawn` signature contains `route`. Check all nine editable hook targets and import each corresponding skill package, asserting its resolved module path is inside the candidate. Check the packaged skill tree against the committed file manifest; no skill import or hook may resolve through the source checkout or previous TypeScript installation.

Write a receipt containing build and packaging source commits, actual toolchain/profile/features, native binary SHA256, launcher SHA256, per-file hashes for the copied Python runtime and skills, and hashes of the rewritten editable hooks. The current Dotfile mise plugin validates the native binary, launcher and `rlm` manifest; qualification must also verify the skill and hook fields before selection.

Repeat the real offline policy/observer canaries against the installed launcher, including distinct root/worker selectors, structured route admission, actual child usage settlement, eventual child grant release, synchronous collection, immediate parent followup ordering, and terminal-child message rejection. Retain their receipts and cleanup evidence. A capability marker alone does not qualify the Python runtime or child path.

Only after these checks may Dotfile register the candidate as `0.10.0-substrate.<commit-prefix>` and update its paired mise and shell pins. Installation, qualification, and default selection are separate steps; a staged receipt does not claim qualification.
