# Tool, MCP, Plugin, and Skill Fabric

Status: PROPOSED

## One Execution Path

Every action uses the same governed pipeline, regardless of origin.

```mermaid
flowchart LR
    Intent[Model/runtime/user tool intent]
    Resolve[Resolve canonical tool]
    Validate[Validate schema and context]
    Authorize[Identity/workspace authorization]
    Policy[Effect, risk, budget, policy]
    Approval[Approval if required]
    Idem[Reserve idempotent call]
    Execute[Execute bounded adapter]
    Verify[Validate/classify result]
    Persist[Persist outcome and audit]
    Observe[Return bounded observation]

    Intent --> Resolve --> Validate --> Authorize --> Policy
    Policy --> Approval --> Idem --> Execute --> Verify --> Persist --> Observe
```

There is no fast path for a native tool, trusted runtime, admin UI, voice call,
or MCP server.

## Canonical Tool Definition

A definition contains:

```text
id                    stable namespace/name and major version
display metadata      user-facing name and concise purpose
input_schema          JSON Schema dialect and schema
output_schema         optional but strongly preferred
effects               read, write, communication, destructive, code, financial,
                      privileged, physical
risk                   low, moderate, high, critical
required_scopes       capability/resource scopes
approval_policy       default policy hint, not authorization
timeout/retry         bounded defaults
idempotency           none, caller-keyed, or naturally idempotent
data_classes          input/output sensitivity
source                native, connector, MCP server, runtime, plugin
source_identity       immutable owner/version/provenance
availability          health and workspace/account prerequisites
```

Descriptions and MCP annotations are untrusted hints. JARVIS derives effect and
risk classifications from trusted manifests, built-in policy, and owner review.

## Stable Identity

Tool names are human-readable aliases; authorization binds a canonical tool ID,
source identity, schema fingerprint, workspace, and resource scope. Discovery
cache changes cannot retarget an existing approval to a different implementation.

Breaking input/effect changes require a new major tool identity. Compatible
description changes do not.

## Policy Decision

Inputs include:

- authenticated principal/device/service client;
- workspace and resource ownership;
- tool identity, source, effects, and schema fingerprint;
- validated arguments and normalized target summary;
- run/runtime/voice/channel origin;
- current grants, deny rules, budgets, quiet hours, and environment;
- prior exact approval or standing policy;
- prompt-injection and untrusted-data provenance indicators.

Outputs are `ALLOW`, `ASK`, or `DENY` with machine reason codes, human summary,
constraints, expiry, and audit metadata. The model cannot modify the decision.

## Approval Binding

An approval request contains a safe preview and an action fingerprint over:

- principal and workspace;
- canonical tool/source/schema identity;
- normalized arguments and target resources;
- material content hash for communication or writes;
- effect/risk classification;
- expiry and one-shot/standing scope.

Argument changes invalidate approval. Approving "send this email" does not
approve a rewritten recipient, subject, body, attachment, or account.

## Execution

The executor:

1. atomically reserves the call/idempotency key;
2. resolves secret references inside the adapter boundary;
3. applies deadline, cancellation, rate, concurrency, network, and sandbox policy;
4. records attempt state before the external side effect;
5. classifies provider response and ambiguous outcomes;
6. validates and bounds output;
7. persists result and outbox/audit event before reporting success.

Ambiguous side effects enter reconciliation; they are not blindly retried.

## MCP Roles

### Client/Host

JARVIS connects to local stdio and remote Streamable HTTP MCP servers, negotiates
protocol/capabilities, and translates discovered tools into canonical definitions.

Discovery cache keys include server configuration identity, authenticated
principal, workspace/account, protocol version, capability set, and list-result
version/TTL. Invocation revalidates current ownership and permission.

### Server

JARVIS exposes a selected projection of canonical tools to external clients.
Each client has:

- authenticated identity and revocable credential;
- workspace binding;
- explicit tool/resource allowlist;
- rate, concurrency, data, and cost budgets;
- protocol/extension capability policy;
- independent audit and kill switch.

Remote clients never receive an implicit "all current and future tools" grant.

### Protocol Evolution

Implement the current stable official MCP version through the official Rust SDK
when its pinned release supports the required features. Negotiate versions and
extensions; do not hardcode a draft assumption. Keep MCP DTOs at the adapter
boundary because MCP features and deprecations evolve independently of JARVIS.

## Plugins

Primary plugin mechanisms are process-safe protocols:

- MCP for tools/resources/prompts;
- versioned JARVIS runtime protocol for agent runtimes;
- HTTP/webhook for remote connectors;
- future WASI components for tightly sandboxed local extensions.

Native dynamic libraries are not the default due to ABI, crash, privilege, and
upgrade risks.

A plugin manifest declares publisher/provenance, version, checksums/signature,
entrypoint, protocol, supported OS/architectures, configuration schema,
capabilities, requested permissions, network/filesystem needs, compatibility,
and health command. Installation never grants requested permissions implicitly.

## Skills

Tools are atomic capabilities. Skills are versioned orchestration instructions
or workflow templates that compose tools. A skill contains:

- identity, version, owner, and provenance;
- purpose, inputs, outputs, and preconditions;
- required capabilities and risk summary;
- instructions or deterministic workflow definition;
- tests/evaluations and compatibility;
- no embedded secret values.

Loading a skill can narrow the relevant tool catalog. It cannot widen the user's
grants or bypass approval.

## Tool Catalog Selection

Do not send hundreds of tool definitions to every model call. Selection is:

1. filter by principal/workspace grants and current availability;
2. filter by task-required capabilities and runtime support;
3. rank relevant tools using trusted metadata;
4. enforce a schema/token budget;
5. record why each tool was exposed.

Selection affects model context, not authorization at execution time.

## Required Tests

- schema rejection before adapter invocation;
- tool-name and source-identity collision;
- stale discovery cache and source replacement;
- argument mutation after approval;
- approval expiry/rejection/restart;
- idempotent duplicate submission and ambiguous provider outcome;
- timeout/cancel while child or remote call is active;
- oversized/malformed/prompt-injected output;
- secret and sensitive-data redaction;
- MCP auth, negotiation, capability downgrade, progress, cancellation, and
  transport parity;
- plugin crash, hang, restart budget, quarantine, and uninstall cleanup.