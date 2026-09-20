# JARVIS Security Policy

Status: ACCEPTED
Release state: PRE-RELEASE
Last updated: 2026-09-20

JARVIS is not yet implemented and has no supported production release. Do not
use this repository with real credentials, customer data, payment authority,
production infrastructure, or telephone campaigns.

## Reporting

A private vulnerability reporting channel has not yet been published. Do not
open a public issue containing credentials, personal data, exploitable details,
or unredacted logs. Until a private channel is established, retain the report
privately and contact the repository owner through an already trusted channel.

Publishing a private security contact is a release blocker.

## Security Baseline

- Local endpoints bind to loopback by default.
- Remote access is disabled until authenticated and explicitly enabled.
- All caller, runtime, plugin, model, tool, document, webhook, and MCP input is
  untrusted.
- Authorization is deterministic and enforced outside models.
- Capability discovery never grants execution permission.
- Side effects are validated, policy-checked, approved where required,
  idempotent, bounded, and audited.
- Secrets are referenced, not copied through prompts, config records, logs, URLs,
  traces, or support bundles.
- Workspaces are isolated in storage queries and authorization checks.
- Code execution uses an explicit sandbox profile and deny-by-default network.
- Persisted state and public protocols are versioned and migration-tested.
- Installers and updates are signed and verified before activation.

## Highest-Risk Surfaces

- Tool execution and shell/code sandboxes
- MCP servers and external plugin/runtime processes
- OAuth callbacks, webhooks, and remote API exposure
- Cross-workspace memory/context retrieval
- Approval binding and side-effect replay
- Voice caller identity and outbound calling consent
- Secret resolution and redaction
- Installer/update supply chain
- Backup, restore, export, and deletion

Changes to these surfaces require abuse-case tests and a threat-model update.

## Data Handling

JARVIS should minimize collection and make retention visible. Before the first
release, the product must provide:

- user-visible recording, transcript, memory, and telemetry controls;
- data export, correction, deletion, and retention behavior;
- support-safe diagnostics preview and redaction;
- provider-specific data-use and residency disclosures;
- encrypted transport and appropriate encrypted-at-rest profiles;
- backup and restore guidance that includes secret handling.

## Dependency and Supply Chain

Before release, CI must enforce:

- dependency vulnerability and license review;
- locked/reproducible dependency resolution where practical;
- SBOM generation;
- signed release artifacts and checksums;
- build provenance attestations;
- protected release environments and key separation;
- installer tests from clean operating-system images.

The project must select a license before distributing artifacts.