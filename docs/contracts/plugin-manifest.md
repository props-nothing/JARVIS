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

## Implementation Status

**Implemented**: the manifest as types, parsed and validated by
`jarvis_infrastructure::plugin::PluginManifest::parse`. This is `TLS-011`'s *definition* half — the
document the contract describes, as a value a reader and a writer share. It enforces the field rules
whose failure is a filesystem, execution, or log-integrity fault rather than a cosmetic one:

- **schema and unknown fields.** `schema_version` must be one this build understands, and
  `deny_unknown_fields` refuses a top-level field the type does not model, because the contract says
  "unknown top-level fields fail until a compatible schema version defines extension behavior";
- **source identity.** `id` and `publisher` are slugs, not free text — the contract's rule that "display
  name or executable filename never identifies a plugin" is what makes `id` the field a grant is
  compared against, so it cannot carry control characters or unbounded length. `name` is display text:
  bounded and control-free, but otherwise unrestricted;
- **the entrypoint path.** `executable` and `working_directory` are package-relative paths that cannot
  escape their root — no absolute path, no `..` traversal, no empty or `.` segment, no Windows
  drive/ADS colon, no reserved device name (matched on the stem, so `con.txt` is caught and
  `console.log` is not), and no trailing dot or space (which Windows strips, making two strings name one
  file). Every one of those is a way two strings name one file or one string leaves the package;
- **the package digest.** `sha256:<64 lowercase hex>` with the algorithm prefix required, so a bare
  digest that could be any algorithm is refused rather than compared as though it were `SHA-256`;
- **the supervision limits.** Non-zero and bounded, because a zero limit is a refusal wearing a limit's
  name and an unbounded one is the hang the limit exists to prevent;
- **the protocol kind** is one of the two this build supports (`jarvis-runtime`, `mcp-stdio`); anything
  else "requires an accepted contract and evidence note" and is refused by name rather than carried as
  an opaque string.

`configuration_schema` is carried as a bounded JSON object and required to *be* an object; JARVIS does
not interpret a plugin's own configuration schema here. The `$schema` keyword the contract's example
carries is modelled and validated as a bounded reference — it is **not** fetched or honoured, because
accepting a URL from an untrusted document and resolving it is how a manifest would choose its own
validator.

**The lifecycle is a state machine** (`jarvis_domain::plugin::PluginState`). The nine states above are
the variants, and the operations are the legal edges; `TLS-015`'s supervisor consults this table rather
than re-deriving it, the same arrangement `ApprovalState` and `RunState` use. Two of the contract's
rules are encoded as **absent** edges rather than as checks a caller could forget:

- **install never enables.** `Verified -> Enabled` is not an edge, so a verified package's only target is
  `InstalledDisabled` — the contract's "install without enabling" is structural, and an installer that
  enabled on install would be a code change against the table rather than a flag.
- **nothing re-enables a quarantined plugin directly.** `Quarantined -> Enabled` is absent; the only exit
  from quarantine is `Disabled`, from which an operator re-enables. That is *stronger* than the contract's
  "never silently re-enables on package update": because no transition reaches `Enabled` from
  `Quarantined` at all, a supervisor that forgot to compare the package version still cannot re-enable
  one. A running plugin (`Enabled` or `Unhealthy`) also cannot be removed directly — it must be disabled
  or quarantined first, so a child process is stopped before the package it runs against is taken away.

**The stable identity is a typed tuple** (`jarvis_domain::plugin::PluginSourceIdentity`). The contract's
"Stable Identity and Provenance" section says `id`, `publisher`, package digest, signature identity,
source, version, and protocol "form the installed source identity" — and it is a **tuple** rather than an
`id` string for the reason `ToolIdentity` records: a name can be re-pointed, so a grant bound to `id`
alone would let a different package (new publisher, new bytes, new signature) inherit every grant the
original had. The display name is deliberately absent, per the contract's own rule that it never
identifies a plugin. The identifier/publisher slug rule (`is_plugin_identifier`) and the
`sha256:<hex>` package-digest rule (`is_canonical_package_digest`) live in this module and the manifest
validation **consults** them rather than holding a second copy.

**An update is a classification, not a comparison a policy re-derives.**
`SourceContinuity::classify` answers whether two source identities stand in the update relationship —
same publisher **and** same identifier **and** a strictly newer version. A different publisher is
`NoContinuity` even for the same identifier and a newer version, which is the impersonation case the
tuple exists to prevent; the same version is a replay and an older one is a downgrade, and neither is the
update the contract describes. Continuity is the *input* to the contract's grant-carry-forward policy,
not the policy itself: `TLS-015` still decides whether to carry grants, and can refuse even when
continuity holds (an expanded capability set, a changed schema). `PluginVersion` is ordered **numerically**
so `1.10.0 > 1.9.0`, and a pre-release sorts below its release (`1.0.0-rc.1 < 1.0.0`).

**The durable grant is a typed value** (`jarvis_domain::plugin::PluginGrant`), bound to the whole
`PluginSourceIdentity` rather than to `id`, plus the workspace and the granting principal — so a
replacement behind the same `id`, or a grant applied "by plugin" across workspaces, does not inherit it.
`PluginGrant::applies_at` answers all four conditions together (identity, workspace, principal, not
expired), and `names_identity_but_expired` tells the operator's "your grant expired" apart from "you have
no grant". Capability selectors are a validated type (`PluginCapabilitySelector`) whose `Deserialize`
**goes through the constructor**, so a selector that arrived over the wire is held to the same rule as one
this code built — required because a selector reaches a persisted grant.

**The carry-forward rule is answered in one method, and its direction is the crux.**
`PluginGrant::carry_forward_to` is the single place the contract's "existing grants carry forward only
under an explicit policy that proves publisher continuity and no capability/schema/effect expansion" is
decided: same workspace and principal, `SourceContinuity` is continuity, and
`grants_no_more_than` — a **subset** on every dimension. The first version of this code asked the
question backwards (a superset check), which carried a grant across exactly the update the rule exists to
stop; two tests falsified it. The schema dimension is carried by the whole identity, which
`carry_forward_to` compares separately, so it is not re-derived in the expansion check. Continuity remains
an *input*: `TLS-015` may still refuse a carry-forward even when all four conditions hold.

**Not implemented, and each is a named slice:**

- **package provenance and signature verification** (`TLS-014`). The `package` block carries the digest,
  the signature reference, and the source so a verifier has a typed value to check; **nothing in this
  module trusts them.** The contract's rule that "package bytes are verified before extraction or
  execution" is `TLS-014`'s, and the archive traversal/symlink rules (contract test 3) belong beside the
  extractor that would perform them.
- **process supervision** (`TLS-015`). The `resource_limits` block is validated here; enforcing a
  timeout, capturing bounded streams, quarantining a crash loop, and revoking a short-lived credential
  are the supervisor's, which does not exist.
- **installation, grants, and lifecycle** (`TLS-014`/`TLS-015`). The contract's lifecycle states and its
  "installation creates no grant" rule are prose here; no installer, no grant store, and no health
  transition writer exists. **`PluginManifest::parse` grants nothing** — it produces a validated
  document, which is exactly what the contract says a parsed manifest is. `PluginGrant` is the *shape* a
  grant store would persist and `carry_forward_to` the rule it would apply; **no store reads or writes it
  yet**, so the grant model is the contract's vocabulary with no producer and no consumer — scaffolding in
  AGENTS.md's sense, stated as such rather than implied to be wired.

