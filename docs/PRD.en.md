# Recode PRD (English)

- **Status:** Draft for internal review
- **Version:** v1.1
- **Last Updated:** 2026-06-12
- **Owner:** Kyungpyo

## 1. Overview

Recode is a lightweight, standalone, cross-platform coding-agent runtime with dual surfaces:
- a human-first TUI
- an automation-friendly CLI for OpenClaw

Its core value is visible, policy-driven execution with explicit retry, timeout, approval, audit, workflow, and scheduling boundaries.

## 2. Background and Objectives

### Vision
Build a coding runtime that feels sharp in the terminal, stays safe under enterprise expectations, ships as Linux and Windows binaries, and exposes a stable CLI contract so OpenClaw can call it directly during real coding workflows.

### Philosophy: The Meaning of RE
RE is the philosophy of disciplined return.

Recode is built for:
- **Re-plan** when reality invalidates the original path
- **Re-try** when failure is temporary but the objective still stands
- **Re-route** when the workflow must take another path
- **Re-sume** when interruption should not mean loss of progress
- **Re-code** when implementation must evolve under evidence

### Problem Statement
Existing coding-agent tools are often good at conversation but weak at execution control. In company environments, missing controls around retry, timeout, approvals, scheduling, and structured automation make them awkward or unsafe to use at scale.

### Target Users
1. **Operator-developers** who want visible execution control, auditability, and cross-platform behavior
2. **OpenClaw** as an automation caller through stable CLI commands

## 3. Success Metrics

### Primary KPIs
- Linux release binary produced successfully
- Windows release binary produced successfully
- 100% of core flows callable through CLI without TUI scraping
- Timeout control available at LLM, tool, step, and task boundaries
- Retry policy coverage for retry count, backoff, manual retry, and resume
- Workflow recovery correctness for branch/retry/resume scenarios

### Guardrails
- No destructive action path bypasses approval policy by default
- Crashes do not silently discard persisted session or scheduled-run state
- Routine CLI state queries respond within 2 seconds in a normal local environment
- Supported features behave consistently on both Linux and Windows

## 4. Scope and Priorities

### Must-Have
- Rust core runtime
- TUI and CLI surfaces
- Session/task/step/attempt model
- Dynamic workflow orchestration
- Timeout/retry controls
- Declarative skills
- OpenAI-compatible endpoint support
- Codex API support
- Tool approval gates
- Built-in cron and delayed scheduling
- Linux/Windows release binaries
- Auditable logs

### Should-Have
- Workflow templates per skill
- Richer operator steering commands
- Import/export of workflow state
- Release automation CI
- Structured failure taxonomy

### Could-Have
- Richer terminal visualizations
- Skill marketplace packaging
- Replay viewer

### Won't-Have in MVP
- Web UI
- Cloud control plane
- Arbitrary plugin SDK
- Distributed workers
- Complex memory engine

## 5. User Stories

- As an operator, I can run a coding workflow with visible retries and timeouts so I can trust and steer execution.
- As an operator, I can pause, redirect, retry, or resume a workflow so failure does not force a full restart.
- As an operator, I can schedule one-shot or recurring workflow runs.
- As OpenClaw, I can invoke Recode through stable CLI commands with JSON output.
- As a security-conscious user, I can require approvals for risky actions.

## 6. Functional Requirements

- Local session create/resume/inspect/persist
- Task execution with step and attempt state
- Dynamic branching, replanning, interruption, and resume
- One-shot, delayed, interval, and cron-expression scheduling
- LLM/tool/step/task timeout controls
- Retry count, backoff, manual retry, and resume semantics
- Tool execution with policy enforcement and approval gates
- Provider abstraction with OpenAI-compatible endpoints and Codex API support
- Declarative-first skills with limited hooks
- TUI live workflow visibility
- CLI subcommands with mandatory JSON output
- Auditable logs for transitions, approvals, retries, failures, and schedules

## 7. Workflow Orchestration Requirements

Recode should provide dynamic workflow control comparable in class to Claude Code style flows, but more inspectable and policy-visible.

Required capabilities:
- Dynamic replanning
- Conditional branching
- Operator intervention
- Approval-aware flow nodes
- Checkpoint and resume
- Skill-driven workflow templates
- Scheduled workflow launches
- Structured outcome routing

## 8. OpenClaw Integration Requirements

### CLI contract
- Stable versionable subcommands
- JSON output mode mandatory
- No TUI scraping
- Consistent exit codes and error classes
- CLI cron control for add/list/run/pause/resume/remove

### LLM API compatibility
- OpenAI-compatible endpoint support
- Codex API support
- Provider abstraction layer
- Policy continuity regardless of provider

## 9. Non-Functional Requirements

### Performance and Reliability
- Routine CLI state queries should respond within 2 seconds
- Scheduled jobs survive process restarts
- Crash recovery preserves the last stable checkpoint

### Platform Support
- Linux and Windows release binaries
- No tmux dependency
- No bash/POSIX-only dependency
- PTY/process/shell/cancel abstractions required
- Windows support is first-class

### Security
Security target should be comparable in class to OpenCode and Claude Code:
- default-safe local execution
- explicit approval gates
- tool allowlists and constrained invocation
- trusted instructions separated from external content
- auditable logs
- no hidden autonomous background behavior outside declared workflow

## 10. User Flow and Edge Cases

### Core Flow
1. User or OpenClaw starts a task or scheduled run
2. Runtime creates or resumes a session
3. Workflow plans subgoals and selects a step
4. Step executes with timeout/retry policy
5. Runtime continues, replans, blocks for approval, or fails visibly
6. User or OpenClaw steers until completion or explicit stop

### Key Edge Cases
- LLM timeout
- Tool non-zero exit
- Approval denied
- Empty state
- Invalid input
- Restart during scheduled wait
- User cancel / back out

## 11. Architecture Direction

Recommended direction:
- **Language:** Rust
- **TUI:** Ratatui
- **Async runtime:** Tokio
- **Execution model:** Session actor + Task graph + Step runner + Attempt policy
- **Workflow control:** dynamic branching, replanning, approval nodes, checkpoint/resume
- **LLM integration:** provider abstraction with OpenAI-compatible endpoint and Codex support
- **Skill model:** declarative-first + limited hooks
- **Scheduler:** built-in cron and delayed-run subsystem
- **Surfaces:** TUI frontend + CLI frontend over shared core

## 12. Release Plan

### MVP In
- Single runtime core
- TUI + CLI
- Session/task/step/attempt model
- Dynamic workflow orchestration
- Timeout and retry controls
- Declarative skills
- OpenAI-compatible and Codex support
- Approval-gated tool execution
- Built-in scheduling
- Linux/Windows binaries

### MVP Out
- Web UI
- Cloud control plane
- Distributed workers
- Arbitrary plugin SDK
- Complex memory engine

### Milestones
- PRD sign-off
- Runtime skeleton
- Workflow control
- Scheduling
- TUI integration
- Cross-platform release
- QA and hardening

## 13. Success Criteria

Recode MVP is successful if:
- Linux and Windows binaries are produced
- OpenClaw invokes Recode without scraping TUI output
- OpenAI-compatible and Codex endpoints work through one provider abstraction
- Cron and delayed runs are controllable and auditable from CLI
- Timeout/retry/workflow behavior is visible and consistent
- Risky actions respect approval boundaries
- Declarative skills work across both TUI and CLI
