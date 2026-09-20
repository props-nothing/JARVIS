# Architecture Decision Records

ADRs record decisions that are expensive or dangerous to rediscover. Accepted
ADRs are immutable except for typo/link repairs. Change a decision with a new
ADR that marks the old one superseded.

## Index

| ADR | Status | Decision |
| --- | --- | --- |
| [0001](0001-rust-control-plane.md) | ACCEPTED | Rust owns the durable control plane |
| [0002](0002-daemon-client-topology.md) | ACCEPTED | One daemon serves thin clients |
| [0003](0003-dual-storage-profiles.md) | ACCEPTED | SQLite local, PostgreSQL server |
| [0004](0004-canonical-tool-policy-and-mcp.md) | ACCEPTED | One canonical tool policy path; MCP at the edge |
| [0005](0005-isolated-external-runtimes.md) | ACCEPTED | External runtimes are isolated adapters |
| [0006](0006-jarvis-owned-memory.md) | ACCEPTED | JARVIS owns canonical memory |
| [0007](0007-voice-as-interface.md) | ACCEPTED | Voice providers are interfaces; JARVIS is the brain |
| [0008](0008-native-workflows-before-temporal.md) | ACCEPTED | Native database workflows first |
| [0009](0009-process-plugins-before-native-abi.md) | ACCEPTED | Process protocols before dynamic libraries |
| [0010](0010-upstream-evidence-gate.md) | ACCEPTED | Current official evidence precedes integrations |

Use [the template](template.md) for new decisions.