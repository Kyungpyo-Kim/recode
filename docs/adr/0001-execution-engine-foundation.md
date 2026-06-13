# ADR 0001: Execution Engine Foundation

- Status: Accepted
- Date: 2026-06-13

## Context

Recode needs a first execution slice that is small, testable, and compatible with future retry, approval, scheduler, and TUI work.
The repo already has a workspace split and persisted session state, but no actual task execution behavior.

## Decision

Use a **persisted session aggregate** in `recode-core` and add a **deterministic single-step execution engine**.

Key parts:
- `SessionRecord` is the persisted root aggregate.
- Each session owns `TaskRecord`s.
- Each task owns ordered `StepRecord`s.
- Each step owns append-only `AttemptRecord`s.
- `WorkflowEngine::create_task(...)` appends a task to a session.
- `WorkflowEngine::run_next_step(...)` selects the next runnable step, executes it through a `StepRunner` trait, records the outcome, and persists the updated session.

## Why this path

### Chosen because
- Simple enough to verify with tight unit tests
- Natural fit for retry history and audit trails
- Keeps CLI and TUI thin, with shared logic in `recode-core`
- Makes future approval and scheduler layers orchestration concerns instead of core state hacks

### Explicit tradeoffs
- First cut is synchronous and local, not async or distributed
- Runner abstraction is deliberately narrow
- No full workflow planner yet, only ordered step execution

## Consequences

### Positive
- Clean foundation for retry, timeout, and approval policies
- Cross-platform friendly because behavior is pure Rust core logic
- Easy to expose through JSON CLI later

### Negative
- More advanced branching and orchestration still need another layer
- Current engine does not yet model pause, approval wait, or timeouts beyond status recording

## Follow-up

1. Add retry and timeout policy types
2. Add CLI commands for task creation and controlled step execution
3. Add approval-gated tool runner abstraction
4. Add scheduler integration on top of the same persisted aggregate
