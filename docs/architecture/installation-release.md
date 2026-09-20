# Installation, Updates, and Release Architecture

Status: PROPOSED

## Product Standard

A user installs JARVIS as an application. They do not clone a repository, install
Rust/Node/Python, run migrations manually, or understand service managers.

Source checkout remains a contributor workflow and is tested separately from
packaged user journeys.

## Tier-1 Release Matrix

Initial targets:

| Platform | Architecture | CLI/daemon | Desktop |
| --- | --- | --- | --- |
| Windows | x86_64 | Required | Required later |
| macOS | arm64 | Required | Required later |
| macOS | x86_64 | Required | Required later while supported |
| Linux | x86_64 | Required | Required later |
| Linux | aarch64 | Required | Evaluate desktop dependencies |

Windows ARM64 and additional Linux packaging become supported only when CI,
installer, service, dependency, and update tests exist. "Rust can compile" is
not sufficient platform support.

## Artifacts

Release artifacts include:

- `jarvis` and `jarvisd` native binaries;
- archive/installer appropriate to the platform;
- desktop bundle when available;
- checksums and detached signatures;
- SBOM and provenance attestation;
- release manifest with version, channel, target, minimum data/API versions,
  artifact URLs/hashes/signatures, and release notes;
- license and third-party notices after license selection.

Build on matching native CI where signing, packaging, OS integration, or desktop
dependencies make cross-compilation unreliable.

## Installation Methods

Offer in order of platform convention and security:

- signed MSI/installer or package manager on Windows;
- signed/notarized app/PKG and package manager on macOS;
- deb/rpm/AppImage or archive and package manager on Linux;
- verified archive for headless/portable use;
- convenience `install.sh` and `install.ps1` that download and verify a signed
  release manifest/artifact before activation.

A `curl | sh` or `iwr | iex` path can exist for convenience, but docs also show
download-inspect-verify-run steps. The bootstrap script itself is versioned,
minimal, TLS-only, and covered by tests.

## Installer Flow

1. Detect OS, architecture, install scope, and existing installation.
2. Select channel/version and fetch signed release metadata.
3. Verify metadata and artifact hash/signature before execution/extraction.
4. Stage files in a new versioned directory.
5. Validate binary self-check and platform dependencies.
6. Atomically switch the active launcher/path.
7. Run non-destructive config/data migration preflight.
8. Onboard or preserve existing profile.
9. Optionally install/refresh per-user background service.
10. Start, wait for readiness, run doctor, and print exact next command.

Failure before activation leaves the old install untouched. Failure after switch
automatically rolls back executable selection when data compatibility permits.

## Onboarding

First launch asks only what the selected path needs:

1. Local/portable or connect-to-server mode.
2. Create/confirm local owner and client enrollment.
3. Select fake/local/cloud model setup; secret goes directly to secret store.
4. Confirm data/backup location and privacy/telemetry defaults.
5. Optional connector and voice setup, clearly skippable.
6. Optional background service installation.
7. Run health/doctor and first safe test request.

Onboarding is resumable and idempotent. A failed optional provider does not
corrupt the base installation.

## Service Installation

Personal defaults avoid elevation:

- Linux systemd user unit;
- macOS launchd LaunchAgent;
- Windows per-user Scheduled Task/startup registration selected through tested
  capability detection.

System/server services are separate explicit admin workflows. Service files
contain no provider secret values and point through a stable launcher so updates
do not leave stale version-specific paths.

## Updates

Support stable, preview, and development channels with explicit opt-in. Update
steps:

1. Check signed metadata for current channel/target.
2. Verify compatibility and available disk space.
3. Download to staging with resumable bounded behavior.
4. Verify checksum/signature and binary self-test.
5. Back up config/database when migration policy requires it.
6. Drain daemon and prevent new durable work.
7. Atomically activate and restart.
8. Verify readiness, schema, version, and doctor subset.
9. Roll back binary/service definition if safe and validation fails.

Database migrations are forward-only unless an explicitly tested down migration
exists. An update requiring an irreversible migration warns and creates a tested
backup before activation.

For Tauri desktop, use the current Tauri v2 updater and its mandatory signature
verification. Desktop update keys and daemon/CLI release keys require a documented
rotation/recovery plan; losing a private key must not strand installed users.

## Compatibility

- CLI and daemon negotiate API version/features.
- Desktop declares compatible daemon range.
- External runtime/plugin manifests declare minimum/maximum protocol/JARVIS
  versions.
- A newer config/schema is never silently rewritten by an older binary.
- Update checks distinguish installed binary version from running daemon version.
- Doctor detects stale service definitions and duplicate installations.

## Uninstall

Uninstall is explicit about:

- stopping/removing service and launchers;
- removing installed binaries and updater state;
- retaining or deleting config/data/logs/backups/plugins;
- revoking local sessions and optionally connector/provider credentials;
- removing remote webhooks/subscriptions where possible;
- generating an export/backup first.

Default uninstall retains user data and prints its path. Purge requires a separate
confirmation and cannot claim provider-side deletion without evidence.

## Release CI

Required lanes:

- format, lint, unit, integration, docs/link/schema drift;
- migration/backup/restore and prior-version upgrade;
- dependency vulnerabilities, license policy, SBOM;
- reproducible/repeatable build checks where practical;
- native target builds and signing in protected environments;
- clean-home install/onboard/service/status/doctor/update/uninstall journey;
- offline install/repair and interrupted install/update;
- desktop packaging/updater tests when introduced;
- artifact signature/provenance verification from the user's path.

Release promotion consumes already built artifacts; it does not rebuild different
bits for stable. Record skipped target/live lanes and block promotion when a
tier-1 lane lacks evidence.

## Release Safety

- Separate developer, CI, and release signing authority.
- Require protected environment approval for public promotion.
- Never expose private keys in logs/artifacts or long-lived runner files.
- Sign immutable manifests; HTTPS alone is insufficient.
- Publish revocation/compromise and key-rotation procedures before first release.
- Maintain a rollback channel and previous artifacts within support policy.
- Test the update path from every supported prior minor, not only a fresh install.