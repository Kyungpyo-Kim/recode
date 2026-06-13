# Recode TODO

## Done so far
- [x] Define MVP bootstrap scope from PRD into a concrete repo skeleton
- [x] Create Rust workspace with core, CLI, and TUI crates
- [x] Add shared domain model for session, task, step, attempt, and status
- [x] Implement minimal JSON CLI commands for version and session init/inspect
- [x] Add ADR for execution-engine architecture decision
- [x] Implement execution engine with TDD-first coverage
- [x] Add GitHub Actions CI/CD workflows
- [x] Add minimal configuration system with ADR and precedence rules
- [x] Wire CLI to config resolution
- [x] Expose task creation through CLI
- [x] Expose run-next step execution through CLI
- [x] Add targeted task execution and session run-all
- [x] Bring CLI and TUI to the same functional milestone baseline
- [x] Add first real Ratatui session/task status screen
- [x] Surface approval/retry/timeout state in TUI
- [x] Replace the TUI built-in success stub with a shared executor bridge
- [x] Unify CLI and TUI on the shared executor bridge
- [x] Add real timeout enforcement for shell-backed steps
- [x] Add minimal shared executor options for streaming, PTY preference, and cancellation
- [x] Thread executor options through CLI and keep TUI on the same bridge
- [x] Bootstrap a default session on first TUI launch
- [x] Update README, ADRs, sample config, and tests across the completed slices
- [x] Verify fmt, clippy, and tests on completed slices

## Basic coding agent MVP, still needed

### 1) Non-blocking runtime lifecycle
- [ ] Add true async run lifecycle so TUI can cancel in-flight commands without blocking
- [x] Introduce persisted run records separate from attempt summaries, including pid/process metadata and lifecycle timestamps
- [x] Support foreground vs background execution mode at the runtime layer
- [x] Add cancel API at core level, not just cancel-file polling
- [x] Make session/task/step status refresh from live run state without requiring full rerun

### 2) Output capture and observability
- [ ] Persist streamed stdout/stderr into session logs instead of terminal-only inheritance
- [x] Add per-attempt log file paths and retention layout under state dir
- [x] Show recent live/output tail inside TUI detail pane
- [x] Add CLI commands to inspect attempt logs and run history
- [ ] Record exit code, signal, timeout reason, and cancellation reason in attempt metadata

### 3) TUI usability to reach "real operator console"
- [x] Add task cursoring inside selected session
- [x] Add step cursoring inside selected task
- [x] Let TUI approve the selected waiting step, not just the first waiting step
- [x] Add TUI keybindings for canceling active run
- [x] Add TUI pane for log/output tail
- [x] Add clear status banner for running, waiting approval, failed, timed out, cancelled
- [ ] Add first-run onboarding/help footer instead of raw empty state behavior

### 4) Execution semantics for coding-agent behavior
- [ ] Replace title-prefix routing with explicit step action/spec model, not just `cmd:` strings
- [ ] Add executor kinds such as shell, noop/operator, approval, plan, codegen, test, patch, and external-agent call
- [ ] Add working-directory override per step
- [ ] Add env override per step
- [ ] Add file/input attachments or references per step
- [ ] Add structured step result payloads beyond freeform summary text

### 5) Planning and agent loop primitives
- [ ] Add explicit plan -> execute -> verify -> retry loop primitives in core model
- [ ] Add step dependencies / DAG or at least guarded sequential prerequisites
- [ ] Add task templates for common coding-agent flows, such as inspect, edit, test, verify, summarize
- [ ] Add result-based branching, for example on success/failure/timeout/cancel
- [ ] Add resumable sessions with open-loop tracking after interruption or restart

### 6) Approval and safety model
- [ ] Implement real differentiated behavior for `approval_policy=on_failure`
- [ ] Add approval scopes, step-level vs task-level vs session-level
- [ ] Add approval reason/message payloads to blocked steps
- [ ] Add dangerous-command detection hooks before execution
- [ ] Add sandbox / allowlist policy layer for command execution
- [ ] Add audit trail for who approved, when, and why

### 7) Retry and recovery sophistication
- [ ] Add retry backoff policy types, fixed/exponential/manual
- [ ] Add retryable error classification instead of only status-based retry
- [ ] Add max runtime / max total attempts policy per task and session
- [ ] Add resume-from-last-good-step semantics after failure
- [ ] Add explicit rerun command for a chosen attempt or step

### 8) CLI surface needed for a usable coding agent
- [ ] Add command to create richer step specs without overloading repeated `--step` strings
- [ ] Add command to append steps/tasks into an existing session interactively or via JSON input
- [ ] Add commands for run inspect, run list, run cancel, log tail, and log export
- [ ] Add machine-friendly JSON schema docs for CLI input/output contracts
- [ ] Add import/export of session state for automation and remote orchestration

### 9) Knowledge of workspace changes
- [ ] Track files touched by each attempt/run
- [ ] Add patch/diff capture into run metadata
- [ ] Surface git status summary per session in CLI/TUI
- [ ] Add verify step helpers for `cargo test`, `cargo clippy`, etc. as reusable step kinds
- [ ] Add rollback/recovery hints when a run leaves the workspace dirty

### 10) Toward actual coding-agent integrations
- [ ] Add adapter abstraction for external coding engines, not just local shell
- [ ] Add OpenAI-compatible / Codex request-backed step executor
- [ ] Add prompt/input packaging for code-edit tasks
- [ ] Add artifact capture for generated patches, plans, and summaries
- [ ] Add session memory/context window compaction strategy for long-lived coding sessions

### 11) Reliability and packaging
- [ ] Add integration tests that cover async run, timeout, cancel, approval, retry, and resume together
- [ ] Add golden tests for CLI JSON outputs
- [ ] Add TUI smoke tests for bootstrap and action flows where practical
- [ ] Add migration/versioning strategy for persisted state schema
- [ ] Add release artifacts and install docs for end users beyond cargo run

## Suggested near-term execution order
- [ ] Phase A: async run lifecycle + persisted logs + TUI cancel
  - [x] A1. Add persisted run records and log-path layout in recode-core
  - [x] A2. Thread run metadata through executor/engine attempt results
  - [x] A3. Add core cancel API backed by persisted run state
  - [x] A4. Expose basic run inspect/cancel surface in CLI
  - [x] A5. Update TUI/README/tests for the new lifecycle foundation
  - [x] A6. Add executor/runtime support for foreground vs background launch mode
  - [x] A7. Persist real stdout/stderr logs from executor paths
  - [x] A8. Add run-state reconcile API so running steps can advance without rerun
  - [x] A9. Wire CLI/TUI controls onto background runs and reconcile flow
- [ ] Phase B: task/step cursoring + TUI log pane + selected-step approval
  - [x] B1. Add task cursor inside selected session
  - [x] B2. Add step cursor inside selected task
  - [x] B3. Make approve act on selected waiting step
  - [x] B4. Reflect cursor state in TUI detail rendering and controls help
  - [x] B5. Add TUI log pane for selected step/run tail
  - [x] B6. Add clear status banner for selected session/task/step/run state
  - [x] B7. Add selected-run cancel request keybinding in TUI
- [ ] Phase C: explicit step action/spec model + richer CLI creation surface
- [ ] Phase D: `on_failure` approval semantics + retry backoff + resume model
- [ ] Phase E: external coding-engine adapters and real coding-agent task templates
