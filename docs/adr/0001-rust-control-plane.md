# ADR-0001: Rust Owns the Durable Control Plane

Status: ACCEPTED
Date: 2026-09-20
Supersedes: None
Superseded by: None

## Context

JARVIS must run continuously across operating systems, guard credentials and
side effects, persist workflows, supervise untrusted processes, and remain
independent from rapidly changing model/agent frameworks. A single framework-
owned Python or TypeScript process would couple product authority to that
framework's lifecycle and data model.

## Decision

Rust owns `jarvisd`, domain/application logic, public protocol contracts,
identity, context, canonical memory, policy, approvals, tools, workflows,
events, scheduling, connectors, secrets, audit, and persistence ports.

Other languages are allowed in isolated runtimes/workers behind versioned
protocols. Rust does not need to reimplement mature external frameworks.

## Consequences

### Positive

- One stable security and durability boundary.
- Native cross-platform binaries without a core language runtime dependency.
- Strong types and explicit concurrency/resource ownership.
- Python/TypeScript ecosystems remain available through adapters.

### Negative

- Smaller ecosystem for some AI integrations.
- More adapter/protocol work than embedding one framework everywhere.
- Contributors need Rust expertise for core changes.

## Alternatives Considered

- TypeScript control plane: excellent integration ecosystem, but weaker fit for
  the permanent native daemon/security boundary selected here.
- Python control plane: excellent AI ecosystem, but too coupled to runtime and
  packaging concerns for the installable core.
- Framework-owned core: fast prototype, unacceptable long-term authority lock-in.

## Verification

- Domain/application crates have no external framework/provider dependencies.
- Core install works without Node or Python.
- A Python/TypeScript runtime can fail while `jarvisd` remains healthy.