# Release Readiness and Owner Decisions

Status: ACCEPTED
Owner: Project owner and Release
Last reviewed: 2026-09-20

Engineering can build and test JARVIS before public-release governance exists.
It must not publish, sign as production, collect vulnerability reports through
an invented channel, or choose legal terms on the owner's behalf.

## Initial Support Matrix

Milestone 1 and the first public release require packaged CLI/daemon journeys on:

| Platform | Architecture | Required proof |
| --- | --- | --- |
| Windows | x86_64 | native build, install, per-user service, update, rollback, uninstall |
| macOS | arm64 | native build, install, LaunchAgent, update, rollback, uninstall |
| macOS | x86_64 | native or explicitly approved representative hardware proof while supported |
| Linux | x86_64 | native build, archive/package, systemd user service, update, rollback, uninstall |
| Linux | aarch64 | native or equivalent target-host proof for headless CLI/daemon journey |

An artifact is not supported merely because it cross-compiles. Windows ARM64,
desktop bundles, package-manager publication, and additional distributions are
separate support decisions with native lifecycle evidence.

## Owner-Controlled Gates

These stable tasks block a public release, not local development or test
artifacts:

| ID | Decision or resource | Required evidence | State |
| --- | --- | --- | --- |
| `OWN-001` | Select project license and contribution terms | Root license, dependency policy, third-party notice rules, owner approval | BLOCKED: project owner decision |
| `OWN-002` | Publish a private vulnerability-reporting channel | Monitored address/form, response ownership, safe disclosure text, test report | BLOCKED: project owner resource |
| `OWN-003` | Establish production signing identities and custody | Platform identities, artifact/update key hierarchy, access policy, backup, rotation and compromise drill | BLOCKED: project owner and Release |
| `OWN-004` | Approve release domain, channels, and metadata location | Controlled domain/repository, TLS, immutable metadata path, rollback/revocation publication | BLOCKED: project owner decision |
| `OWN-005` | Approve privacy/telemetry defaults and public notices | Data inventory, defaults, retention, subprocess/provider disclosures, legal review where required | BLOCKED: project owner decision |

The owner records decisions in repository configuration or a dated ADR/policy,
never only in chat history.

## Test Signing Versus Production Signing

CI uses isolated, explicitly labelled test keys and identities to prove:

- manifest and artifact signing/verification;
- rejection of altered bytes, metadata, wrong target, downgrade, and unknown key;
- key rollover and recovery behavior;
- update activation and rollback from a signed lower-version fixture.

Test keys are committed only when they have no trust outside tests and artifacts
are visibly marked non-production. Production private keys never enter the
repository, ordinary CI variables, logs, caches, artifacts, or developer
machines without an approved custody design.

Passing test-signing scenarios can complete implementation tasks. It cannot
satisfy `OWN-003` or authorize public promotion.

## First-Release Update Baseline

Because a first release has no public predecessor, CI builds a frozen signed
compatibility fixture with a lower semantic version and prior schema/API
metadata. `ACC-004` updates that fixture to the candidate, exercises failed
activation, and verifies old-binary refusal of unsupported newer state.

After the first public release, every supported prior minor becomes an update
test input. Removing an upgrade path requires a documented support-policy change
and migration/export path.

## Promotion Gate

Public promotion is denied unless:

- `OWN-001` through `OWN-005` are complete;
- every tier-1 target has current packaged acceptance evidence;
- release artifacts are built once in protected native environments and promoted
  without rebuilding;
- consumer-side checksum, signature, SBOM, and provenance verification passes;
- backup, update, rollback, repair, and uninstall procedures are current;
- known vulnerabilities, license violations, secret scans, or critical risks are
  resolved or the affected artifact is not shipped;
- release notes name supported targets, data/API minimums, breaking changes,
  migrations, known limitations, and rollback constraints.

No AI agent may complete an owner-controlled gate by inventing a contact,
license, legal conclusion, signing identity, domain, consent, or credential.
