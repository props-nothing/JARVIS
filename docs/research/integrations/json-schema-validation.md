# Integration Evidence: JSON Schema 2020-12 Validation

Status: ACCEPTED
Owner: Tool fabric (`TLS-003`)
Last verified: 2026-09-30
Revalidate by: 2026-12-29
Implementation gate: PASSED
Review scope: Versioned specifications for Core, Validation, and the 2020-12 meta-schema; the official
test suite as the verification plan; and the MCP note that fixes the dialect. Gate PASSED for the
bounded subset in "Bounded subset" — no crate adopted, so no dependency review is owed, and the
refusal list is part of what passed.

## Decision Summary

- Purpose: validate a tool call's **arguments** against the schema its `ToolDefinition` declares, so
  `tool.schema_invalid` is reachable and an argument document the tool cannot accept is refused before
  the executor runs.
- JARVIS boundary: `jarvis_domain::tool` holds the definitions; validation is **pure** (a schema and a
  document in, a verdict out) and belongs with the fabric, not with any transport. The domain has no
  hashing or JSON dependency today, which is why `schema_fingerprint_of` lives in
  `jarvis-infrastructure` — the same boundary applies here.
- Proposed package or protocol version: **no crate adopted yet.** This note establishes the *format*
  and the design constraints; a validator implementation is a separate decision (see "Adoption
  decision" below).
- Supported deployment modes: any. Validation is in-process, offline, and needs no network.
- Explicitly unsupported: the full 2020-12 vocabulary. See "Bounded subset" — JARVIS validates the
  assertion keywords its own tool schemas use and **refuses** a schema containing a keyword it does
  not implement, rather than ignoring it.
- Kill switch or disable path: not applicable — validation has no external dependency and no state.

## Official Sources

| Source | URL | Version/date | Accessed | What it establishes |
| --- | --- | --- | --- | --- |
| Specification (Core) | <https://json-schema.org/draft/2020-12/json-schema-core> | 2020-12 | 2026-09-30 | The data model, `$ref`/`$defs`, applicator keywords, and the meta-schema URI |
| Specification (Validation) | <https://json-schema.org/draft/2020-12/json-schema-validation> | 2020-12 (draft-bhutton-json-schema-validation-01, 2022-06-16) | 2026-09-30 | The assertion keywords and their exact semantics — the vocabulary this note implements |
| Meta-schema | <https://json-schema.org/draft/2020-12/schema> | 2020-12 | 2026-09-30 | The `$vocabulary` set a dialect declares, and the deprecated aliases (`definitions`, `dependencies`) |
| Documentation index | <https://json-schema.org/understanding-json-schema/> | current | 2026-09-30 | Keyword-by-keyword guidance; the reference index for implementers |
| Test suite | <https://github.com/json-schema-org/JSON-Schema-Test-Suite> | current | 2026-09-30 | The official conformance vectors — named as the verification plan, **not yet vendored** |
| MCP evidence note | [mcp.md](mcp.md) | ACCEPTED | 2026-09-30 | Records that "tool input/output schemas currently use JSON Schema 2020-12 in the reviewed stable material" — the reason this dialect, and not another, is the target |

Attempted `llms.txt` URLs that did not exist:

- `https://json-schema.org/llms.txt` → **HTTP 404** at 2026-09-30. The gate's primary index is
  therefore absent for this provider, and the versioned specification page is used instead —
  the same substitution `release-signing.md` records for crates and `rfc8785-canonicalization.md`
  for an IETF spec.

## Version Matrix

| Component | JARVIS target | Documentation target | Compatibility status |
| --- | --- | --- | --- |
| Dialect | `https://json-schema.org/draft/2020-12/schema` | 2020-12 | VERIFIED — the dialect the MCP note records, and the one a 2020-12 tool schema declares |
| Validation vocabulary | `.../vocab/validation` | 2020-12 | VERIFIED for the subset in "Bounded subset" |
| Core vocabulary | `.../vocab/core` | 2020-12 | PARTIAL — `$defs` and `$ref` for local references only; no remote reference resolution |
| Format-annotation | `.../vocab/format-annotation` | 2020-12 | **NOT implemented as an assertion**, and the specification says that is the correct default: "the implementation MUST provide options to enable and disable such evaluation and MUST be disabled by default" |
| Format-assertion | `.../vocab/format-assertion` | 2020-12 | NOT implemented — OPTIONAL per the spec, and it requires full format support |

## Contract

### Authentication and Authorization

Not applicable: no credential, no endpoint, no account. The schema is trusted configuration that ships
with a tool definition; the **document being validated is untrusted model output**, which is the whole
reason this boundary exists.

### Transport and Lifecycle

Not applicable. In-process function call.

### Limits and Timeouts

- **The specification names the limit this design must impose**, under Security Considerations:
  "Regular expressions can often also be crafted to be extremely expensive to compute (with so-called
  'catastrophic backtracking'), resulting in a denial-of-service attack." JARVIS therefore:
  - bounds the schema and the argument document by bytes at the port (both already bounded:
    `MAX_ARGUMENT_BYTES`, and a definition's schema will be bounded when added);
  - **bounds validation by a step budget** — a keyword-evaluation counter that refuses rather than
    hangs, because a bounded *input* does not bound a backtracking *regex*;
  - validates with `pattern` **only where the schema is JARVIS-owned trusted configuration**, never as
    a general regex engine over model-supplied patterns.
- `$ref` resolution is **local only** (`#/$defs/...`). A remote `$ref` would make validation perform
  network I/O from inside a request path, which the architecture forbids ("Bind to loopback by
  default"; every external call is an adapter behind a port).

### Errors and Failure Semantics

| Condition | Answer |
| --- | --- |
| Document violates the schema | `tool.schema_invalid` — the contract's own code, already in the closed set |
| Schema itself is malformed | **Refused at definition review**, not at call time: a `ToolDefinition` with an unusable schema is a configuration defect, and reporting it as a caller error would blame the model |
| Schema uses a keyword this build does not implement | **Refused at definition review** — ignoring an unknown assertion keyword silently validates less than the author asked for, which is the fail-open direction |
| Document is not valid JSON | `tool.schema_invalid` (the domain's `ToolArguments` holds the text; parsing and validating are one step) |

## Bounded Subset

Implemented assertion keywords — the ones a JARVIS-owned tool schema needs:

`type`, `enum`, `const`, `minimum`, `maximum`, `exclusiveMinimum`, `exclusiveMaximum`, `multipleOf`,
`minLength`, `maxLength`, `pattern`, `minItems`, `maxItems`, `uniqueItems`, `items`, `prefixItems`,
`minProperties`, `maxProperties`, `required`, `properties`, `additionalProperties`, `propertyNames`,
`allOf`, `anyOf`, `oneOf`, `not`, `if`/`then`/`else`, `$defs`, `$ref` (local), `default`, `title`,
`description` (annotations, ignored for the verdict).

Explicitly **not** implemented, and refused if present: `format` as an assertion, `contentEncoding`,
`contentMediaType`, `contentSchema`, `unevaluatedProperties`, `unevaluatedItems`, `dependentSchemas`,
`dependentRequired`, remote `$ref`, `$dynamicRef`/`$dynamicAnchor`.

The refusal list is the important half: per the specification, an unknown keyword is *ignored* by
default, so a schema relying on one would validate **less** than its author intended — the direction
that admits a malformed call.

## Verification Plan

- **Contract tests**: the official `JSON-Schema-Test-Suite` vectors for the implemented subset,
  vendored under `crates/*/tests/` with the suite's `remotes/` excluded (no remote refs). Named here,
  not yet vendored.
- **Falsifications**: a document that violates each implemented keyword must be refused; a document
  that satisfies the boundary (`minLength` at exactly its value, `uniqueItems` with one element) must
  pass; a schema containing a refused keyword must be rejected at definition review.
- **Abuse case**: a schema with a nested `pattern` designed to backtrack must be refused by the step
  budget rather than hang — the specification's own Security Considerations section is the source for
  this requirement.
- **Property**: validation never mutates the document, and a verdict for `(schema, document)` is stable
  across runs.

## Adoption decision

This note establishes the format, the vocabulary boundary, and the failure semantics. **No crate is
adopted, and the routine-dependency ledger does not apply**: a JSON Schema validator models a
persisted, published document format (`input_schema` on a definition, the MCP wire shape), which fails
the ledger's "does not define a public wire type or persisted format" criterion — so a package choice
requires its own evidence section here naming the exact version, license, and transitive tree.

Two paths, neither taken yet:

1. **Implement the bounded subset directly** in `jarvis-domain` (or beside it, where a JSON dependency
   already exists). The subset above is small, the semantics are fully specified, and the alternative
   is a dependency whose recursion and regex behaviour JARVIS would have to bound anyway. This is the
   `rfc8785-canonicalization` precedent: implement the specification rather than adopt a crate, and
   record why.
2. **Adopt a validator crate** for the full vocabulary. That needs the version, license, transitive
   review, and a step-budget story this note does not yet have.

The first path is recorded as the recommendation **because the second cannot bound catastrophic
backtracking without the same budget the first would implement** — and because the tool schemas are
JARVIS-owned, so the vocabulary needed is the subset above, not the full 2020-12 surface.

## Changelog

| Date | Change | Sources |
| --- | --- | --- |
| 2026-09-30 | Initial note. Establishes the 2020-12 dialect, the implemented subset and the refusal list, the step-budget requirement from the specification's own Security Considerations, and the local-only `$ref` rule. No crate adopted; both adoption paths recorded with the reason the first is recommended. Records the `llms.txt` 404 and the substitution | Core and Validation specifications, the 2020-12 meta-schema, the JSON Schema Test Suite, and `mcp.md` |
