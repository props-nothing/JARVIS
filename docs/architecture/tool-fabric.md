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

**Every field of a definition is validated on the way in as well as where it is constructed.** A
definition may arrive as a document — a persisted catalog, an MCP server's list result — so a type that
enforces its rules only in a constructor has them enforced only for values this process built, and the
derived `Deserialize` is the door that skips the constructor. `ToolDefinition` and each of the field
types it composes (`ToolSource`, `DataClasses`, `ExecutionDefaults`, `ToolIdentity`, `ToolResultBody`,
`ToolCallIntent`, `PathGrant`, `ReservationKey`, `RetryPolicy`) hand-write `Deserialize` to go through
their own constructor, so "a definition is always valid" is a property of the type rather than a claim
about the callers. The counterpart rule matters as much: a type read **only** through a `parse` that
validates and returns a typed error keeps its derived impl, because a custom one would collapse the
specific error into `serde`'s generic form — a validating path is required, and a *second*, unvalidated
one is the defect.

### Argument Validation

`input_schema` is validated by `jarvis_infrastructure::tool_schema`, which
implements a **bounded subset of JSON Schema 2020-12** and refuses any schema
containing a keyword outside it. The refusal is the security property rather than a
limitation: the specification says an unrecognised keyword "SHOULD be treated as an
annotation" ([Core §6.5](https://json-schema.org/draft/2020-12/json-schema-core)),
so a schema relying on a keyword JARVIS ignored would validate *less* than its author
asked for while reporting success. A refusal at definition review is an
operator-visible configuration defect; an ignored assertion admits a malformed call.

The unimplemented keywords are refused **by name** so the reason is visible, and two
are worth stating because they look like omissions: `pattern` and `patternProperties`
need a reviewed regular-expression engine (the specification's own Security
Considerations section names catastrophic backtracking as a denial-of-service risk),
and `multipleOf` needs exact decimal arithmetic that `f64` cannot provide. `$ref`
resolves against the schema's own `$defs` only — a remote reference would make
validation perform network I/O from inside a request path.

Two resource bounds are enforced, each required by a specification: a **step budget**
charged per keyword and per `uniqueItems` comparison, and a **depth bound** covering
schema descent, instance descent, and `$ref` hops. Both specifications require the
latter by name ("Validators MUST NOT fall into an infinite loop", Core §13). A refused
budget is reported as *this validator's* limit rather than as a schema violation, so a
limit of JARVIS's is never attributed to the caller's arguments.

The definition's stated `schema_fingerprint` must be the fingerprint of the schema that
is actually enforced — `ToolSchema::confirms` is the check, and it matters because the
fingerprint is part of the identity an approval is recorded against. A definition
carrying one schema's fingerprint while a looser schema decided acceptance would let a
recorded approval survive a change to the rules that decide what is accepted, which is
the failure `ACC-024` names reached without the display name ever changing.

The evidence note is
[json-schema-validation.md](../research/integrations/json-schema-validation.md).

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

### Grants Are Configuration, Not Code

**A grant is a durable row an operator writes, and this section exists because it was not always so.** For
several milestones the only grant source was a constructor in `jarvis-infrastructure`, so the answer to "may
this principal use this tool without asking?" was compiled in — and a user who wanted "never ask about the
clock, always ask about outbound mail" had no way to say it. The store is `tool_grants` (plus
`tool_deny_rules`), reached through `ToolGrantRepository` and served at `/api/v1/tool-grants`.

Four properties of it are load-bearing rather than incidental:

- **A grant may only narrow.** It confers a subset of the tool's declared scopes and effects, and its risk and
  sensitivity ceilings must not exceed the definition's. A grant *wider* than the tool is refused, because it
  would sit in the store looking like permission for something the tool cannot do. The narrowing direction is
  the ordinary one: "let this read `~/notes`, not the whole home directory" is exactly what a grant is for.
- **The key is a capability, not an identity.** `ToolIdentity` includes the schema fingerprint, so a grant
  keyed by identity would read as *absent* after a tool was recompiled rather than as *replaced* — and those
  two lead an operator to different next steps, which is why `resolve_grant` distinguishes them. The stored
  identity is compared, never used as the key.
- **Revocation is a state, not a delete.** A revoked row stays readable, because the approval contract
  requires that revoking "cannot erase historical audit" — and the evaluator's read filters it in the query,
  so a withdrawn grant cannot reach policy through a code path that forgot to check.
- **A store failure is an error, not a fallback.** Answering "use the defaults" for an unreachable store would
  re-authorize a principal an operator had narrowed. The reviewed grants are the posture of an *unconfigured*
  principal, and one stored grant replaces them for that principal — which is what makes the store
  authoritative rather than a source of extra permissions.

**The tool pipeline consults this store on every dispatch**, through the same `ToolGrantSource` port it has
always used. The reviewed grants remain as the unconfigured posture so a fresh profile is not a profile that
refuses its own read-only tools; see `StoredGrants` for why an empty store falls back and why a *failure* does
not.

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

The fingerprint is computed rather than asserted: `jarvis_domain::tool::canonical`
produces the RFC 8785 canonical form of a versioned envelope — a flat object whose
values are all strings, which is what makes the JCS number-serialization algorithm
(unspecified by the RFC) unnecessary rather than merely unimplemented — and the
SHA-256 that turns it into a `sha256:<hex>` digest lives in
`jarvis_infrastructure::tool_fingerprint`. The evidence note is
[rfc8785-canonicalization.md](../research/integrations/rfc8785-canonicalization.md).

The envelope covers the **effects and the risk separately from the tool identity**,
because `ToolIdentity` fingerprints the input schema alone: a tool reclassified from
reading to deleting keeps its identity, so a fingerprint built from the identity would
let an approval granted for the read authorize the delete. The effects are canonicalized
as a set, so one effect set has one fingerprint.

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

**That rule is now a requirement rather than a convention, and it is the invariant
the whole skill ecosystem rests on.** Letting a skill *narrow* the catalog is a
useful optimization — a procedure names the tools it needs, so the rest can be
dropped from the call. Letting it *widen* the catalog is a privilege escalation
dressed as convenience, and it is the single most dangerous thing a skill or
plugin ecosystem can carry. The narrowing rule is therefore asserted against a
skill that declares the expansion, not only against a well-behaved one
(`ACC-085`), because a rule that is only checked on conforming inputs is not
checked at all.

### Authoring, Trust, and Supply Chain

A skill that JARVIS writes for itself and a skill it installs from a registry are
the same *shape* and different *trust*. A skill carries a `source` and a trust
tier, and the tier — not a field the skill declares — decides the install policy:

| Tier | Origin | Install policy |
| --- | --- | --- |
| `builtin` | Ships with JARVIS | Always trusted; scanned for completeness |
| `official` | Published by the JARVIS project | Built-in trust; no third-party warning |
| `trusted` | Reviewed registries the project pins | More permissive; dangerous findings still block |
| `community` | Any other user-installed source | Full scan; a caution finding is overridable only by a recorded user override |
| `learned` | Authored by an agent run | Deny-by-default; requires the learning grant and, by policy, review |

A **dangerous** scan verdict is never overridable at any tier. The override exists
for caution-level findings a user has read, not for a verdict naming exfiltration,
injection, or destructive commands. An installed skill records its resolved source,
content hash, scanner version, findings, and timestamp in a lockfile; a skill that
cannot be scanned is not installed, and a dangerous verdict **quarantines** the
skill — inert, on disk, inspectable, absent from the index — rather than deleting
it silently.

### Binding to an Implementation, Not a Name

A skill's `content_hash` covers every file it can load, not only its entry body,
because a reference file a procedure loads on demand is part of the procedure. A
grant or approval recorded against a skill names that hash and the **schema
fingerprints** of the tools the skill declares. A changed hash or capability set
requires a new grant; the old one does not carry forward. This is the same rule
`ACC-024` applies to a tool — an approval binds to the implementation, not to a
name that can be re-pointed — lifted one level, where the "implementation" is a
procedure and its tools together.

### Progressive Disclosure and the Context Manifest

A skill loads in three levels, and each is a separate context-manifest entry so
the budget explains itself:

```text
Level 0  index      { name, description, tier, declared capabilities }   one line each
Level 1  body       the full procedure, loaded when a task needs it
Level 2  reference  one named file under the skill's references/, on demand
```

The index is the only level present in every prompt. Loading level 1 or 2 appends a
manifest entry naming the skill and its token estimate. **A system-prompt mutation
that the manifest does not record is a defect**, because the manifest is how a
reader answers "why was this in the context". A skill whose body exceeds a bounded
size is split or refused rather than loaded whole, since a body loaded once stays
in context for the remainder of the run.

The full wire and persistence shape — including the learning write pipeline, the
review job's budget, and revocation — is
[the skill contract](../contracts/skill-contract.md), decided by
[ADR-0012](../adr/0012-governed-learning-loop.md).


## Tool Catalog Selection

Do not send hundreds of tool definitions to every model call. Selection is:

1. filter by principal/workspace grants and current availability;
2. filter by task-required capabilities and runtime support;
3. rank relevant tools using trusted metadata;
4. enforce a schema/token budget;
5. record why each tool was exposed.

Selection affects model context, not authorization at execution time.

## Required Tests

- schema rejection before adapter invocation; **implemented** — the refusal happens at
  definition review (`ToolSchema::parse`), so an unusable schema cannot reach a call at
  all, and 30 tests plus 15 falsifications cover the load-time refusals;
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