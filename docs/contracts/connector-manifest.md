# Connector Manifest Contract

Status: ACCEPTED
Contract version: 0.1.0
Lifecycle: DRAFT

## Example

```json
{
  "$schema": "https://example.invalid/jarvis/connector-manifest-v1.schema.json",
  "id": "github",
  "name": "GitHub",
  "version": "1.0.0",
  "publisher": "jarvis.project",
  "integration_type": "service",
  "data_flow": "cloud_push_and_poll",
  "jarvis": {"minimum_version": "0.1.0"},
  "auth": [
    {
      "id": "github_app",
      "kind": "oauth2",
      "scopes": []
    }
  ],
  "capabilities": {
    "tools": [],
    "events": [],
    "resources": []
  },
  "config_schema": {},
  "secret_fields": [],
  "lifecycle": {
    "supports_reauth": true,
    "supports_disable": true,
    "supports_unload": true,
    "supports_remove": true
  },
  "documentation": {},
  "quality": {"target": "bronze"}
}
```

The placeholder schema URL is replaced by the generated repository schema before
implementation. Manifests with unknown top-level fields fail until a compatible
schema version defines extension behavior.

## Identity

- `id` is stable lowercase namespace syntax and matches directory/config/tool
  namespace.
- `version` is connector package/implementation SemVer.
- `publisher` is immutable provenance identity, not display text.
- Installation identity includes package hash/signature and source.

## Integration Type

Initial values:

```text
service
account
device
hub
system
virtual
```

`virtual` provides discovery/documentation mapping and no executable code.

## Data Flow

Initial values:

```text
local_poll
local_push
cloud_poll
cloud_push
cloud_push_and_poll
calculated
```

This communicates operational/privacy behavior; it is not used alone for
authorization.

## Authentication

Each auth strategy declares type, official authorization/token endpoints or
discovery behavior, PKCE/device/service support, requested scopes by capability,
secret references, callback requirements, refresh/rotation behavior, and
evidence version.

Endpoint values are trusted manifest/config data, never model-generated. Dynamic
OAuth discovery is restricted and SSRF-protected.

## Capabilities

Manifest entries reference canonical tool/event/resource schemas and trusted
effect/risk declarations. Provider discovery may alter availability, but cannot
silently add granted effects. New capabilities require user/admin review.

## Lifecycle and Diagnostics

Manifest declares supported lifecycle operations, health probes, remote cleanup,
migration range, and safe diagnostics schema. Claiming support requires tests.

## Quality

`quality.yaml`, not self-asserted manifest metadata alone, records every rule as
`done`, `todo`, or `exempt` with evidence/reason. CI computes the effective tier.

## Validation

CI validates schema, ID/directory match, unique capabilities, valid versions,
documentation/evidence presence, effect/risk metadata, no inline secret values,
quality claims, and generated registry consistency.