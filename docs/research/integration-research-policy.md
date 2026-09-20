# Integration Research Policy

Status: ACCEPTED

External APIs, protocols, SDKs, and hosted products change faster than JARVIS.
No integration may be implemented from model memory or a copied example. This
policy defines the evidence required before code is written and before an
integration is declared complete.

## Scope

This policy applies to:

- model and embedding providers;
- agent runtimes and protocols;
- MCP and ACP;
- voice, telephony, messaging, and realtime transports;
- OAuth providers and third-party identity systems;
- email, calendar, files, CRM, browser, home, and developer connectors;
- database drivers, vector stores, object stores, and workflow engines;
- desktop frameworks, installers, updaters, and OS service managers;
- any external crate or package whose behavior affects a public contract,
  security boundary, durable state, or supported platform.

Routine implementation-only libraries may use the
[dependency evidence ledger](dependencies.md) only when they satisfy every
eligibility criterion there. Uncertain classification requires a full evidence
note. Boundary-affecting infrastructure crates are not routine merely because
they are Rust libraries.

## Required Discovery Sequence

For each integration, perform these steps in order:

1. Try the official documentation root plus `/llms.txt`.
2. Try a section-specific `/llms.txt` for large documentation sites.
3. Follow the index to narrowly relevant Markdown pages. Some sites expose clean
   Markdown by appending `.md` to a page URL.
4. Locate the versioned specification, OpenAPI, AsyncAPI, JSON Schema, protobuf,
   or equivalent machine-readable contract.
5. Locate official SDK repositories, examples, release notes, changelog,
   migration guides, deprecation policy, security guidance, and rate limits.
6. Compare the current docs with the exact dependency version proposed or
   pinned in JARVIS.
7. Search official repository tests when prose leaves lifecycle, streaming,
   cancellation, or failure behavior ambiguous.
8. Record the evidence before writing integration code.
9. Update [the evidence manifest](evidence-manifest.json) only after the note is
  complete; `ARCHITECTURE_ONLY` evidence does not permit implementation.

If no `llms.txt` exists, record the attempted URL and use the official docs
navigation. Its absence is not permission to rely on third-party summaries.

## Evidence Note

Create `docs/research/integrations/<slug>.md` from
[the evidence template](integration-evidence-template.md).

At minimum, prove:

- official ownership of each source;
- date accessed and documented API/spec/SDK versions;
- supported transports, endpoints, event shapes, and streaming termination;
- authentication and authorization flow;
- secret placement and redaction requirements;
- scopes and least-privilege options;
- quotas, rate limits, payload limits, retention, and pricing assumptions that
  influence design;
- retryable versus terminal errors;
- idempotency and webhook verification semantics;
- ordering, duplication, cancellation, reconnect, and resume behavior;
- platform limitations and data residency/privacy constraints;
- deprecations, beta features, and known incompatibilities;
- a contract-test fixture or live-test plan that can falsify each key claim.

Quotes should be short. Prefer a precise paraphrase plus a URL. Do not copy
large sections of upstream documentation into this repository.

## Evidence Labels

Use these labels on material claims:

- `VERIFIED`: confirmed by a current official specification, schema, or live
  test against the pinned version.
- `DOCUMENTED`: stated in current official docs but not yet exercised by JARVIS.
- `OBSERVED`: seen in a captured real response with date and environment.
- `INFERRED`: reasoned from evidence but not promised by the provider.
- `UNVERIFIED`: plausible but unsupported; must not drive production behavior.
- `STALE`: source predates the supported version and needs revalidation.

Claims can carry more than one label, such as `DOCUMENTED, OBSERVED`.

## Implementation Gate

Before the first adapter edit, the pull request or work log must name:

1. the evidence note;
2. the exact contract version targeted;
3. one falsifiable local hypothesis;
4. the cheapest contract check;
5. known unsupported behavior;
6. the rollback or disable path.

The evidence note must say `Implementation gate: PASSED`, target exact versions,
and have a non-expired `Revalidate by` date. Its manifest entry must be
`IMPLEMENTATION_READY` with `implementation_ready: true`. Changing only the
manifest is invalid; documentation validation cross-checks note metadata.

If a changed file matches an `implementation_paths` pattern in the manifest,
the integration must be ready. During local work, pass changed paths to:

```text
node scripts/validate-docs.mjs --changed-file <path>
```

CI must pass all changed paths. Until CI exists, the implementation agent and
reviewer run the same command explicitly and record it as evidence.

Every changed package manifest, lockfile, or Rust toolchain file must be passed
in the same validator invocation as all changed evidence files. A package/
lockfile change requires either a changed routine dependency ledger or a changed
`IMPLEMENTATION_READY` full evidence note plus manifest. A Rust toolchain change
requires the changed implementation-ready `rust-foundation` note plus manifest.
Prior Foundation approval is never blanket approval for a later dependency.

The first vertical slice should use a captured or live provider payload. A
synthetic fixture only proves that the implementation agrees with itself.

## Review Gate

An integration cannot be marked `DONE` until it has:

- deterministic unit tests for JARVIS-owned normalization and policy;
- contract tests against official schemas or captured provider payloads;
- a gated live test for auth, discovery, and one representative operation;
- negative tests for invalid auth, insufficient scope, malformed input,
  provider errors, timeout, cancellation, and rate limiting;
- diagnostics that expose safe, actionable state without leaking secrets;
- setup, reauthentication, disable/unload, migration, and deletion behavior;
- operator documentation and a support-safe redacted diagnostics path.

Live tests must be opt-in and skip clearly when credentials are absent. They
must never spend money, send communication, mutate production data, or start a
phone call without an explicit test account and confirmation gate.

## Revalidation

Revalidate an evidence note when:

- upgrading the SDK or API version;
- a provider announces a breaking change or deprecation;
- a contract test changes unexpectedly;
- the note is older than 90 days for beta/realtime APIs or 180 days for stable
  APIs;
- auth, scopes, retention, pricing, or security behavior affects the change;
- a production incident contradicts the recorded assumptions.

Update `last_verified` and summarize the delta. Preserve prior conclusions when
they explain migrations or compatibility decisions.

## Useful AI Documentation Patterns

Known official sites may expose one or more of:

```text
https://provider.example/llms.txt
https://provider.example/docs/llms.txt
https://provider.example/docs/section/llms.txt
https://provider.example/page.md
https://provider.example/openapi.json
https://provider.example/asyncapi.json
```

These are discovery conventions, not universal standards. Confirm that the
domain is official before trusting the content.

## Conflict Handling

When official sources disagree:

1. Prefer a versioned normative specification over prose.
2. Prefer a machine-readable schema for wire shape, but check documented
   semantics that schemas cannot express.
3. Prefer the SDK version's source and tests for SDK behavior.
4. Capture a real response when permitted.
5. Record the contradiction and fail closed until resolved.

Never silently choose the source that makes implementation easiest.