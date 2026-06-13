# ADR 0003: Retry and Timeout Policy Foundation

- Status: Accepted
- Date: 2026-06-13

## Context

Recode can now create sessions and tasks and execute steps, but execution behavior is still driven entirely by one-off CLI outcomes.
Without a first-class policy model, retry behavior and timeout handling will stay implicit, brittle, and hard to audit.

## Decision

Add a minimal policy model for step execution with two concerns:
- retry limits
- timeout defaults

Initial shape:
- session-level `ExecutionPolicy`
- `RetryPolicy` with `max_attempts`
- `TimeoutPolicy` with `step_timeout_secs`

Behavior rules:
- a failed or timed out attempt is retryable while `attempt_count < max_attempts`
- once the limit is reached, the step becomes terminal failed
- a timed out attempt is stored as `timed_out` and follows the same retry gate as failure
- successful attempts complete the step immediately

Configuration rules:
- config provides default retry and timeout values
- CLI can override policy at session creation time

## Why this path

### Chosen because
- keeps policy explicit in persisted state
- gives audit visibility for why a step retried or stopped
- avoids hiding retry semantics inside runners or shell loops
- creates a stable place for later backoff and approval escalation logic

### Tradeoffs
- no backoff/jitter yet
- no per-task or per-step override yet
- timeout is modeled as policy and outcome state, not wall-clock enforcement

## Consequences

### Positive
- retry behavior becomes deterministic and inspectable
- timeout defaults stop leaking into ad hoc caller logic
- future scheduler and approval work can branch from shared policy state

### Negative
- policy may need another refinement when real async tool execution lands
- timeout enforcement still needs a real runner/executor later

## Follow-up

1. Add backoff policy after base retry flow proves stable
2. Support per-task and per-step overrides
3. Add real wall-clock timeout enforcement in the executor layer
