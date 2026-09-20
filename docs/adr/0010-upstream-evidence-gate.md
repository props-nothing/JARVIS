# ADR-0010: Current Official Evidence Precedes Integration Code

Status: ACCEPTED
Date: 2026-09-20
Supersedes: None
Superseded by: None

## Context

Model, voice, agent, MCP, OAuth, and hosted-service APIs change rapidly. Synthetic
fixtures and remembered examples often agree with incorrect assumptions. This is
especially dangerous for auth, event shapes, streaming, retries, pricing, and
security behavior.

## Decision

Every external integration requires a dated evidence note before implementation.
Agents first discover official sources through current `llms.txt` indexes when
available, then read the exact versioned spec/schema, auth/security/limits,
changelog, SDK source/examples/tests, and pinned-version compatibility. Key
claims must have falsifying contract/live checks.

## Consequences

### Positive

- Fewer stale-API implementations and false confidence from synthetic fixtures.
- Design assumptions, limitations, and contradictions are visible.
- Upgrade and incident work starts with reproducible evidence.

### Negative

- Integrations take longer to start.
- Evidence needs periodic revalidation and some providers lack good sources.

## Alternatives Considered

- Research during implementation: rejected because architecture may already be
  shaped by false assumptions.
- Trust SDK type signatures alone: rejected because lifecycle/security/limits
  often live outside types.
- Community examples first: rejected as non-authoritative.

## Verification

- CI/review checks integration changes for an evidence note and verification
  date.
- Contract fixtures record provenance and capture date.
- Beta/realtime notes expire after 90 days; stable notes after 180 days unless a
  provider change requires earlier review.