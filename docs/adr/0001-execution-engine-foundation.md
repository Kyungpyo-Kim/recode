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
- The engine still lacks a true async actor/runtime model for in-flight control

## Follow-up

Completed on top of this foundation:
1. Added retry and timeout policy types
2. Added CLI commands for task creation and controlled step execution
3. Added approval-gated execution flow in the shared engine/executor path
4. Added persisted run records, background execution, and reconcile flow

Still open:
1. Add scheduler integration on top of the same persisted aggregate
2. Add explicit workflow planner / branching layer above ordered step execution
3. Introduce true async process lifecycle control for TUI/operator steering
