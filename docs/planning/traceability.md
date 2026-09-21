# Requirement Traceability

Status: ACCEPTED
Last updated: 2026-09-20

Every requirement in [PRODUCT.md](../../PRODUCT.md) maps to an owning architecture
area, normative contract or schema, roadmap milestone, implementation TODO, and
stable acceptance evidence. `N/A` is permitted only when the requirement governs
verification itself rather than a product wire or persistence contract.

## Functional Requirements

| Requirement | Owner document | Contract or schema | Milestone | Implementation TODO | Acceptance |
| --- | --- | --- | --- | --- | --- |
| `FR-ID-001` | [Identity](../architecture/identity-workspaces.md) | [Local API](../contracts/local-control-api.md) | 1/10 | `FND-007`, `PRD-003` | `ACC-002` |
| `FR-ID-002` | [Identity](../architecture/identity-workspaces.md), [data](../data/schema.md) | [Common](../contracts/common-conventions.md), [event](../contracts/event-envelope.md) | All durable milestones | `FND-006`, `MEM-009`, `AUT-001`, `PRD-003`, `PRD-009` | `ACC-032`, `ACC-044`, `ACC-079` |
| `FR-ID-003` | [Identity](../architecture/identity-workspaces.md), [security](../architecture/security.md) | [Common](../contracts/common-conventions.md) | 1/3/4/10 | `FND-007`, `TLS-004`, `MEM-009`, `PRD-003` | `ACC-002`, `ACC-021`, `ACC-032` |
| `FR-ID-004` | [Process topology](../architecture/process-topology.md) | [Local API](../contracts/local-control-api.md) | 1 | `FND-007` | `ACC-001`, `ACC-002`, `ACC-006` |
| `FR-ID-005` | [Identity](../architecture/identity-workspaces.md) | [Common](../contracts/common-conventions.md) | 10 | `PRD-003` | `ACC-077` |
| `FR-RUN-001` | [Agent/runtime](../architecture/agent-runtime.md), [data](../data/schema.md) | [Local API](../contracts/local-control-api.md) | 2 | `BRN-004` | `ACC-010`, `ACC-016` |
| `FR-RUN-002` | [Agent/runtime](../architecture/agent-runtime.md) | [Local API](../contracts/local-control-api.md) | 2 | `BRN-005`, `BRN-007` | `ACC-010` |
| `FR-RUN-003` | [Agent/runtime](../architecture/agent-runtime.md) | [Local API](../contracts/local-control-api.md) | 2 | `BRN-008` | `ACC-012`, `ACC-016` |
| `FR-RUN-004` | [Agent/runtime](../architecture/agent-runtime.md), [workflows](../architecture/workflows-events.md) | [Approval](../contracts/approval-contract.md) | 3/6 | `TLS-005`, `AUT-004` | `ACC-020`, `ACC-042` |
| `FR-RUN-005` | [Agent/runtime](../architecture/agent-runtime.md), [context](../architecture/memory-context.md) | [Model stream](../contracts/model-stream.md) | 2 | `BRN-005`, `BRN-006` | `ACC-014` |
| `FR-RUN-006` | [Agent/runtime](../architecture/agent-runtime.md) | [Runtime protocol](../contracts/runtime-protocol.md) | 7 | `RTM-001`, `RTM-002`, `RTM-003`, `RTM-008` | `ACC-050`, `ACC-051`, `ACC-052` |
| `FR-MOD-001` | [Model gateway](../architecture/model-gateway.md) | [Model stream](../contracts/model-stream.md) | 2 | `BRN-001` | `ACC-015` |
| `FR-MOD-002` | [Model gateway](../architecture/model-gateway.md) | [Model stream](../contracts/model-stream.md), [model data policy](../contracts/model-data-policy.md) | 2 | `BRN-001`, `BRN-009`, `BRN-010` | `ACC-013`, `ACC-017`, `ACC-018` |
| `FR-MOD-003` | [Model gateway](../architecture/model-gateway.md) | [Model stream](../contracts/model-stream.md) | 2 | `BRN-001`, `BRN-003` | `ACC-010`, `ACC-011` |
| `FR-MOD-004` | [Model gateway](../architecture/model-gateway.md) | [Model stream](../contracts/model-stream.md) | 2 | `BRN-003`, `BRN-008` | `ACC-011`, `ACC-013`, `ACC-017` |
| `FR-RT-001` | [Agent/runtime](../architecture/agent-runtime.md) | [Local API](../contracts/local-control-api.md) | 2 | `BRN-005` | `ACC-010`, `ACC-012` |
| `FR-RT-002` | [Agent/runtime](../architecture/agent-runtime.md) | [Runtime protocol](../contracts/runtime-protocol.md) | 7 | `RTM-001`, `RTM-003` | `ACC-050`, `ACC-051` |
| `FR-RT-003` | [Agent/runtime](../architecture/agent-runtime.md), [security](../architecture/security.md) | [Runtime protocol](../contracts/runtime-protocol.md) | 7 | `RTM-002`, `RTM-008` | `ACC-051`, `ACC-052` |
| `FR-TL-001` | [Tool fabric](../architecture/tool-fabric.md) | [Tool](../contracts/tool-contract.md) | 3 | `TLS-001`, `TLS-003` | `ACC-021` |
| `FR-TL-002` | [Tool fabric](../architecture/tool-fabric.md) | [Tool](../contracts/tool-contract.md) | 3 | `TLS-001` | `ACC-021`, `ACC-024` |
| `FR-TL-003` | [Tool fabric](../architecture/tool-fabric.md) | [Tool](../contracts/tool-contract.md), [approval](../contracts/approval-contract.md) | 3 | `TLS-004`, `TLS-005` | `ACC-020`, `ACC-021`, `ACC-024` |
| `FR-TL-004` | [Tool fabric](../architecture/tool-fabric.md), [workflows](../architecture/workflows-events.md) | [Tool](../contracts/tool-contract.md) | 3/6 | `TLS-006`, `AUT-004` | `ACC-025`, `ACC-044`, `ACC-063` |
| `FR-TL-005` | [Tool fabric](../architecture/tool-fabric.md), [security](../architecture/security.md) | [Tool](../contracts/tool-contract.md) | 3 | `TLS-003`, `TLS-012` | `ACC-021`, `ACC-073` |
| `FR-AP-001` | [Tool fabric](../architecture/tool-fabric.md) | [Approval](../contracts/approval-contract.md) | 3 | `TLS-005` | `ACC-020` |
| `FR-AP-002` | [Tool fabric](../architecture/tool-fabric.md), [voice](../architecture/voice-telephony.md) | [Approval](../contracts/approval-contract.md) | 3/8/9 | `TLS-005`, `TLS-013`, `UI-005`, `UI-009`, `VOI-010` | `ACC-020`, `ACC-027`, `ACC-062` |
| `FR-MCP-001` | [Tool fabric](../architecture/tool-fabric.md) | [Tool](../contracts/tool-contract.md) | 3 | `TLS-008`, `TLS-010` | `ACC-022` |
| `FR-MCP-002` | [Tool fabric](../architecture/tool-fabric.md) | [Tool](../contracts/tool-contract.md) | 3 | `TLS-009`, `TLS-010` | `ACC-023` |
| `FR-MCP-003` | [Tool fabric](../architecture/tool-fabric.md) | [Common](../contracts/common-conventions.md) | 3 | `TLS-008`, `TLS-009` | `ACC-022`, `ACC-023` |
| `FR-MCP-004` | [Tool fabric](../architecture/tool-fabric.md), [MCP evidence](../research/integrations/mcp.md) | [Common](../contracts/common-conventions.md) | 3 | `TLS-008`, `TLS-010` | `ACC-022`, `ACC-023` |
| `FR-PLG-001` | [Tool fabric](../architecture/tool-fabric.md) | [Plugin manifest](../contracts/plugin-manifest.md) | 3 | `TLS-011`, `TLS-014`, `TLS-015` | `ACC-026` |
| `FR-PLG-002` | [Tool fabric](../architecture/tool-fabric.md) | [Plugin manifest](../contracts/plugin-manifest.md) | 3 | `TLS-011`, `TLS-014`, `TLS-015` | `ACC-026` |
| `FR-MEM-001` | [Memory/context](../architecture/memory-context.md) | [Data schema](../data/schema.md) | 4 | `MEM-001`, `MEM-002`, `MEM-010` | `ACC-034` |
| `FR-MEM-002` | [Memory/context](../architecture/memory-context.md) | [Data schema](../data/schema.md) | 4 | `MEM-001`, `MEM-002` | `ACC-030`, `ACC-034` |
| `FR-MEM-003` | [Memory/context](../architecture/memory-context.md), [retention](../data/retention.md) | [Data schema](../data/schema.md) | 4 | `MEM-007` | `ACC-031` |
| `FR-MEM-004` | [Memory/context](../architecture/memory-context.md) | [Data schema](../data/schema.md) | 4 | `MEM-003`, `MEM-004`, `MEM-005`, `MEM-006` | `ACC-030`, `ACC-032`, `ACC-035`, `ACC-036` |
| `FR-CTX-001` | [Memory/context](../architecture/memory-context.md) | [Data schema](../data/schema.md) | 2/4 | `BRN-006`, `MEM-008` | `ACC-035` |
| `FR-CTX-002` | [Memory/context](../architecture/memory-context.md), [identity](../architecture/identity-workspaces.md) | [Common](../contracts/common-conventions.md) | 4 | `MEM-005`, `MEM-008`, `MEM-009` | `ACC-032`, `ACC-033`, `ACC-035` |
| `FR-EVT-001` | [Workflows/events](../architecture/workflows-events.md) | [Event](../contracts/event-envelope.md) | 6 | `AUT-001` | `ACC-044`, `ACC-074` |
| `FR-EVT-002` | [Workflows/events](../architecture/workflows-events.md), [storage](../architecture/storage-data.md) | [Event](../contracts/event-envelope.md) | 6 | `AUT-001` | `ACC-041`, `ACC-042`, `ACC-044` |
| `FR-EVT-003` | [Workflows/events](../architecture/workflows-events.md) | [Event](../contracts/event-envelope.md) | 6 | `AUT-001`, `AUT-006` | `ACC-044` |
| `FR-WF-001` | [Workflows/events](../architecture/workflows-events.md) | [Event](../contracts/event-envelope.md), [approval](../contracts/approval-contract.md) | 6 | `AUT-003`, `AUT-004`, `AUT-006` | `ACC-042`, `ACC-046` |
| `FR-WF-002` | [Workflows/events](../architecture/workflows-events.md) | [Event](../contracts/event-envelope.md) | 6 | `AUT-004`, `AUT-006` | `ACC-042`, `ACC-046` |
| `FR-WF-003` | [Workflows/events](../architecture/workflows-events.md) | [Event](../contracts/event-envelope.md) | 6 | `AUT-005` | `ACC-043` |
| `FR-CON-001` | [Connector platform](../architecture/connector-platform.md) | [Connector manifest](../contracts/connector-manifest.md) | 5 | `CON-001`, `CON-010` | `ACC-040` |
| `FR-CON-002` | [Connector platform](../architecture/connector-platform.md), [security](../architecture/security.md) | [Connector manifest](../contracts/connector-manifest.md) | 5 | `CON-002` | `ACC-040`, `ACC-070` |
| `FR-CON-003` | [Connector platform](../architecture/connector-platform.md), [workflows/events](../architecture/workflows-events.md) | [Event](../contracts/event-envelope.md) | 5 | `CON-003` | `ACC-041`, `ACC-044` |
| `FR-CON-004` | [Connector platform](../architecture/connector-platform.md) | [Connector manifest](../contracts/connector-manifest.md) | 5 | `CON-004`, `CON-010` | `ACC-045` |
| `FR-VOI-001` | [Voice/telephony](../architecture/voice-telephony.md) | [Voice call](../contracts/voice-call.md) | 8 | `VOI-001`, `VOI-011` | `ACC-065`, `ACC-067` |
| `FR-VOI-002` | [Voice/telephony](../architecture/voice-telephony.md) | [ElevenLabs edge](../contracts/elevenlabs-edge.md) | 8 | `VOI-003`, `VOI-005` | `ACC-060` |
| `FR-VOI-003` | [Voice/telephony](../architecture/voice-telephony.md), [tool fabric](../architecture/tool-fabric.md) | [ElevenLabs edge](../contracts/elevenlabs-edge.md) | 8 | `VOI-006` | `ACC-061` |
| `FR-VOI-004` | [Voice/telephony](../architecture/voice-telephony.md), [identity](../architecture/identity-workspaces.md) | [ElevenLabs edge](../contracts/elevenlabs-edge.md) | 8 | `VOI-005`, `VOI-007` | `ACC-062` |
| `FR-VOI-005` | [Voice/telephony](../architecture/voice-telephony.md) | [ElevenLabs edge](../contracts/elevenlabs-edge.md) | 8 | `VOI-008` | `ACC-063`, `ACC-066` |
| `FR-VOI-006` | [Voice/telephony](../architecture/voice-telephony.md) | [Voice call](../contracts/voice-call.md), [ElevenLabs edge](../contracts/elevenlabs-edge.md) | 8 | `VOI-001`, `VOI-007`, `VOI-009`, `VOI-011` | `ACC-060`, `ACC-062` through `ACC-067` |
| `FR-VOI-007` | [Voice/telephony](../architecture/voice-telephony.md) | [Voice call](../contracts/voice-call.md), [Twilio telephony](../research/integrations/telephony-twilio.md) | 8 | `VOI-001`, `VOI-013` | `ACC-069` |
| `FR-VOI-008` | [Voice/telephony](../architecture/voice-telephony.md) | [Twilio telephony](../research/integrations/telephony-twilio.md) | 8 | `VOI-012` | `ACC-069` |
| `FR-OPS-001` | [Installation/releases](../architecture/installation-release.md) | [Local API](../contracts/local-control-api.md) | 1/9 | `FND-010`, `FND-011`, `UI-007` | `ACC-001`, `ACC-072` |
| `FR-OPS-002` | [Installation/releases](../architecture/installation-release.md), [migrations](../data/migrations.md) | [Local API](../contracts/local-control-api.md) | 1 | `FND-006`, `FND-012`, `FND-014` | `ACC-003` through `ACC-005`, `ACC-009`, `ACC-072` |
| `FR-OPS-003` | [Process topology](../architecture/process-topology.md) | [Local API](../contracts/local-control-api.md) | 1/10 | `FND-007`, `FND-009`, `FND-012`, `PRD-003`, `PRD-008` | `ACC-001`, `ACC-006`, `ACC-008` |
| `FR-OPS-004` | [Observability](../architecture/observability.md) | [Local API](../contracts/local-control-api.md) | 1 | `FND-005`, `FND-008` | `ACC-003`, `ACC-070`, `ACC-076` |
| `FR-OPS-005` | [Observability](../architecture/observability.md) | [Common](../contracts/common-conventions.md) | 1 | `FND-005`, `FND-008`, `FND-013` | `ACC-070`, `ACC-076`, `ACC-078` |

## Non-Functional Requirements

| Requirement | Owner document | Contract or schema | Milestone | Implementation TODO | Acceptance |
| --- | --- | --- | --- | --- | --- |
| `NFR-SEC-001` | [Security](../architecture/security.md), [threat model](../security/threat-model.md) | [Tool](../contracts/tool-contract.md), [approval](../contracts/approval-contract.md) | All | `FND-007`, `TLS-004`, `TLS-012`, `PRD-006` | `ACC-002`, `ACC-020` through `ACC-026`, `ACC-070` through `ACC-073` |
| `NFR-SEC-002` | [Security](../architecture/security.md), [observability](../architecture/observability.md) | [Common](../contracts/common-conventions.md) | All | `FND-005`, `FND-013`, `PRD-006` | `ACC-070`, `ACC-076`, `ACC-078` |
| `NFR-REL-001` | [Storage](../architecture/storage-data.md), [migrations](../data/migrations.md), [workflows](../architecture/workflows-events.md) | [Event](../contracts/event-envelope.md), [local API](../contracts/local-control-api.md) | All durable milestones | `FND-006`, `BRN-004`, `TLS-005`, `TLS-006`, `MEM-001`, `CON-003`, `AUT-004`, `AUT-006`, `VOI-011`, `PRD-009` | `ACC-003` through `ACC-005`, `ACC-020`, `ACC-042`, `ACC-044`, `ACC-079` |
| `NFR-REL-002` | [Domain boundaries](../architecture/domain-boundaries.md) | [Common](../contracts/common-conventions.md) | All | `FND-002`, `TLS-003`, `AUT-005`, `RTM-008`, `VOI-009` | `ACC-016`, `ACC-017`, `ACC-051`, `ACC-067`, `ACC-073` |
| `NFR-PORT-001` | [Process topology](../architecture/process-topology.md), [installation](../architecture/installation-release.md) | [Local API](../contracts/local-control-api.md) | 1/9 | `FND-003`, `FND-009`, `FND-010`, `UI-008` | `ACC-001`, `ACC-006`, `ACC-008` |
| `NFR-COMP-001` | [API/protocols](../architecture/api-protocols.md), [migrations](../data/migrations.md) | [Common](../contracts/common-conventions.md) | All | `FND-004`, `FND-006`, `RTM-001` | `ACC-004`, `ACC-074` |
| `NFR-OBS-001` | [Observability](../architecture/observability.md) | [Common](../contracts/common-conventions.md) | 1 | `FND-002`, `FND-005` | `ACC-076` |
| `NFR-PRIV-001` | [Observability](../architecture/observability.md), [process topology](../architecture/process-topology.md) | [Common](../contracts/common-conventions.md) | 1/2 | `FND-004`, `FND-005`, `BRN-002` | `ACC-075` |
| `NFR-PRIV-002` | [Retention](../data/retention.md), [model gateway](../architecture/model-gateway.md), [voice](../architecture/voice-telephony.md) | [Model data policy](../contracts/model-data-policy.md), [ElevenLabs edge](../contracts/elevenlabs-edge.md) | 1/2/4/8 | `FND-013`, `BRN-010`, `MEM-007`, `VOI-007`, `VOI-009` | `ACC-018`, `ACC-031`, `ACC-064`, `ACC-078` |
| `NFR-UX-001` | [Installation](../architecture/installation-release.md) | [Local API](../contracts/local-control-api.md) | 1/2 | `FND-012`, `BRN-007` | `ACC-001`, `ACC-009`, `ACC-010` |
| `NFR-TEST-001` | [Testing strategy](../testing/strategy.md), [research policy](../research/integration-research-policy.md) | N/A - verification policy | All | `DOC-015` and every implementation TODO | `ACC-074` plus every applicable `ACC-*`; provider behavior also requires gated live tests |
| `NFR-VOI-001` | [Voice/telephony](../architecture/voice-telephony.md), [observability](../architecture/observability.md) | [Voice call](../contracts/voice-call.md), [Twilio telephony](../research/integrations/telephony-twilio.md) | 8 | `VOI-001`, `VOI-011`, `VOI-014` | `ACC-068` |
| `NFR-VOI-002` | [Model gateway](../architecture/model-gateway.md), [voice](../architecture/voice-telephony.md) | [Model stream](../contracts/model-stream.md) | 2/8 | `BRN-011`, `VOI-014` | `ACC-068` |

## Maintenance Rule

Adding or changing a product requirement must update this table and at least one
stable acceptance scenario before implementation is marked complete. Validation
must fail when a product requirement is missing, duplicated, points to an unknown
TODO/acceptance ID, or uses an unlinked placeholder owner. A row naming only
"unit tests" or an unnamed future suite is insufficient.