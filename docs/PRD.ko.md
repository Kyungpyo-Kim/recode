# Recode PRD (한국어)

- **상태:** 내부 검토용 초안
- **버전:** v1.1
- **최종 수정일:** 2026-06-12
- **오너:** Kyungpyo

## 1. 개요

Recode는 두 개의 표면을 가진 경량 독립형 크로스플랫폼 코딩 에이전트 런타임이다.
- 사람용 인터랙티브 TUI
- OpenClaw 자동화를 위한 CLI

핵심 가치는 단순한 대화 품질이 아니라, **재시도, 타임아웃, 승인, 감사, 워크플로우, 스케줄링 경계가 명시된 정책 기반 실행 제어**다.

## 2. 배경 및 목적

### 비전
터미널에서 날카롭게 동작하고, 기업 환경의 안전성 요구를 만족하며, Linux/Windows 바이너리로 배포되고, OpenClaw가 실제 코딩 워크플로우에서 직접 호출할 수 있는 안정적인 CLI 계약을 가진 코딩 런타임을 만든다.

### 철학: RE의 의미
RE는 **통제된 복귀의 철학**이다.

Recode는 다음을 위해 존재한다.
- **Re-plan**: 현실이 원래 경로를 무효화할 때 다시 계획한다
- **Re-try**: 실패가 일시적이지만 목표는 유효할 때 다시 시도한다
- **Re-route**: 워크플로우가 다른 경로를 타야 할 때 방향을 바꾼다
- **Re-sume**: 중단이 곧 진행 손실이 되지 않게 한다
- **Re-code**: 증거에 따라 구현 자체를 다시 바꾼다

### 문제 정의
기존 코딩 에이전트 도구는 대화는 잘하지만 실행 제어는 약한 경우가 많다. 특히 회사 환경에서는 재시도, 타임아웃, 승인, 스케줄링, 구조화된 자동화 제어가 부족하면 실제 업무에 쓰기 불편하거나 위험하다.

### 타겟 유저
1. **운영자형 개발자**: 실행 상태를 보고, 조향하고, 감사 가능성을 확보하고 싶은 사용자
2. **OpenClaw**: 안정적인 CLI 호출자로서 Recode를 도구처럼 쓰는 자동화 런타임

## 3. 성공 지표

### 핵심 KPI
- Linux 릴리즈 바이너리 생성 성공
- Windows 릴리즈 바이너리 생성 성공
- 핵심 플로우 100%를 TUI 스크래핑 없이 CLI에서 호출 가능
- LLM, tool, step, task 경계 모두 타임아웃 제어 가능
- 재시도 횟수, backoff, 수동 재시도, resume 지원
- branch/retry/resume 시나리오에서 상태 복구 정확성 확보

### 가드레일
- 파괴적 동작은 기본적으로 승인 정책을 우회하지 않아야 함
- 크래시가 persisted session/scheduled-run state를 조용히 버리면 안 됨
- 일반적인 로컬 환경에서 CLI 상태 조회는 2초 이내 응답
- 지원한다고 표시한 기능은 Linux/Windows 모두에서 일관되게 동작해야 함

## 4. 범위 및 우선순위

### Must-Have
- Rust 코어 런타임
- TUI + CLI
- Session/task/step/attempt 모델
- 동적 workflow orchestration
- timeout/retry 제어
- 선언형 skill
- OpenAI 호환 endpoint 지원
- Codex API 지원
- tool approval gate
- 내장 cron 및 delayed scheduling
- Linux/Windows 바이너리
- 감사 가능한 로그

### Should-Have
- skill별 workflow template
- 더 풍부한 operator steering command
- workflow state import/export
- release automation CI
- 구조화된 failure taxonomy

### Could-Have
- 더 강한 terminal visualization
- skill marketplace packaging
- replay viewer

### Won’t-Have (MVP)
- Web UI
- Cloud control plane
- Arbitrary plugin SDK
- Distributed workers
- Complex memory engine

## 5. 사용자 스토리

- 사용자는 재시도와 타임아웃이 보이는 코딩 워크플로우를 실행할 수 있어야 한다.
- 사용자는 실패 시 workflow를 pause, redirect, retry, resume 할 수 있어야 한다.
- 사용자는 one-shot 또는 recurring workflow run을 스케줄링할 수 있어야 한다.
- OpenClaw는 JSON 출력이 가능한 안정적인 CLI로 Recode를 호출할 수 있어야 한다.
- 보안 민감 사용자는 위험한 동작에 approval을 강제할 수 있어야 한다.

## 6. 기능 요구사항

- 로컬 session 생성 / 재개 / 조회 / 영속화
- task execution과 step/attempt 상태 관리
- 동적 branch, replan, interruption, resume
- one-shot, delayed, interval, cron-expression 스케줄링
- LLM/tool/step/task timeout 제어
- retry count, backoff, manual retry, resume semantics
- policy enforcement와 approval gate가 있는 tool execution
- OpenAI-compatible endpoint와 Codex API를 지원하는 provider abstraction
- declarative-first skill + limited hooks
- live workflow state를 보여주는 TUI
- JSON 출력이 가능한 CLI subcommand
- transition, approval, retry, failure, schedule에 대한 audit log

## 7. 워크플로우 오케스트레이션 요구사항

Recode는 Claude Code급의 동적 workflow 감각을 제공하되, 더 inspectable 하고 policy-visible 해야 한다.

필수 능력:
- Dynamic replanning
- Conditional branching
- Operator intervention
- Approval-aware flow node
- Checkpoint and resume
- Skill-driven workflow template
- Scheduled workflow launch
- Structured outcome routing

## 8. OpenClaw 연동 요구사항

### CLI 계약
- 안정적이고 버전 관리 가능한 subcommand
- JSON 출력 필수
- TUI 스크래핑 금지
- 일관된 exit code와 error class
- CLI 기반 cron add/list/run/pause/resume/remove 제어

### LLM API 호환성
- OpenAI-compatible endpoint 지원
- Codex API 지원
- Provider abstraction layer
- Provider가 바뀌어도 policy continuity 유지

## 9. 비기능 요구사항

### 성능 및 신뢰성
- 일반적인 CLI 상태 조회는 2초 이내 응답
- scheduled job은 프로세스 재시작 후에도 유지
- crash recovery 시 마지막 stable checkpoint를 보존

### 플랫폼 지원
- Linux/Windows 릴리즈 바이너리 제공
- tmux 의존 금지
- bash/POSIX-only 의존 금지
- PTY/process/shell/cancel abstraction 필수
- Windows 지원은 first-class여야 함

### 보안
OpenCode, Claude Code와 같은 등급의 보안 수준을 목표로 한다.
- default-safe local execution
- explicit approval gate
- tool allowlist와 constrained invocation
- trusted instruction과 external content 분리
- audit log
- 선언되지 않은 hidden autonomous background behavior 금지

## 10. 사용자 흐름 및 엣지 케이스

### 핵심 흐름
1. 사용자 또는 OpenClaw가 task 또는 scheduled run 시작
2. runtime이 session 생성 또는 재개
3. workflow가 subgoal을 계획하고 첫 step 선택
4. step이 timeout/retry 정책 아래 실행
5. runtime이 continue, replan, approval block, fail 중 하나를 명시적으로 처리
6. 사용자 또는 OpenClaw가 steer 하며 완료 또는 명시적 종료까지 진행

### 주요 엣지 케이스
- LLM timeout
- Tool non-zero exit
- Approval denied
- Empty state
- Invalid input
- Scheduled wait 중 process restart
- User cancel / back out

## 11. 아키텍처 방향

권장 방향:
- **Language:** Rust
- **TUI:** Ratatui
- **Async runtime:** Tokio
- **Execution model:** Session actor + Task graph + Step runner + Attempt policy
- **Workflow control:** dynamic branching, replanning, approval node, checkpoint/resume
- **LLM integration:** OpenAI-compatible endpoint와 Codex를 포함한 provider abstraction
- **Skill model:** declarative-first + limited hooks
- **Scheduler:** built-in cron / delayed-run subsystem
- **Surfaces:** shared core 위의 TUI frontend + CLI frontend

## 12. 릴리즈 계획

### MVP 포함
- 단일 runtime core
- TUI + CLI
- Session/task/step/attempt 모델
- Dynamic workflow orchestration
- Timeout/retry control
- Declarative skill
- OpenAI-compatible endpoint / Codex support
- Approval-gated tool execution
- Built-in scheduling
- Linux/Windows 바이너리

### MVP 제외
- Web UI
- Cloud control plane
- Distributed workers
- Arbitrary plugin SDK
- Complex memory engine

### 마일스톤
- PRD 승인
- Runtime skeleton
- Workflow control
- Scheduling
- TUI integration
- Cross-platform release
- QA and hardening

## 13. 성공 기준

다음이 충족되면 Recode MVP는 성공으로 본다.
- Linux/Windows 바이너리 생성 성공
- OpenClaw가 TUI 스크래핑 없이 CLI로 Recode 호출 가능
- OpenAI-compatible endpoint와 Codex API가 하나의 provider abstraction 위에서 동작
- Cron/delayed run을 CLI에서 생성, 조회, 실행, 감사 가능
- Timeout/retry/workflow 동작이 가시적이고 일관됨
- 위험한 동작이 approval boundary를 존중함
- Declarative skill이 TUI와 CLI 양쪽에서 일관되게 동작함
