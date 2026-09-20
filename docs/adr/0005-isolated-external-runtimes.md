# ADR-0005: External Agent Runtimes Are Isolated Adapters

Status: ACCEPTED
Date: 2026-09-20
Supersedes: None
Superseded by: None

## Context

OpenClaw, OpenAI Agents, LangGraph, ACP agents, coding agents, and future
runtimes have different languages, dependencies, checkpoints, release cadence,
and failure modes. Embedding them all in `jarvisd` would expand the trusted
computing base and couple upgrades.

## Decision

External runtimes normally run out of process behind a versioned,
capability-negotiated JARVIS runtime protocol. JARVIS owns run identity, policy,
tools, approvals, canonical state, limits, and completion reconciliation.
Runtime-native checkpoints remain opaque adapter state.

## Consequences

### Positive

- Runtime crash/dependency compromise is contained.
- Any implementation language can participate.
- Runtimes can upgrade and degrade independently.

### Negative

- Protocol, supervision, backpressure, and artifact transfer are real work.
- Some framework-native features need explicit capability negotiation.

## Alternatives Considered

- Link every runtime in process: rejected for security and dependency conflicts.
- Choose one framework permanently: rejected as product lock-in.

## Verification

- Fault-injection tests cover crash, hang, malformed event, oversized output,
  version mismatch, cancellation, and quarantine.
- Runtimes receive ephemeral scoped tools, never ambient credentials.
- Completion is reconciled against durable JARVIS outcomes.