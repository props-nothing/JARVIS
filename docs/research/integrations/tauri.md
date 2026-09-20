# Integration Evidence: Tauri v2

Status: ACCEPTED
Review scope: architecture only; refresh required before implementation
Owner: Desktop/release
Last verified: 2026-09-20
Revalidate by: 2027-03-19
Implementation gate: NOT READY

## Decision Summary

- Purpose: native Windows/macOS/Linux desktop client and packaging/updater.
- JARVIS boundary: thin UI client of `jarvisd`; no core policy/storage/secrets.
- Proposed version: current stable Tauri v2 selected/pinned during `UI-001`.
- Supported deployment modes: optional desktop companion for local/remote daemon.
- Explicitly unsupported initially: mobile, remote web content with native API
  access, desktop-owned database/tools/provider credentials.
- Kill switch: desktop can disconnect/disable update; daemon remains usable.

## Official Sources

| Source | URL | Version/date | Accessed | What it establishes |
| --- | --- | --- | --- | --- |
| `llms.txt` | https://tauri.app/llms.txt | Tauri v2 docs | 2026-09-20 | Guides/reference/full-doc indexes |
| Complete guides index | https://v2.tauri.app/_llms-txt/guides.txt | v2 | 2026-09-20 | Development/distribution/security discovery |
| Reference index | https://v2.tauri.app/_llms-txt/reference.txt | v2 | 2026-09-20 | Config/API/ACL references |
| Capabilities | https://v2.tauri.app/security/capabilities/ | Updated 2026-09-03 | 2026-09-20 | Window/webview/platform permission boundaries |
| Updater | https://v2.tauri.app/plugin/updater/ | v2 | 2026-09-20 | Mandatory signatures, artifacts, endpoints, permissions |
| Repository | https://github.com/tauri-apps/tauri | Current dev | 2026-09-20 | Bundler, ACL/schema, platform source |
| Updater source | https://github.com/tauri-apps/plugins-workspace/tree/v2/plugins/updater | v2 | 2026-09-20 | Plugin implementation/source |

Attempted `llms.txt` URLs that did not exist:

- None; `https://tauri.app/llms.txt` points to v2 documentation sets.

## Version Matrix

| Component | JARVIS target | Documentation target | Compatibility status |
| --- | --- | --- | --- |
| Tauri core/CLI | Stable v2 pinned together | v2 | UNVERIFIED until lockfile |
| Updater plugin | Matching supported stable | v2 | DOCUMENTED |
| Frontend | React/TypeScript versions selected later | N/A | UNVERIFIED |
| OS packages | Native CI target matrix | Bundler docs | DOCUMENTED, packaged proof pending |

## Contract

### Security Boundary

- Capabilities grant/deny Tauri core/plugin permissions per window/webview and
  can be platform-specific.
- Remote content access is not enabled. Bundled UI receives only minimal daemon
  client/OS presentation commands.
- Tauri capability controls reduce frontend compromise impact but do not protect
  against insecure Rust commands or overly broad scopes.
- JARVIS business authorization still occurs in `jarvisd`.

### Packaging and Updates

- Current updater docs report full desktop support for Windows/Linux/macOS.
- Updater signature verification is mandatory and cannot be disabled.
- Public verification key is configured in app; private signing key must remain
  secret and is supplied at build time, not assumed from `.env`.
- Artifacts differ per OS; native build/signing CI is preferred because Tauri
  warns cross-platform compilation is experimental/incomplete.
- Production updater endpoints use TLS unless dangerous insecure transport is
  explicitly enabled; JARVIS will not enable it in production.

### Data and Lifecycle

- Use OS-aware app/config/log/resource paths.
- Desktop stores only enrollment/session material appropriate to the client.
- Daemon/service lifecycle is JARVIS-owned; desktop can discover/start/request
  repair but must not contain a second core.
- Windows updater exits the application during install according to current docs;
  daemon compatibility/drain must be coordinated independently.

## Security Analysis

- A compromised webview must not access secrets, arbitrary filesystem/shell, or
  direct tool invocation.
- Explicitly list enabled capabilities in `tauri.conf`, rather than relying on
  automatic inclusion of every file in the capabilities directory.
- Use strict CSP and bundled assets; remote content receives no native bridge.
- Signing private key loss/compromise needs rotation/recovery before release.
- Update manifest/artifact validation supplements OS code signing/notarization.
- Desktop/daemon version skew cannot weaken API authorization.

## Normalization Map

| Tauri concept | JARVIS concept | Conversion/loss |
| --- | --- | --- |
| Window/webview capability | Desktop-local frontend permission | Not JARVIS business authorization |
| Tauri command | Minimal native desktop operation | Product operations call daemon API |
| Updater release | Desktop artifact update | Daemon/CLI update coordinated separately |
| App data/log path | Desktop client state | Canonical data remains daemon-owned |

## Falsifiable Claims

| ID | Claim | Label | Evidence | Check that could disprove it |
| --- | --- | --- | --- | --- |
| `TA-C001` | Tauri v2 supports desktop packaging on tier-1 OSes | DOCUMENTED | Bundler/updater docs/source | Native candidate fails packaged smoke |
| `TA-C002` | Updater refuses unsigned/invalid updates | DOCUMENTED | Official updater docs | Tampered artifact installs |
| `TA-C003` | Capabilities can constrain frontend plugin/core calls | DOCUMENTED | Official capability docs/schema | E2E calls denied command successfully |
| `TA-C004` | Desktop can remain a thin daemon client | INFERRED | JARVIS topology | Required OS feature forces core duplication |

## Test Plan

### Deterministic Tests

- [ ] Generated API client contract and auth expiry/reconnect
- [ ] No direct tool/database/secret frontend commands
- [ ] Capability/config schema and explicit enabled set
- [ ] CSP/navigation/deep-link validation
- [ ] Version mismatch and daemon unavailable/recovery UX

### Packaged Tests

- [ ] Native install/launch on every supported desktop target
- [ ] First-run daemon discovery/onboarding
- [ ] Signed update and tampered signature rejection
- [ ] Failed update/rollback and Windows before-exit coordination
- [ ] Accessibility/keyboard/screen-size and offline/reconnect E2E

## Operational Readiness

- [ ] Code signing/notarization and protected keys
- [ ] Updater key rotation/compromise process
- [ ] Crash logs and redacted diagnostics
- [ ] Desktop/daemon compatibility matrix
- [ ] Uninstall/client credential revocation

## Open Questions

- Exact Tauri/core/plugin/Node package versions at implementation.
- Whether daemon ships beside desktop or through one coordinated installer per OS.
- Update ownership when desktop and headless CLI installations coexist.
- Windows ARM64 and Linux ARM64 desktop dependency support.

## Change Log

| Date | Change | Evidence |
| --- | --- | --- |
| 2026-09-20 | Initial architecture review | Official Tauri v2 index, capabilities, updater, repository |