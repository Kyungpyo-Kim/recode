# Recode

**RE the run.**

Recode is a cross-platform coding-agent runtime built for controlled iteration.
It is designed to **re-plan, re-try, re-route, re-sume, and re-code** with explicit workflow control, approval boundaries, scheduling, and automation-friendly CLI integration.

## What Recode aims to be

- Rust-based local runtime for coding workflows
- Interactive TUI for operators
- Stable CLI for automation, including OpenClaw integration
- Dynamic workflow orchestration comparable in class to Claude Code style flows
- Explicit timeout, retry, approval, audit, and cron control
- OpenAI-compatible endpoint support and Codex API support

## Core philosophy

**RE is the philosophy of disciplined return.**

Good systems do not assume they are right on the first pass.
They observe, adapt, recover, and continue.
Recode exists to make that loop explicit, governable, and trustworthy.

## Current status

The repo now has the first working MVP foundation:

- Cargo workspace split into `recode-core`, `recode-cli`, and `recode-tui`
- Shared `session / task / step / attempt` domain model
- Local JSON session persistence under `.recode/state`
- Deterministic execution engine in `recode-core`
- Minimal layered configuration system with file, env, and CLI precedence
- Retry and timeout policy persisted at session level
- Approval wait foundation in the core model/engine
- Shared `ExecutorBridge` used by both CLI and TUI
- Real timeout enforcement for shell-backed steps
- Shared execution options for streaming, PTY preference, and file-based cancellation
- CLI support for session creation, task creation, approval-gated step creation, controlled next-step execution, targeted task execution, step approval, and session-wide run-all
- TUI support for session browsing plus `run-next`, `run-all`, and `approve` actions
- ADR-based architecture decision records for engine, config, and policy foundation
- GitHub Actions CI for fmt, clippy, tests, and Linux/Windows build checks
- Tag-based release workflow for binary artifacts

## Architecture direction

### Core crates

- `recode-core`
  - shared domain model
  - layered config loader
  - persisted session aggregate
  - workflow execution engine
  - state storage
  - shared executor bridge
- `recode-cli`
  - automation-friendly JSON CLI
- `recode-tui`
  - Ratatui operator surface over the same session model

### Execution model

The current engine uses a persisted aggregate model:

- `SessionRecord` is the root persisted state
- each session owns an `ExecutionPolicy`
- each session owns `TaskRecord`s
- each task owns ordered `StepRecord`s
- each step keeps append-only `AttemptRecord`s

The engine currently supports:

- create a task with ordered steps
- create approval-gated steps that stop at a wait boundary
- approve a blocked step and make it runnable again
- select the next runnable step across the session
- execute only a targeted task by `task_id`
- run all remaining runnable steps in a session until blocked or complete
- persist attempt history and resulting task/session status
- retry a failed or timed out step while retry budget remains
- stop retrying once `max_attempts` is exhausted

### Shared executor bridge

CLI and TUI now share the same minimal executor path.

Current behavior:
- step titles prefixed with `cmd:`, `shell:`, or `exec:` run in the local shell
- shell-backed steps are killed and marked `timed_out` when they exceed `session.policy.timeout.step_timeout_secs`
- `--stream` inherits stdio for live command output in the CLI path
- `--pty` prefers a PTY-backed launch on Unix and falls back to the normal shell bridge if PTY launch is unavailable
- `--cancel-file <path>` cancels a running shell command once that file appears and records the attempt as `cancelled`
- non-prefixed steps are treated as explicit operator/no-op steps and succeed with a summary
- approval gates still stop execution before step run

Examples:

```bash
cargo run -p recode-cli -- session init --name timeout-demo --step-timeout-secs 1
cargo run -p recode-cli -- task create \
  --session-id <session_uuid> \
  --title "shell demo" \
  --step "cmd: sleep 2"
cargo run -p recode-cli -- task run-next --session-id <session_uuid>
cargo run -p recode-cli -- task run-next --session-id <session_uuid> --stream
cargo run -p recode-cli -- task run-next --session-id <session_uuid> --stream --pty
cargo run -p recode-cli -- task run-next --session-id <session_uuid> --cancel-file /tmp/recode.cancel
```

Manual injection mode still exists for testing:
- `--outcome success|failed|timeout|cancelled`
- optional `--summary "..."`

If `--outcome` is omitted, CLI uses the shared `ExecutorBridge`.

### TUI parity slice

The first real TUI now supports both visibility and basic actions.
It still runs through the same shared bridge, but this slice keeps live streaming and explicit cancellation controls CLI-first so the TUI does not fight its alternate-screen lifecycle yet.

Shown on screen:
- session list panel
- selected session detail panel
- task / step / attempt summary
- retry / timeout / approval policy summary
- approval-required and approval-granted step state

Keybindings:
- `↑` / `↓` or `j` / `k`: move selection
- `r`: refresh from disk
- `n`: run next step on selected session
- `A`: run all remaining runnable steps on selected session
- `a`: approve the first waiting approval step in selected session
- `q`: quit

For non-interactive checks:

```bash
cargo run -p recode-tui -- --dump
```

Opt out of default-session bootstrap:

```bash
cargo run -p recode-tui -- --no-bootstrap --dump
```

First-run behavior:
- if no sessions exist, TUI auto-creates a `default` session
- use `--no-bootstrap` if you want to inspect an intentionally empty state

### Configuration model

Recode supports a minimal layered config system with this precedence:

1. CLI flags
2. Environment variables
3. `recode.toml`
4. Built-in defaults

Current config surface:
- `state_dir`
- `log_level`
- `default_provider`
- `default_timeout_secs`
- `default_max_attempts`
- `approval_policy`

See:
- [ADR 0001: Execution Engine Foundation](docs/adr/0001-execution-engine-foundation.md)
- [ADR 0002: Minimal Configuration System](docs/adr/0002-minimal-configuration-system.md)
- [ADR 0003: Retry and Timeout Policy Foundation](docs/adr/0003-retry-timeout-policy.md)
- [PRD HTML](docs/Recode-PRD.html)
- [English PRD](docs/PRD.en.md)
- [한국어 PRD](docs/PRD.ko.md)

## Quality gates

Local checks:

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## Next steps

- deepen PTY support beyond the current Unix `script` fallback and add richer streaming capture for TUI/log panes
- add true asynchronous cancellation controls inside the TUI instead of CLI-first file signalling
- add task/step cursoring instead of first waiting-step approval only
- add backoff and richer retry policy types
- build approval policy `on_failure` into real differentiated behavior
