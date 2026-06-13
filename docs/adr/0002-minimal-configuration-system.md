# ADR 0002: Minimal Configuration System

- Status: Accepted
- Date: 2026-06-13

## Context

Recode is about to add provider settings, retry and timeout defaults, approval policy, logging, and scheduler behavior.
Without a configuration system, those concerns will leak into CLI flags and ad hoc defaults, which gets ugly fast.
At the same time, the project is still early. A heavy configuration platform now would slow down core runtime work.

## Decision

Add a **minimal layered configuration system** now.

Initial source precedence:
1. CLI flags
2. Environment variables
3. `recode.toml`
4. Built-in defaults

Initial config surface:
- `state_dir`
- `log_level`
- `default_provider`
- `default_timeout_secs`
- `approval_policy`

Implementation choice:
- keep config logic in `recode-core::config`
- use a typed `RecodeConfig` for effective values
- use a partial `PartialConfig` for file, env, and CLI overlays
- support a root `recode.toml` in the project directory, plus explicit `--config <path>` override

## Why this path

### Chosen because
- Gives the runtime one stable place for defaults and overrides
- Keeps CLI thin and automation-friendly
- Avoids premature profile, secret, or enterprise policy complexity
- Makes later provider and scheduler work much cleaner

### Tradeoffs
- No profile system yet
- No dynamic reload
- No multi-file layering beyond one file
- No secret helper abstraction yet

## Consequences

### Positive
- Future features have a clear home for configuration
- Precedence is explicit and testable
- OpenClaw and operators get predictable override behavior

### Negative
- Some near-term config fields may move as the model evolves
- A later refactor may split config into its own crate if scope grows

## Follow-up

Completed since this ADR:
1. Added `default_max_attempts` alongside timeout/provider defaults
2. Wired the shared CLI/TUI surfaces through the same config loader path

Still open:
1. Add richer task/execution defaults as the step spec model expands
2. Add provider-specific nested config once provider abstraction lands
3. Consider profiles and workspace/user layering only after the base model proves stable
