# Process Plugin Manifest Contract

Status: ACCEPTED
Contract version: 0.1.0
Lifecycle: DRAFT
Owner: Tool and Runtime Platform

## Purpose

This contract describes installable out-of-process extensions. A plugin package
can provide MCP tools, an agent runtime, event handlers, connector components,
or another explicitly supported process protocol. Discovery and installation do
not grant execution or data access.

Native dynamic libraries are not a v1 plugin format. A package that needs host
code execution uses a supervised process, MCP, HTTP/WebSocket on an authenticated
loopback channel, or a future researched WASI profile.

## Manifest

```json
{
  "$schema": "https://example.invalid/jarvis/plugin-manifest-v1.schema.json",
  "schema_version": "1.0.0",
  "id": "example.research-runtime",
  "name": "Example Research Runtime",
  "version": "1.2.3",
  "publisher": "example.org",
  "package": {
    "digest": "sha256:...",
    "signature": {"kind":"sigstore-bundle","value_ref":"package/signature.json"},
    "source": "https://example.org/releases/1.2.3"
  },
  "compatibility": {
    "jarvis": ">=0.1.0 <0.2.0",
    "protocol": {"kind":"jarvis-runtime","versions":["0.1.0"]},
    "platforms": ["windows-x86_64","macos-arm64","linux-x86_64"]
  },
  "entrypoint": {
    "executable": "bin/example-runtime",
    "arguments": ["serve"],
    "working_directory": "isolated-data",
    "environment_allowlist": []
  },
  "capabilities": {
    "provides": ["runtime.text","artifacts.write"],
    "requests": ["jarvis.tools.read:selected","network:api.example.org"]
  },
  "configuration_schema": {},
  "resource_limits": {
    "startup_timeout_ms": 10000,
    "message_bytes": 1048576,
    "stdout_bytes": 1048576,
    "stderr_bytes": 1048576
  },
  "documentation": {
    "setup": "docs/setup.md",
    "privacy": "docs/privacy.md",
    "support": "https://example.org/support"
  }
}
```

The placeholder schema URL is replaced by a generated repository schema before
plugin implementation. Unknown top-level fields fail until a compatible schema
version defines extension behavior.

## Stable Identity and Provenance

- `id`, `publisher`, package digest, signature identity, source, version, and
  protocol form the installed source identity.
- Display name or executable filename never identifies a plugin.
- Package bytes are verified before extraction or execution. Extraction rejects
  path traversal, links escaping staging, reserved device names, and permission
  broadening.
- An update is a new source identity. Existing grants carry forward only under
  an explicit policy that proves publisher continuity and no capability/schema/
  effect expansion; otherwise they require review.
- Unsigned local development packages are allowed only in an explicit developer
  profile, visibly labelled, with no production grant inheritance.

## Protocol and Entrypoint

Supported initial protocol kinds are versioned JARVIS runtime protocol and MCP
stdio. Other process protocols require an accepted contract and evidence note.

Entrypoint rules:

- executable paths are package-relative, canonicalized, and cannot escape the
  verified installation root;
- arguments are fixed manifest values plus typed JARVIS-owned launch arguments,
  never shell-concatenated strings;
- environment starts from a minimal allowlist and contains scoped handles or
  short-lived credentials, not daemon ambient secrets;
- working/data directories are plugin- and workspace-scoped;
- stdout is protocol-only where required; logs use bounded stderr/diagnostic
  channels;
- startup, heartbeat, idle, request, shutdown, output, CPU, memory, process,
  filesystem, and network limits are explicit capabilities of the platform
  sandbox profile.

If a required limit cannot be enforced on a platform, the plugin profile is
unsupported there or runs with a visible weaker-isolation warning and policy
deny for dangerous capabilities. The model cannot waive this restriction.

## Permissions and Grants

Manifest `requests` are untrusted permission requests. Installation creates no
grant. A durable grant binds:

```text
plugin source identity and manifest digest
workspace and granting principal
specific capability/resource/tool selectors
filesystem/network/process constraints
data sensitivity and effect ceiling
validity, expiry, policy version, and revocation
```

JARVIS reauthorizes every tool/data operation at invocation time. A plugin that
provides tools cannot self-classify their effects or bypass the canonical tool
pipeline. Discovery output, descriptions, annotations, and schemas are untrusted
until normalized and reviewed.

## Configuration and Secrets

`configuration_schema` marks secret-reference fields, sensitive display fields,
defaults, bounds, and restart requirements. Ordinary config stores references,
not secret material. Secret values are resolved only for the operation/process
that requires them and are excluded from command lines, environment where an
alternative exists, logs, health output, crash reports, and support bundles.

Configuration migration is versioned, validated before activation, and rolls
back independently from package activation where possible.

## Lifecycle

```text
DISCOVERED
VERIFIED
INSTALLED_DISABLED
ENABLED
UNHEALTHY
QUARANTINED
DISABLED
REMOVING
REMOVED
```

Required operations:

- inspect provenance, requested capabilities, compatibility, and docs;
- install without enabling;
- grant/revoke per workspace;
- enable, health-check, restart, disable, and quarantine;
- stage/verify/update with rollback to prior package/config;
- remove process/data/config while separately choosing whether grants, artifacts,
  runtime checkpoints, and audit history are retained;
- revoke short-lived credentials and stop child processes on disable/removal.

Repeated crashes, protocol violations, output floods, signature/provenance
failure, or denied-resource attempts can trigger quarantine. Quarantine survives
daemon restart and never silently re-enables on package update.

## Failure and Audit

Plugin failure cannot crash `jarvisd` or corrupt canonical state. Runtime-
reported success is reconciled against durable JARVIS events/artifacts. Every
install, verification, grant, launch, health transition, quarantine, update,
rollback, disable, and removal emits a safe audit record with source identity and
correlation IDs.

Logs and diagnostic capture are size/time bounded and redacted. Support bundles
include manifest/provenance/health summaries, never secret material or arbitrary
plugin data without explicit review.

## Required Contract Tests

Before `TLS-011` or an extension adapter is complete, test:

1. schema, compatibility range, platform, and unknown-field validation;
2. signature/digest/source verification and tampered archive rejection;
3. archive traversal, symlink/junction, executable path, and argument injection;
4. install-without-grant and denied discovery/execution before explicit grant;
5. source/schema/capability change invalidating unsafe inherited grants;
6. scoped environment, filesystem, network, tool, and secret access;
7. startup failure, hang, crash loop, output flood, protocol corruption, and
   quarantine persistence;
8. update activation failure and package/config rollback;
9. disable/remove process cleanup, credential revocation, and data-retention
   choices;
10. redacted diagnostics and no daemon failure under hostile plugin behavior.

These tests provide evidence for `ACC-026` and `FR-PLG-001`/`FR-PLG-002`.
