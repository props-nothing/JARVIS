# JARVIS Documentation

This directory is the engineering source of truth for JARVIS. Product intent,
architecture, contracts, decisions, implementation plans, integration evidence,
and operating procedures live here. Code must not silently contradict these
documents.

## Architecture

- [Architecture index](architecture/README.md)
- [System overview](architecture/overview.md)
- [Process topology](architecture/process-topology.md)
- [Domain boundaries](architecture/domain-boundaries.md)
- [Agent and runtime model](architecture/agent-runtime.md)
- [Model gateway](architecture/model-gateway.md)
- [Tool and MCP fabric](architecture/tool-fabric.md)
- [Storage and data](architecture/storage-data.md)
- [Memory and context](architecture/memory-context.md)
- [Events and workflows](architecture/workflows-events.md)
- [Identity and workspaces](architecture/identity-workspaces.md)
- [Connector platform](architecture/connector-platform.md)
- [API and protocol surfaces](architecture/api-protocols.md)
- [Voice and telephony](architecture/voice-telephony.md)
- [Security architecture](architecture/security.md)
- [Observability and diagnostics](architecture/observability.md)
- [Installation and releases](architecture/installation-release.md)

## Decisions

- [Architecture decision records](adr/README.md)

## Contracts

- [Contract index](contracts/README.md)
- [Common conventions](contracts/common-conventions.md)
- [Local control API](contracts/local-control-api.md)
- [Runtime protocol](contracts/runtime-protocol.md)
- [Model stream](contracts/model-stream.md)
- [Model data policy](contracts/model-data-policy.md)
- [Canonical tools](contracts/tool-contract.md)
- [Process plugin manifest](contracts/plugin-manifest.md)
- [Approvals](contracts/approval-contract.md)
- [Event envelope](contracts/event-envelope.md)
- [Connector manifest](contracts/connector-manifest.md)
- [Provider-neutral voice call](contracts/voice-call.md)
- [ElevenLabs compatibility edge](contracts/elevenlabs-edge.md)

## Data

- [Data model index](data/README.md)
- [Conceptual relational schema](data/schema.md)
- [Migration strategy](data/migrations.md)
- [Retention and deletion](data/retention.md)

## Security

- [Security documentation](security/README.md)
- [Threat model](security/threat-model.md)

## Testing

- [Testing index](testing/README.md)
- [Testing strategy](testing/strategy.md)
- [Acceptance scenarios](testing/acceptance.md)

## Planning

- [Planning index](planning/README.md)
- [First vertical slice](planning/first-vertical-slice.md)
- [Risk register](planning/risk-register.md)
- [Requirement traceability](planning/traceability.md)

## Operations

- [Operations index](operations/README.md)
- [Foundation implementation handoff](operations/foundation-handoff.md)
- [Release readiness and owner decisions](operations/release-readiness.md)
- [CI gates and native lanes](operations/ci-gates.md)

## Research

- [Research and evidence index](research/README.md)
- [Integration research policy](research/integration-research-policy.md)
- [Integration evidence template](research/integration-evidence-template.md)
- [Evidence readiness manifest](research/evidence-manifest.json)
- [Routine dependency evidence](research/dependencies.md)
- [Official source registry](research/source-registry.md)
- [Upstream project study](research/upstream-projects.md)
- [Rust Foundation evidence](research/integrations/rust-foundation.md)
- [GitHub Actions CI evidence](research/integrations/github-actions.md)
- [MCP evidence](research/integrations/mcp.md)
- [ElevenLabs evidence](research/integrations/elevenlabs.md)
- [Twilio telephony evidence](research/integrations/telephony-twilio.md)
- [LiveKit evidence](research/integrations/livekit.md)
- [Tauri evidence](research/integrations/tauri.md)

## Reading Order

Once the bootstrap is complete, contributors should read:

1. Product vision and scope
2. System architecture and trust boundaries
3. Accepted architecture decisions
4. Contracts for the subsystem being changed
5. Current roadmap milestone and acceptance criteria
6. Integration evidence for every external dependency involved

## Truth Hierarchy

When documents disagree, use this order and fix the conflict in the same change:

1. Accepted or superseding ADRs
2. Versioned contracts and schemas
3. Architecture documents
4. Security requirements
5. Product specification and acceptance criteria
6. Roadmap and TODO tracking
7. Research notes

Official upstream specifications still control external protocol behavior. A
local document records the version and assumptions JARVIS supports; it does not
rewrite the upstream contract.