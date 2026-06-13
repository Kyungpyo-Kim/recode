# Recode PRD (한국어)

- **상태:** 내부 검토용 초안
- **버전:** v1.1
- **최종 수정일:** 2026-06-13
- **오너:** Kyungpyo

> 구현 메모, 2026-06-13: 현재 저장소는 Rust workspace, persisted session/task/step/attempt 모델, 공유 CLI/TUI executor 경로, timeout enforcement, background run record, reconcile 흐름, task/step cursoring, TUI log tail, status banner, selected-run cancel request 흐름까지 포함한다. 내장 scheduler, 명시적 step spec 모델, richer retry/backoff, real async process control은 다음 마일스톤이다.

## 1. 개요

Recode는 사람용 TUI와 OpenClaw용 CLI를 함께 갖춘 경량 독립형 코딩 에이전트 런타임이다.
- 사람은 인터랙티브 TUI로 다룬다.
- OpenClaw는 CLI로 직접 호출한다.

핵심은 그럴듯한 대화가 아니다. **재시도, 타임아웃, 승인, 감사, 워크플로우, 스케줄링 경계를 드러내는 실행 제어**가 본체다.

## 2. 배경 및 목적

### 비전
터미널에서 매끄럽게 돌아가고, 기업 환경의 안전성 요구를 견디며, Linux와 Windows 바이너리로 배포되고, OpenClaw가 실제 코딩 흐름 안에서 바로 호출할 수 있는 안정적인 CLI 계약을 갖춘 런타임을 만든다.

### 철학: RE의 의미
RE는 **통제된 복귀**를 뜻한다.

좋은 시스템은 한 번에 맞히는 척하지 않는다. 상황을 보고, 틀리면 고치고, 막히면 우회하고, 끊겨도 다시 이어 간다. Recode는 그 반복을 숨기지 않고 드러내기 위해 만든다.
- **Re-plan**: 현실이 처음 계획을 무너뜨리면 다시 짠다.
- **Re-try**: 실패가 일시적이면 다시 시도한다.
- **Re-route**: 더 나은 길이 보이면 흐름을 바꾼다.
- **Re-sume**: 중단돼도 이어서 간다.
- **Re-code**: 증거에 맞춰 구현을 다시 고친다.

### 문제 정의
기존 코딩 에이전트 도구는 말은 그럴듯해도 실행 제어는 약한 경우가 많다. 특히 회사 환경에서는 재시도, 타임아웃, 승인, 스케줄링, 구조화된 자동화 제어가 빠지면 실제 업무에 붙이기 불편하고, 경우에 따라 위험하다.

### 타겟 유저
1. **운영자형 개발자**: 실행 상태를 눈으로 확인하고, 흐름을 직접 조정하고, 나중에 추적까지 하고 싶은 사용자
2. **OpenClaw**: Recode를 안정적인 CLI 도구처럼 호출해 쓰는 자동화 런타임

## 3. 성공 지표

### 핵심 KPI
- Linux 릴리즈 바이너리 생성 성공
- Windows 릴리즈 바이너리 생성 성공
- 핵심 플로우 100%를 TUI 스크래핑 없이 CLI에서 호출 가능
- LLM, tool, step, task 경계 모두 타임아웃 제어 가능
- 재시도 횟수, backoff, 수동 재시도, resume 지원
- branch/retry/resume 시나리오에서 상태 복구 정확성 확보

### 가드레일
- 파괴적 동작은 기본 승인 정책을 우회하면 안 된다.
- 크래시가 저장된 세션 상태나 예약 실행 상태를 조용히 버리면 안 된다.
- 일반적인 로컬 환경에서 CLI 상태 조회는 2초 안에 응답해야 한다.
- 지원한다고 적은 기능은 Linux와 Windows에서 모두 일관되게 돌아가야 한다.

## 4. 범위 및 우선순위

### Must-Have
- Rust 코어 런타임
- TUI + CLI
- 세션, 작업, 단계, 시도 모델
- 동적 워크플로우 오케스트레이션
- 타임아웃, 재시도 제어
- 선언형 스킬
- OpenAI 호환 엔드포인트 지원
- Codex API 지원
- 도구 실행 승인 게이트
- 내장 cron 및 지연 실행 스케줄링
- Linux/Windows 바이너리
- 감사 가능한 로그

### Should-Have
- 스킬별 워크플로우 템플릿
- 더 풍부한 운영자 조향 명령
- 워크플로우 상태 가져오기/내보내기
- 릴리즈 자동화 CI
- 구조화된 실패 분류 체계

### Could-Have
- 더 풍부한 터미널 시각화
- 스킬 배포 패키징
- 실행 이력 재생 뷰어

### Won’t-Have (MVP)
- Web UI
- 클라우드 제어면
- 임의 플러그인 SDK
- 분산 워커
- 복잡한 메모리 엔진

## 5. 사용자 스토리

- 사용자는 재시도와 타임아웃이 눈에 보이는 코딩 워크플로우를 실행할 수 있어야 한다.
- 사용자는 실패했을 때 워크플로우를 멈추고, 돌리고, 다시 시도하고, 이어서 실행할 수 있어야 한다.
- 사용자는 일회성 실행이나 반복 실행을 예약할 수 있어야 한다.
- OpenClaw는 JSON 출력이 가능한 안정적인 CLI로 Recode를 호출할 수 있어야 한다.
- 보안에 민감한 사용자는 위험한 동작에 반드시 승인을 걸 수 있어야 한다.

## 6. 기능 요구사항

- 로컬 세션 생성, 재개, 조회, 영속화
- 작업 실행과 단계/시도 상태 관리
- 동적 분기, 재계획, 중단, 재개
- 일회성, 지연, 주기, cron 표현식 스케줄링
- LLM/tool/step/task 단위 타임아웃 제어
- 재시도 횟수, backoff, 수동 재시도, 재개 규칙
- 정책 강제와 승인 게이트가 있는 도구 실행
- OpenAI 호환 엔드포인트와 Codex API를 지원하는 provider abstraction
- 선언형 우선 스킬 + 제한된 훅
- live workflow state를 보여주는 TUI
- JSON 출력을 지원하는 CLI subcommand
- 전이, 승인, 재시도, 실패, 스케줄 이력을 남기는 audit log

## 7. 워크플로우 오케스트레이션 요구사항

Recode는 Claude Code급의 동적인 흐름 제어를 제공하되, 내부 상태와 정책이 더 잘 보이도록 만들어야 한다.

필수 능력:
- 동적 재계획
- 조건부 분기
- 운영자 개입
- 승인 인지형 플로우 노드
- 체크포인트와 재개
- 스킬 기반 워크플로우 템플릿
- 예약 실행
- 구조화된 결과 라우팅

## 8. OpenClaw 연동 요구사항

### CLI 계약
- 안정적이고 버전 관리 가능한 subcommand
- JSON 출력 필수
- TUI 스크래핑 금지
- 일관된 종료 코드와 오류 분류
- CLI 기반 cron add/list/run/pause/resume/remove 제어

### LLM API 호환성
- OpenAI 호환 엔드포인트 지원
- Codex API 지원
- Provider abstraction layer
- provider가 바뀌어도 정책 일관성 유지

## 9. 비기능 요구사항

### 성능 및 신뢰성
- 일반적인 CLI 상태 조회는 2초 안에 응답해야 한다.
- 예약 실행은 프로세스가 다시 떠도 유지돼야 한다.
- 크래시 복구 시 마지막 안정 체크포인트를 보존해야 한다.

### 플랫폼 지원
- Linux/Windows 릴리즈 바이너리 제공
- tmux 의존 금지
- bash/POSIX 전용 가정 금지
- PTY/process/shell/cancel abstraction 필수
- Windows 지원은 처음부터 핵심 범위로 다뤄야 한다.

### 보안
OpenCode, Claude Code와 같은 급의 보안 수준을 목표로 한다.
- 기본 안전 실행 정책
- 명시적 승인 게이트
- 도구 허용 목록과 제한된 실행 정책
- 신뢰된 지시와 외부 콘텐츠 분리
- 감사 로그
- 문서화되지 않은 자율 백그라운드 동작 금지

## 10. 사용자 흐름 및 엣지 케이스

### 핵심 흐름
1. 사용자 또는 OpenClaw가 작업이나 예약 실행을 시작한다.
2. runtime이 세션을 만들거나 다시 연다.
3. workflow가 하위 목표를 잡고 첫 단계를 고른다.
4. 단계는 timeout/retry 정책 아래에서 실행된다.
5. runtime은 계속 진행, 재계획, 승인 대기, 실패 중 하나를 명시적으로 처리한다.
6. 사용자 또는 OpenClaw는 필요하면 흐름을 조정하면서 완료 또는 명시적 종료까지 가져간다.

### 주요 엣지 케이스
- LLM timeout
- Tool non-zero exit
- Approval denied
- Empty state
- Invalid input
- 예약 대기 중 프로세스 재시작
- 사용자 취소 또는 중도 이탈

## 11. 아키텍처 방향

권장 방향:
- **Language:** Rust
- **TUI:** Ratatui
- **Async runtime:** Tokio
- **Execution model:** Session actor + Task graph + Step runner + Attempt policy
- **Workflow control:** 동적 분기, 재계획, 승인 노드, 체크포인트/재개
- **LLM integration:** OpenAI 호환 엔드포인트와 Codex를 포함한 provider abstraction
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
- QA 및 안정화

## 13. 성공 기준

다음이 충족되면 Recode MVP는 성공으로 본다.
- Linux/Windows 바이너리 생성 성공
- OpenClaw가 TUI를 긁지 않고 CLI로 Recode를 호출할 수 있음
- OpenAI 호환 엔드포인트와 Codex API가 하나의 provider abstraction 위에서 동작함
- Cron/delayed run을 CLI에서 생성, 조회, 실행, 감사할 수 있음
- Timeout/retry/workflow 동작이 눈에 보이고 일관됨
- 위험한 동작이 approval boundary를 지킴
- Declarative skill이 TUI와 CLI 양쪽에서 일관되게 동작함
