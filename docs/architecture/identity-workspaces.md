# Identity, Sessions, and Workspaces

Status: PROPOSED

## Principle

Identity is established at the gateway and carried as trusted request context.
The model, runtime, tool arguments, URL body, voice metadata, and provider
payload cannot select an arbitrary principal or workspace.

## Identity Types

- **User**: human account and policy subject.
- **Device**: enrolled client installation with revocable identity.
- **Session**: bounded authenticated interaction context.
- **Service client**: non-human client with scoped credentials.
- **Runtime instance**: supervised agent process identity.
- **Plugin/connector account**: external capability principal.

These identities are distinct even if one user owns all of them.

`User`, external identity, and device records are global principal roots so one
person/device can join multiple workspaces. They contain no workspace-private
content or capability by themselves. Memberships, active session scope, client
credentials, grants, policies, and every owned resource carry workspace scope;
authentication of a global principal never implies authorization in a workspace.

## Local Single-Owner Mode

Local mode creates one owner during onboarding. It still uses an authenticated
local transport so browser tabs, unrelated processes, and external agents are
not implicitly the owner.

Bootstrap flow:

1. First-run daemon creates owner and local device records.
2. It generates a high-entropy local bootstrap credential in owner-only state.
3. CLI/desktop proves local possession and exchanges it for a scoped session.
4. The bootstrap path can be rotated/revoked through recovery.
5. Remote requests cannot use the local bootstrap credential.

OS account identity is useful context and ACL protection, but not the sole API
authentication mechanism.

## Server Mode

Server mode requires a researched authentication implementation. The domain
supports:

- browser authorization-code flow with PKCE;
- device authorization where appropriate for CLI/headless use;
- service clients using asymmetric or securely stored credentials;
- short-lived access and revocable refresh/session state;
- MFA/step-up signals for high-risk approvals;
- external identity linking without making provider subject IDs global.

Do not build a custom password system casually. Select an identity strategy by
ADR after threat, deployment, and recovery requirements are known.

## Workspace Model

A workspace is the primary isolation boundary for:

- memories, entities, conversations, tasks, documents, and artifacts;
- connectors and external accounts;
- tools, plugins, runtimes, models, and budgets;
- workflows, proactive rules, and notifications;
- audit visibility and retention.

A user can belong to multiple workspaces with different roles/grants. A session
has an active workspace selected through an authorized server-side lookup.

## Authorization Model

Use capabilities over only coarse roles. Roles can bundle grants, while final
authorization considers:

- subject identity and assurance;
- workspace membership/status;
- action and resource type/ID;
- tool effect/risk and target account;
- resource ownership/classification;
- environment, channel, device, and network posture;
- standing policy, approval, and budget;
- time/expiry and explicit deny rules.

Explicit denies override grants. Authorization decisions have reason codes and
policy versions.

## Context Propagation

Trusted request context includes:

```text
request/correlation/trace IDs
principal and authentication method/assurance
device or service-client ID
session ID
active workspace ID
grants/policy snapshot reference
channel and origin
deadline and cancellation
```

Adapters receive only the subset they need. They never deserialize a model-
generated `workspace_id` into trusted context.

## Data Isolation

- Workspace predicates are part of repository queries and unique constraints.
- Cache keys include workspace and authorization-relevant principal/account.
- Embedding/vector search filters workspace before nearest-neighbor ranking.
- Object keys and signed URLs are workspace-scoped.
- External provider IDs are unique only within connector account/provider scope.
- Tests create same-named/same-shaped data in two workspaces to expose missing
  predicates.
- Server mode may add PostgreSQL row-level security as defense in depth.

## Sessions and Channels

Session types include CLI, desktop, browser, API, MCP, and voice. Each declares:

- authentication and expiry;
- active workspace and allowed workspace switching;
- channel capabilities and approval methods;
- inactivity and absolute timeout;
- retention and transcript/recording policy;
- bound device/client/call identity;
- revocation and disconnect behavior.

An approval on one channel is accepted only if policy permits that channel and
its current authentication assurance.

## External Agent/MCP Clients

External clients are service identities with explicit exports. Credentials bind
to:

- client ID and owner;
- workspace;
- allowed MCP protocol/transport;
- tool/resource allowlist;
- data, rate, concurrency, and cost budgets;
- issue/expiry/revocation state.

Client metadata and model name are untrusted display fields.

## Voice Identity

Caller ID is not authentication. Inbound voice can establish identity through
one or more of:

- pre-associated provider phone route plus limited baseline capability;
- signed one-time link or app handoff;
- DTMF/voice-independent challenge with rate limiting;
- explicit confirmation in an already authenticated JARVIS client;
- provider-specific verified metadata documented in evidence.

Until assurance is sufficient, the call is a guest/limited session and cannot
read private workspace content or perform side effects.

Every canonical call record is still associated with the workspace that owns
the configured inbound route. That association controls retention and audit; it
does not grant the guest principal workspace capabilities. Provider callbacks
that cannot resolve a configured route fail before creating an unscoped call.

## Recovery and Revocation

Recovery paths are security boundaries. They require:

- explicit owner verification appropriate to deployment mode;
- audit and user notification;
- ability to revoke all sessions/devices/service clients;
- local credential reset without deleting user data;
- server secret/key rotation and token invalidation;
- connector reauthentication without exposing old credentials.

`jarvis doctor` can diagnose identity state but does not silently weaken auth or
grant access as a repair.

## Tests

- forged principal/workspace fields;
- session fixation, expiry, revocation, and workspace switch;
- local browser origin/host attacks;
- duplicate external IDs across workspaces/accounts;
- cache/vector/object cross-workspace isolation;
- approval from wrong principal/channel/device;
- runtime/plugin token used outside its run;
- caller ID spoof and insufficient voice assurance;
- recovery and global revocation.