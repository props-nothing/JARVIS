# Architecture Index

Status: PROPOSED

These documents define the intended JARVIS architecture. Accepted ADRs are the
authority for decisions; the architecture explains how those decisions fit
together.

## Core

- [System overview](overview.md)
- [Process topology](process-topology.md)
- [Domain boundaries](domain-boundaries.md)
- [Agent and runtime model](agent-runtime.md)
- [Model gateway](model-gateway.md)
- [Tool and MCP fabric](tool-fabric.md)
- [Storage and data](storage-data.md)
- [Memory and context](memory-context.md)
- [Events and workflows](workflows-events.md)
- [Identity and workspaces](identity-workspaces.md)
- [Connector platform](connector-platform.md)
- [API and protocol surfaces](api-protocols.md)
- [Voice and telephony](voice-telephony.md)
- [Realtime media sessions](realtime-media.md)
- [Security architecture](security.md)
- [Observability and diagnostics](observability.md)
- [Installation and releases](installation-release.md)

## Architectural Test

For every new component, answer:

1. Which bounded context owns the decision?
2. Which process owns the lifecycle and durable state?
3. What is the typed contract at each trust boundary?
4. Which principal and workspace authorize the operation?
5. What is persisted before a success event is emitted?
6. How does cancellation, timeout, restart, replay, and version mismatch behave?
7. What check can falsify the most important assumption?

If those answers are spread across framework callbacks or prompts, the boundary
is not ready.