# Contract Index

Status: ACCEPTED
Lifecycle: DRAFT

Contracts define required public or persistence-sensitive behavior. Prose and
examples are normative for implementation unless explicitly labelled
`UNVERIFIED` or placeholder. Generated JSON Schemas/OpenAPI become the wire
artifacts during implementation. If prose and generated schema disagree,
release is blocked until the owning source is corrected.

## Contracts

- [Common conventions](common-conventions.md)
- [Local control API](local-control-api.md)
- [Runtime protocol](runtime-protocol.md)
- [Model stream](model-stream.md)
- [Model data policy](model-data-policy.md)
- [Canonical tools](tool-contract.md)
- [Process plugin manifest](plugin-manifest.md)
- [Approvals](approval-contract.md)
- [Event envelope](event-envelope.md)
- [Connector manifest](connector-manifest.md)
- [Provider-neutral voice call](voice-call.md)
- [Provider-neutral media session](media-session.md)
- [ElevenLabs compatibility edge](elevenlabs-edge.md)

## Contract Lifecycle

Document `Status` records whether the architecture decision is accepted. The
separate lifecycle below records compatibility guarantees. An `ACCEPTED`
contract may remain `DRAFT` before its first stable release, but its stated
behavior is normative for implementation and tests.

- `DRAFT`: implementation may change freely; no release compatibility promise.
- `EXPERIMENTAL`: usable behind opt-in; migration may be required.
- `STABLE`: compatibility policy applies within the major version.
- `DEPRECATED`: supported for a documented interval with replacement.
- `REMOVED`: no longer accepted by the current major.

Every generated contract records its lifecycle, semantic version, and minimum
JARVIS version.

## Change Rule

Additive optional fields and new event variants can be compatible when clients
are required to ignore unknown fields/variants. Removing, renaming, changing
meaning/default, widening side effects, or weakening authorization is breaking.
Security tightening can be intentionally breaking and must include migration and
operator communication.