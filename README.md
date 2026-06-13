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
- Approval wait foundation in the core model/engine, including `on_failure` pause-and-approve semantics
- Shared `ExecutorBridge` used by both CLI and TUI
- Real timeout enforcement for shell-backed steps
- Shared execution options for streaming, PTY preference, and file-based cancellation
- CLI support for session creation, task creation, approval-gated step creation, controlled next-step execution, targeted task execution, step approval, session-wide run-all, run listing/inspection/reconcile/cancel, and background execution
- TUI support for session browsing, task/step cursoring, selected-step approval, background execution, reconcile flow, selected-step log tail, selected status banner, and selected-run cancel requests
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
- each executed attempt can now point at a persisted `RunRecord` with pid/log-path metadata

The engine currently supports:

- create a task with ordered steps
- create approval-gated steps that stop at a wait boundary
- approve a blocked step and make it runnable again
- pause on failed/timed_out/cancelled attempts when `approval_policy=on_failure`
- resume an `on_failure` blocked step after operator approval
- select the next runnable step across the session
- execute only a targeted task by `task_id`
- run all remaining runnable steps in a session until blocked or complete
- persist attempt history and resulting task/session status
- retry a failed or timed out step while retry budget remains
- stop retrying once `max_attempts` is exhausted unless an operator explicitly re-approves continuation

### Shared executor bridge

CLI and TUI now share the same minimal executor path.

Current behavior:
- persisted steps now carry an explicit `kind` plus optional executor payload, instead of relying on title-prefix routing at runtime
- current CLI compatibility still accepts legacy `cmd:`, `shell:`, or `exec:` step text and normalizes it into explicit shell-step records on write
- a minimal `llm_chat` executor now exists for explicit chat steps and uses the configured provider/model surface for one-shot OpenAI-compatible requests
- shell-backed steps are killed and marked `timed_out` when they exceed `session.policy.timeout.step_timeout_secs`
- `--stream` inherits stdio for live command output in the CLI path
- `--pty` prefers a PTY-backed launch on Unix and falls back to the normal shell bridge if PTY launch is unavailable
- `--cancel-file <path>` cancels a running shell command once that file appears and records the attempt as `cancelled`
- `--background` launches a shell-backed step without blocking the caller and records the attempt/run as `running`
- executed steps now persist a `RunRecord` under `.recode/state/runs` and write stdout/stderr log files under `.recode/state/logs`
- the core store also reserves cancel request files under `.recode/state/cancels`
- non-prefixed steps are treated as explicit operator/no-op steps and succeed with a summary
- approval gates still stop execution before step run

Examples:

```bash
cargo run -p recode-cli -- session init --name timeout-demo --step-timeout-secs 1
cargo run -p recode-cli -- task create \
  --session-id <session_uuid> \
  --title "shell demo" \
  --shell-step "sleep::sleep 2"
cargo run -p recode-cli -- task create \
  --session-id <session_uuid> \
  --title "chat demo" \
  --chat-step "ask::Summarize the Rust ownership model in 3 bullets"
cargo run -p recode-cli -- task run-next --session-id <session_uuid>
cargo run -p recode-cli -- task run-next --session-id <session_uuid> --stream
cargo run -p recode-cli -- task run-next --session-id <session_uuid> --stream --pty
cargo run -p recode-cli -- task run-next --session-id <session_uuid> --cancel-file /tmp/recode.cancel
cargo run -p recode-cli -- task run-next --session-id <session_uuid> --background
cargo run -p recode-cli -- run list
cargo run -p recode-cli -- run inspect --id <run_uuid>
cargo run -p recode-cli -- run reconcile --id <run_uuid>
cargo run -p recode-cli -- run cancel --id <run_uuid>
```

Background lifecycle foundation now supports:
- launch a step into `running`
- persist stdout/stderr log paths plus an exit-code file path
- reconcile a finished background run back into attempt/task/session state
- trigger reconcile from CLI (`run reconcile`) or from TUI refresh (`r`)

Current `run cancel` is a lifecycle foundation, not full async process control yet:
- it writes a persisted cancel request file for the run
- it aligns with the existing file-based cancellation path
- TUI `x` now issues the same cancel request for the selected running run
- full in-flight operator control still needs a later async runtime slice

Manual injection mode still exists for testing:
- `--outcome success|failed|timeout|cancelled`
- optional `--summary "..."`

If `--outcome` is omitted, CLI uses the shared `ExecutorBridge`.

### TUI parity slice

The current TUI now supports both visibility and operator steering on top of the shared bridge.
It still avoids true live streaming and hard in-flight process control inside the alternate screen, but it now covers the main MVP operator loop for selection, execution, approval, background reconcile, output inspection, and cancel-request flow.
For `llm_chat` steps it now prefers a chat-first layout: transcript first, composer second, and task/run context alongside the log tail, with pane focus and practical vertical scrolling for longer conversations.

Shown on screen:
- session list panel
- selected session/task/step/run status banner
- chat-first `llm_chat` detail view with transcript, composer, context, and log tail
- default non-chat detail view with session/task/step summary plus logs/transcript
- task / step / attempt summary
- retry / timeout / approval policy summary
- approval-required and approval-granted step state

Keybindings:
- `↑` / `↓` or `j` / `k`: move session selection
- `←` / `→` or `h` / `l`: move task selection inside the selected session
- `u` / `d`: move step selection inside the selected task
- `Tab` / `Shift+Tab`: rotate focused pane in the chat-first view (transcript, composer, context, log)
- `PgUp` / `PgDn`: scroll the focused pane
- `Home` / `End`: jump the focused pane to the top or bottom
- `r`: reconcile finished background runs, then refresh from disk
- `n`: run next step on selected session
- `b`: run next step in background on selected session
- `A`: run all remaining runnable steps on selected session
- `a`: approve the selected waiting step
- `x`: request cancel for the selected running run
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
- `provider_mode`
- `provider_base_url`
- `provider_api_key_env`
- `provider_model`
- `default_timeout_secs`
- `default_max_attempts`
- `approval_policy` (`manual`, `on_failure`, `never`)

Current LLM executor scope:
- `llm_chat` uses one blocking OpenAI-compatible `POST /chat/completions`
- request/response text is written into the run stdout/stderr logs
- request/response/transcript JSON artifacts are also persisted under the state dir per run
- each new `llm_chat` attempt reloads the previous transcript for that step and appends the new user message, so step-level multi-turn chat now works
- persisted transcript keeps full history, but outbound API context is trimmed to a recent message window before each request
- `llm_chat` now supports minimal CLI streaming when execution runs with `--stream`, using OpenAI-compatible SSE deltas through the shared executor
- TUI now does minimal live redraw for streaming `llm_chat` runs by polling persisted run/log state while a foreground chat worker is in flight
- when the selected step is `llm_chat`, the alternate-screen layout becomes chat-first so the transcript and next prompt stay in the primary reading area
- transcript pane now reflects in-flight assistant streaming directly from the persisted transcript artifact, while the log tail still mirrors raw streamed output
- background mode is still not implemented for `llm_chat`
- CLI can now create explicit `--chat-step` records, and TUI now shows persisted chat transcript artifacts for the selected `llm_chat` step
- TUI also supports minimal prompt editing for the selected `llm_chat` step: `e` to edit, `Enter` to save and run, `Esc` to cancel
- chat-first panes keep independent scroll offsets, and the focused pane gets a highlighted border so long transcripts and prompts remain navigable without changing the session/task/step cursor
- CLI run results and TUI selected-run detail now show provider/model/token usage when the backend returns `usage`

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

- deepen PTY support beyond the current Unix `script` fallback and improve the in-flight TUI streaming UX beyond the current file-polling redraw
- deepen the chat UX beyond the current pane-focus + vertical-scroll model, especially transcript-aware composer behaviors and more granular in-pane navigation
- add true async runtime/process control so TUI cancel is not only request + reconcile
- Phase C is complete: CLI/TUI now surface provider/model/token-usage summary where the LLM response includes usage metadata
- add backoff and richer retry policy types
- build approval policy `on_failure` into real differentiated behavior
