# Integration Evidence: JSON Canonicalization (RFC 8785 / JCS)

Status: ACCEPTED
Lifecycle: ACTIVE
Owner: Tool and Runtime Platform
Last verified: 2026-09-27
Revalidate by: 2027-03-26
Implementation gate: PASSED

## Decision Summary

- Purpose: compute the **action fingerprint** the
  [approval contract](../../contracts/approval-contract.md) requires, so that
  approving one action cannot authorize a different one. The contract's own words
  are "use a researched deterministic JSON canonicalization such as RFC 8785 and
  SHA-256, encoded with an explicit algorithm prefix", and its binding rule is
  "approving 'send this email' does not approve a rewritten recipient, subject,
  body, attachment, or account".
- JARVIS boundary: `jarvis_domain::tool::canonical` owns the canonical form;
  `jarvis_infrastructure` owns the SHA-256 that turns it into a digest, because the
  domain layer must not depend on a hashing crate. The domain half is a **pure
  function of values** with no I/O, no clock, and no environment, so the
  canonicalization is exhaustively testable without an adapter.
- Proposed package or protocol version: **RFC 8785 (JCS), June 2020**, implemented
  directly against the specification. **No crate is adopted.** The `sha2` crate that
  produces the digest is already resolved at `0.10.9` and already reviewed for this
  boundary-affecting use (see the `rust-foundation` note).
- Supported deployment modes: local daemon and server profiles, identically. The
  canonicalizer is deterministic and platform-independent by construction, which is
  the property that makes a fingerprint comparable across two JARVIS processes and
  across a Rust client that previews the same action.
- Explicitly unsupported: **JSON number, `true`/`false`/`null` literals, and nested
  objects inside the fingerprinted document.** The fingerprinted input is a flat
  object whose values are **all strings**, so the JCS number-serialization algorithm
  is neither implemented nor reachable — see "Security Analysis" and "Falsifiable
  Claims". Also unsupported: the RFC's ECMAScript reference implementation,
  `BigInt`/`DateTime` string subtypes (Appendix E), and any cross-language vector
  suite for languages JARVIS does not ship.
- Kill switch or disable path: `jarvis_domain::tool::canonical::canonical_object`
  and `jarvis_infrastructure`'s `action_digest_of` are the only two functions
  involved, and the fingerprint is **compared, never trusted as authorization** —
  policy and the approval record still decide. Reverting the computation to the
  previous "digest compared but never produced" state is a two-file change and
  restores the recorded gap without weakening any other control.

## Official Sources

| Source | URL | Version/date | Accessed | What it establishes |
| --- | --- | --- | --- | --- |
| `llms.txt` | none published | N/A | 2026-09-27 | Recorded honestly rather than omitted — see the attempts below |
| Normative specification | `https://www.rfc-editor.org/rfc/rfc8785.txt` | RFC 8785, June 2020 | 2026-09-27 | The whole canonical form: no whitespace (§3.2.1), string serialization (§3.2.2.2), property sorting (§3.2.3), UTF-8 generation (§3.2.4) |
| Property-sorting test vector | RFC 8785 §3.2.3 | RFC 8785, June 2020 | 2026-09-27 | The seven-key sample and its **expected order**, which the implementation's sort is asserted against verbatim |
| Number-serialization samples | RFC 8785 Appendix B | RFC 8785, June 2020 | 2026-09-27 | The IEEE 754 edge cases, read **to justify refusing numbers** rather than to implement them |
| Normative reference | `https://www.rfc-editor.org/rfc/rfc8259` (JSON) | RFC 8259, STD 90 | 2026-09-27 | Which characters require escaping in a JSON string, and that `NaN`/`Infinity` are not JSON |
| Normative reference | `https://www.rfc-editor.org/rfc/rfc7493` (I-JSON) | RFC 7493, March 2015 | 2026-09-27 | The input constraint JCS builds on: no duplicate property names, values expressible as IEEE 754 doubles |
| Normative reference | `https://www.ecma-international.org/ecma-262/10.0/index.html` §24.3.2.2 | ECMA-262, 10th edition, June 2019 | 2026-09-27 | The string-serialization table the RFC defers to: `\b \t \n \f \r`, lowercase `\uXXXX`, `\\` and `\"` |
| Existing digest primitive | `https://docs.rs/sha2/0.10.9/sha2/` | sha2 0.10.9 | 2026-09-27 | The SHA-256 API already used by the credential verifier and the diagnostics manifest |

Attempted `llms.txt` URLs that did not exist:

- `https://www.rfc-editor.org/llms.txt` and `https://www.ietf.org/llms.txt`
  return **HTTP 404**. The RFC Editor and the IETF publish no AI-readable index.
- **This is a real gap in the research gate for specifications**, the same one the
  `release-signing` note recorded for crates: `AGENTS.md` asks for an official
  `llms.txt` first, and an IETF standard has none. The substitute is *stronger*
  than an index would be — RFC 8785 is a single versioned document with a stable
  URL, and the two sections this implementation depends on (§3.2.2.2 and §3.2.3)
  are quoted into the implementation's doc comments so a reader does not have to
  re-fetch the RFC to check the rules.

## Version Matrix

| Component | JARVIS target | Documentation target | Compatibility status |
| --- | --- | --- | --- |
| JCS canonical form | RFC 8785, June 2020 | RFC 8785, June 2020 | **VERIFIED for the string-only subset**; numbers explicitly out of scope |
| JSON | RFC 8259 | RFC 8259 | VERIFIED — the output is valid JSON |
| I-JSON | RFC 7493 | RFC 7493 | VERIFIED — no duplicate keys by construction (`BTreeMap`), no numbers |
| SHA-256 | `sha2` `0.10.9` (pinned in `Cargo.toml`) | `docs.rs/sha2/0.10.9` | VERIFIED — already resolved and reviewed |
| Digest algorithm prefix | `sha256:`, a JARVIS convention | `docs/contracts/approval-contract.md` | VERIFIED — mirrors the existing `SchemaFingerprint` |

## Contract

### Authentication and Authorization

Not applicable: RFC 8785 is a pure serialization algorithm. It performs no network
access, holds no credential, and has no authorization concept. The fingerprint it
produces is **not** an authorization answer — the approval contract is explicit
that the server derives the deciding principal and channel and that policy decides,
and the fingerprint only binds a decision to an exact action.

### Transport and Lifecycle

Not applicable. There is no endpoint, no connection, and no lifecycle. The
canonicalizer is called once per fingerprint computation, in-process.

### Data and Limits

- **Input shape:** a flat JSON object whose values are **all strings**. The type
  that carries it (`FingerprintInput`) has no field through which a number, a
  boolean, `null`, an array, or a nested object could arrive, which is the
  structural reason this implementation has no number serializer.
- **Covered fields:** `fingerprint_version`, `principal`, `workspace`,
  `tool_capability`, `tool_source`, `schema_fingerprint`, **`effects`**, **`risk`**,
  and `arguments`.
  - **`effects` and `risk` are covered separately from the identity, and that is a
    correction.** `ToolIdentity::schema_fingerprint` covers the **input schema
    only**, so a tool reclassified from `read_only` to `destructive` keeps its
    identity — and a fingerprint built from the identity alone would keep the
    *fingerprint* too, so an approval granted for the read would authorize the
    delete. The contract requires the fingerprinted object to contain "effects and
    constraints", and the tool fabric names "effect/risk classification" in what an
    approval binds to. Both are therefore envelope fields, and both were
    **falsified** by omitting them.
  - The effects list is **sorted and deduplicated through the contract spellings**,
    so the same *set* produces one digest however the caller's list was ordered or
    whether it repeated an entry. The list is not trusted to be canonical even
    though `ToolDefinition::new` refuses duplicates: a fingerprint that varied with
    list order would refuse an approval for the action the user actually reviewed.
    Falsified by concatenating the spellings instead of joining them.
- **Output:** the canonical document as a `String`, unescaped except where JSON
  requires it, with no whitespace, encoded as UTF-8.
- **Bounds:** every contributed value is already bounded by the type that produced
  it (tool capability ≤ 64 bytes per segment, source reference bounded, schema and
  action digests `sha256:<64 hex>`, arguments bounded by `MAX_ARGUMENT_BYTES`). A
  fingerprint is computed over bounded inputs, so its own size is bounded.
- **Sorting:** property names sorted as arrays of UTF-16 code units, compared as
  unsigned integers, independent of locale (§3.2.3).

### Errors and Retries

- A digest that is not `sha256:` plus 64 lowercase hex characters is **refused at
  parse**, not normalized, so two spellings never denote one digest.
- The canonicalizer cannot fail: its input type admits only values it can serialize.
  A function that could fail would need a caller to decide what to do, and there is
  no sensible recovery from "the canonical form could not be produced".
- No retry semantics. The computation is pure and deterministic.

## Security Analysis

### The defect this closes

`TLS-005` recorded that the action fingerprint was **a type with no computation**:

> the fingerprint is a type without a computation, so this stays `~`

and the approval contract's own Implementation Status repeated it:

> The contract requires RFC 8785 deterministic canonicalization; there is no
> evidence note for it, so the digest is **compared and never produced**. This
> layer verifies nothing about how a presented fingerprint was computed.

That was the honest state, and it is a real hole: `ApprovalService::decide`
compared `stored.action_digest != fingerprint` against a value the caller supplied
and nothing had ever derived. A client could therefore *assert* the digest, and the
binding "the approval covers this exact action" rested on the caller's honesty. The
contract's rule — "Argument changes invalidate approval" — was enforced against a
value the argument-changes-nothing path could forge.

### Why numbers are refused rather than implemented

This is the load-bearing security decision in this note, and it is a **narrowing**
rather than a shortcut.

RFC 8785 §3.2.2.3 defers ECMAScript number serialization to ECMA-262 §7.1.12.1 and
then says outright:

> Due to the relative complexity of this part, the algorithm itself is **not
> included in this document**.

Appendix B is a table of samples, not an algorithm. A conformant number serializer
therefore requires shortest-round-trip double formatting (the Ryu/Grisu family) plus
ECMAScript's exponent and `-0` rules. Getting it subtly wrong produces a digest that
is **stable within one build and different from every other implementation** — the
worst outcome available, because every JARVIS-internal test would pass and a
Rust client previewing the same action would compute a different fingerprint and be
refused. It is the failure the contract names when it requires the implementation to
"round-trip test across Rust and any client that previews/verifies fingerprints".

Since the fingerprinted document is **JARVIS's own envelope** (not arbitrary JSON),
the number problem is removable by construction: every value contributed is a string.
Identifiers, tool capabilities, source references, schema and action digests, and the
argument document are all text, and the argument document is carried **as text**
rather than re-parsed, which also means the fingerprint binds the *bytes the user
reviewed* rather than a re-serialization of them. So:

- there is no floating-point hazard, because there is no number path;
- the input type makes a number **unrepresentable**, so the property is structural
  rather than a documented convention that a later edit could break;
- the remaining rules — no whitespace, string escaping, UTF-16 property sort, UTF-8
  output — are each fully specified, and the RFC supplies a test vector for the one
  that is easy to get wrong.

The cost is honest and bounded: the canonicalizer **cannot** canonicalize an
arbitrary third-party document. That is recorded as unsupported above and in the
`TLS-005` TODO, and it is why no general-purpose JCS function is exported.

### Sorting by UTF-16 code units, not by bytes

§3.2.3 requires property names to be sorted as arrays of **UTF-16 code units**,
"independent of locale settings", and notes that sorting UTF-8 bytes would produce a
*different* order for the RFC's own sample and be incompatible. For pure ASCII keys
the two agree, and an implementation that sorted bytes and called it JCS would look
correct on every JARVIS key until the day a key contained a non-ASCII character — at
which point two processes would disagree about one fingerprint. The implementation
encodes each name to UTF-16 and compares, which is exact rather than lucky, and a
test asserts the RFC's sample order including a non-ASCII key.

## Normalization Map

| RFC 8785 requirement | JARVIS implementation | Where |
| --- | --- | --- |
| §3.2.1 no whitespace between tokens | Emitted directly with no separators beyond `,` and `:` | `canonical_object` |
| §3.2.2.2 control characters as lowercase `\u00xx` | `escape_string`, lowercase hex | `canonical_object::escape_string` |
| §3.2.2.2 `\b \t \n \f \r` short forms | The five short escapes | `escape_string` |
| §3.2.2.2 `\\` and `\"`; everything else as-is | Escaped first, then passthrough of other code points | `escape_string` |
| §3.2.2.3 number serialization | **Not implemented — numbers are unrepresentable in the input** | `FingerprintInput` |
| §3.2.3 recursive property sort by UTF-16 code units | `BTreeMap` keyed on the name, with an explicit UTF-16 comparison | `canonical_object` |
| §3.2.4 UTF-8 output | A Rust `String`, which is UTF-8 | `canonical_object` |
| §3.2.2.3 `NaN`/`Infinity` must abort | **Unreachable**: no number can be contributed, so neither can arrive | `FingerprintInput` |
| §3.1 no duplicate property names | Structural: a `BTreeMap` cannot hold a duplicate key | `canonical_object` |

## Falsifiable Claims

| ID | Claim | Evidence type | Falsifier |
| --- | --- | --- | --- |
| `JCS-C001` | The canonical form contains no whitespace between tokens | VERIFIED by unit test | A test finds a space or newline in the output |
| `JCS-C002` | Property names sort by UTF-16 code units, matching the RFC's own expected order | VERIFIED by unit test | The RFC §3.2.3 sample produces a different order |
| `JCS-C003` | A control character is emitted as lowercase `\u00xx` and the five short forms are used | VERIFIED by unit test | `\u000F`-style output is uppercase, or `\n` is emitted as `\u000a` |
| `JCS-C004` | `"` and `\` are escaped and no other code point is altered | VERIFIED by unit test | A literal `"` reaches the document unescaped |
| `JCS-C005` | A number, boolean, `null`, array, or nested object **cannot** be contributed | VERIFIED by type + test | A field accepting a non-string value compiles |
| `JCS-C006` | Canonicalization is deterministic: the same input twice is byte-identical | VERIFIED by unit test | Two calls differ |
| `JCS-C007` | Input field *order* does not affect the output | VERIFIED by unit test | Two field orders produce different documents |
| `JCS-C008` | A different arguments document produces a different fingerprint | VERIFIED by unit test | Two argument documents collide |
| `JCS-C009` | The fingerprint carries an explicit algorithm prefix and 64 lowercase hex characters | VERIFIED by unit test | The rendered form is a bare digest or another algorithm |
| `JCS-C010` | A fingerprint from another algorithm is refused rather than compared | VERIFIED by unit test | `md5:...` parses |
| `JCS-C011` | The digest is computed from canonical bytes, not from `serde_json`'s default output | VERIFIED by unit test | Reordering input fields changes the digest |
| `JCS-C012` | A reclassified **effect** or **risk** moves the fingerprint though the identity is identical | VERIFIED by unit test | Omitting `effects` and `risk` from the envelope leaves the two fingerprints equal |
| `JCS-C013` | The same effects **set** produces one fingerprint however the list was ordered or repeated | VERIFIED by unit test | Concatenating the spellings instead of joining them makes the orders differ |

## Test Plan

### Deterministic Tests

- `canonical.rs` unit tests: the RFC §3.2.3 sample ordering (including the non-ASCII
  key), whitespace absence, the escape table, determinism, and input-order
  independence.
- `ActionDigest` parse/display round-trip, and the refusal of an alien algorithm.
- A fingerprint computation test asserting two differently-ordered inputs of the same
  action produce **one** digest, and a one-character argument change produces another.

### Contract Fixtures

- The canonical document for the RFC's own §3.2.3 sample, asserted as a literal so
  the expected value is visible in the test rather than derived from the code.

### Gated Live Tests

None. There is no external service. The RFC is the contract and it is offline.

## Operational Readiness

- **Diagnostics:** a mismatch reports `approval.fingerprint_mismatch`, which already
  exists and already maps to a stable code and a `409`. The computation adds no new
  failure mode to report.
- **Observability:** the fingerprint is already on the wire as
  `ApprovalView::action_fingerprint`. No log line carries the arguments, and none is
  added — a fingerprint is a one-way digest of them, which is what makes it safe to
  display.
- **Migration:** none. The stored column already holds `sha256:...` text, and no
  rows exist in any shipped profile because nothing ever produced one.

## Open Questions

- **A general-purpose JCS function for third-party documents** is deliberately absent.
  Adding one means implementing ECMAScript number serialization, and that should be a
  separate evidence note with a cross-language vector suite rather than an extension
  of this one.
- **Cross-language vectors for the string-only subset.** The contract's test list
  asks for "cross-language fingerprint vectors", and this note covers only the Rust
  side. A JavaScript client could be checked against the same fixture, and that is the
  next increment rather than part of this one.
- **Whether the envelope's field set is complete.** The contract lists "material
  content/artifact hashes" and "target connector account/resources", and neither has a
  producer yet (approval creation has no caller). The field list will grow with the
  executor, and each addition is a new digest input rather than a new rule.

## Change Log

| Date | Change | Evidence |
| --- | --- | --- |
| 2026-09-27 | Initial note. RFC 8785 adopted for the string-only subset; numbers refused by construction; SHA-256 via the already-reviewed `sha2` `0.10.9` | RFC 8785 §3.2.1–§3.2.4 read in full; Appendix B read to justify the number refusal; `TLS-005`'s recorded gap closed |
| 2026-09-27 | **`effects` and `risk` added to the envelope**, found by reading the contract's own input list against the implementation: the identity fingerprints the input schema only, so a reclassification kept its identity *and* its fingerprint, and an approval granted for a read would have authorized a delete. The effects list is also canonicalized as a set | Both omissions **falsified** — 3 domain tests plus 1 digest test fail; the set-canonicalization falsified separately |
