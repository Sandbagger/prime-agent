# Local substrate candidate installation

This is a local macOS qualification build of Prime Agent 0.10.0, not an upstream release. Native build source: `9b095eb`. Default Prime selection remains separately controlled by Dotfile.

Install side by side under `~/.local/opt/prime-agent-substrate/0.10.0-9b095eb/`, refusing an existing directory. Use APFS `cp -c` to clone `target/debug/prime-agent` into `libexec/prime-agent`, and `cp -cR ~/.prime/agent/kernel-venv <candidate>/runtime` to clone dependencies without modifying the original environment. Replace only the cloned `runtime/lib/python3.11/site-packages/rlm` directory with `prime-agent-runtime/src/rlm`, excluding `__pycache__`.

The candidate runtime depends on the already installed uv-managed CPython 3.11.15 interpreter referenced by its `bin/python` symlink and `pyvenv.cfg`. It does not depend on the mutable source checkout or original kernel venv packages. Console-script shebangs copied with the environment are not used; launch the runtime through its explicit `bin/python`.

Create a regular executable `prime-agent` shell launcher that exports `PRIME_AGENT_KERNEL_PYTHON=<absolute candidate>/runtime/bin/python` and `PYTHONDONTWRITEBYTECODE=1`, then executes `<absolute candidate>/libexec/prime-agent "$@"`. This pin is inherited by native child workers. The launcher neither supplies policy credentials nor selects a root identity.

Verify the native version and `--substrate-protocol-version`, run the exact `RUNTIME_READY_CHECK` from `crates/pa-core/src/kernel/bootstrap/venv/probe.rs`, and import requests, httpx, yaml, tomli, dotenv, pandas, numpy, scipy, bs4, lxml, pydantic and tyro. Assert `rlm.__file__` resolves inside the candidate and the `rlm.spawn` signature contains `route`.

Write a receipt containing build source commit, packaging source commit, native binary SHA256, launcher SHA256, and per-file SHA256 values for the copied Python runtime. Repeat the real offline policy/observer canary against the installed launcher, including distinct root/worker selectors, structured route admission, actual child usage settlement and eventual child grant release. A capability marker alone does not qualify the Python runtime or end-to-end child path. Keep default selection unchanged until its separate qualification decision.
