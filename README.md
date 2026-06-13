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
- ADR-based architecture decision record for the engine foundation
- TDD-first coverage for task creation, success, failure, and retry recovery
- GitHub Actions CI for fmt, clippy, tests, and Linux/Windows build checks
- Tag-based release workflow for binary artifacts

## Architecture direction

### Core crates

- `recode-core`
  - shared domain model
  - persisted session aggregate
  - workflow execution engine
  - state storage
- `recode-cli`
  - automation-friendly JSON CLI
- `recode-tui`
  - operator-facing TUI surface placeholder

### Execution model

The first engine slice uses a persisted aggregate model:

- `SessionRecord` is the root persisted state
- each session owns `TaskRecord`s
- each task owns ordered `StepRecord`s
- each step keeps append-only `AttemptRecord`s

The engine currently supports:

- create a task with ordered steps
- select the next runnable step
- execute via a `StepRunner` abstraction
- persist attempt history and resulting task/session status
- retry a failed step by running the same step again and appending a new attempt

See:
- [ADR 0001: Execution Engine Foundation](docs/adr/0001-execution-engine-foundation.md)
- [PRD HTML](docs/Recode-PRD.html)
- [English PRD](docs/PRD.en.md)
- [한국어 PRD](docs/PRD.ko.md)

## Repo layout

```text
.
├── .github/workflows/         # CI and release automation
├── crates/
│   ├── recode-core/           # model, engine, persistence
│   ├── recode-cli/            # JSON CLI surface
│   └── recode-tui/            # TUI surface placeholder
├── docs/
│   ├── adr/
│   │   └── 0001-execution-engine-foundation.md
│   ├── PRD.en.md
│   ├── PRD.ko.md
│   └── Recode-PRD.html
└── TODO.md                    # active implementation checklist
```

## CLI today

Currently implemented:

```bash
cargo run -p recode-cli -- version
cargo run -p recode-cli -- session init --name demo
cargo run -p recode-cli -- session inspect --id <uuid>
```

All CLI output is JSON-oriented so it stays automation-friendly.

## Quality gates

Local checks:

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

GitHub Actions:
- `ci.yml`: fmt, clippy, tests, Linux/Windows build verification
- `release.yml`: builds release binaries on `v*` tags and uploads artifacts

## Next steps

- expose task creation and step execution through CLI
- add retry and timeout policy types
- model approval-wait states and approval-gated execution
- add scheduler and cron subsystem
- build live TUI state panels and operator controls
