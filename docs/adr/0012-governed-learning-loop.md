# ADR-0012: The Learning Loop Is Governed, Durable, and Deny-by-Default

Status: PROPOSED
Date: 2026-10-01
Deciders: <owners>
Supersedes: None
Superseded by: None

## Context

JARVIS already owns a memory subsystem
([ADR-0006](0006-jarvis-owned-memory.md),
[memory-context.md](../architecture/memory-context.md)) whose principle is that
durable memory is a typed claim with provenance, not a transcript index. The
memory architecture names a **procedural** class and the tool fabric names
**skills** as "versioned orchestration instructions or workflow templates that
compose tools". Neither has an owning requirement, contract, milestone, or test.

The result is a product gap rather than a design gap. A system that cannot learn a
procedure from experience, improve it during use, and reuse it deliberately is a
tool assistant rather than an agent that grows. Peer systems ship this capability:
the upstream study records one whose entire identity is a closed learning loop,
where the agent authors its own procedural memory after a complex task and a
background review consolidates lessons at turn end. Its own documentation is
honest about the cost of shipping the loop before the guards — its memory is
bounded by a character budget the agent must curate, its stores are explicitly
single-writer ("one agent per Hermes home"), and its learning writes are gated by
a boolean rather than by the product's authorization model.

Copying that shape would import both the capability and the cost. Two properties
of JARVIS make a different shape possible and *better*:

- the canonical tool fabric already routes **every** side effect through
  resolve → validate → authorize → policy → approval → idempotency → execute →
  verify → persist ([tool-fabric.md](../architecture/tool-fabric.md)), and
- the durable workflow engine ([workflows-events.md](../architecture/workflows-events.md),
  [ADR-0008](0008-native-workflows-before-temporal.md)) already persists
  multi-step work across restarts, waits, and failures.

A learning write is a side effect. A learning review is multi-step durable work.
Both already have an owning subsystem; what is missing is the decision that binds
them, so that "the agent learned something" is a governed event rather than an
unguarded file write.

## Decision

Agent learning is a **governed, durable subsystem**, not a side effect of a
conversation turn.

1. **A learning write is a canonical tool call.** Creating, patching, or deleting
   a skill, or promoting a durable memory, is a tool with typed input and output,
   a stable identity, declared effects and risk, a timeout, and an idempotency
   key. It passes through the one execution path with no fast path, exactly like
   any other write. There is no privileged "memory writer" or "skill writer" that
   bypasses policy.

2. **A learning review is a durable workflow.** The post-turn review that extracts
   candidate lessons is a persisted workflow run, not an in-memory fork. It is
   resumable after a crash, bounded by an explicit input-token and wall-clock
   budget, and cancellable. A learning review that is interrupted is recovered to
   an explicit resumable or failed state like any other run.

3. **Learning is deny-by-default.** With no grant, a run cannot author a skill or
   promote a memory. Enabling learning is an explicit workspace-scoped grant, and
   the write policy for each class is explicit rather than assumed.

4. **A skill can narrow authority, never widen it.** Loading a skill may remove
   tools from the model's catalog for that call. It can never add a tool the
   principal's grants did not already authorize, raise a risk ceiling, relax an
   approval requirement, or change an effect classification. This restates and
   extends the rule the tool fabric already carries.

5. **A skill binds to an implementation, not a name.** An installed skill carries
   a content hash and a declared capability set. A grant or approval recorded
   against a skill names its exact content hash and the schema fingerprints of the
   tools it declares; changing the procedure or the tools requires a new grant,
   for the same reason `ACC-024` binds tool approvals to source and schema
   identity.

6. **Learning never persists hidden reasoning.** A review stores concise decisions,
   evidence, pitfalls, and user-visible summaries. It never stores or derives from
   hidden chain-of-thought, consistent with `FR-RUN-005`.

7. **Every learning write is inspectable, correctable, and revocable.** A user can
   see what was learned, why, and from which run; edit or delete it; disable
   learning for a workspace; and export the learned set, consistent with
   `FR-MEM-003`.

8. **Cost is bounded and visible.** A review routes to a cheaper auxiliary model
   when configured, reuses the parent model's prompt-cache prefix when it does not,
   and is capped by a cumulative replayed-input-token budget. Learning must not be
   a silent, unbounded cost center.

## Consequences

### Positive

- Learning becomes trustworthy enough to grant, because it runs through the same
  policy, approval, and idempotency path as every other effect.
- Interrupted learning resumes rather than being lost, because the review is a
  durable workflow run.
- The capability that a peer system uses to justify its identity is available
  without inheriting its single-writer store, unbounded-memory, or boolean-gate
  compromises.
- Progressive skill loading keeps the context budget honest: a skill index is
  cheap, a full procedure loads only when a task needs it, and a reference file
  loads only when the procedure needs it.

### Negative

- The first learning slice is larger than "write a file after a turn", because it
  depends on the tool fabric and the workflow engine existing first. This is
  deliberate: the guards are the feature.
- A governed learning write costs more than an ungoverned one — a policy decision,
  possibly an approval record, and an idempotency row.
- Requiring a new grant when a skill's content hash changes is more friction than
  silently accepting a re-pointed implementation.

### Risks and Mitigations

| Risk | Mitigation/evidence |
| --- | --- |
| A learned skill smuggles an instruction that widens authority. | Decision 4 plus `ACC-085`: a skill cannot add a tool, raise a ceiling, or relax approval; asserted against a malicious fixture. |
| An approved skill is silently replaced by an expanded one. | Decision 5 plus `ACC-086`: a changed content hash or capability set requires a new grant; asserted as the expansion case. |
| A review writes sensitive content to durable state without confirmation. | Decisions 6 and 7 plus `ACC-087`: sensitive candidates require confirmation and hidden reasoning is never persisted. |
| Runaway review cost on a busy host. | Decision 8 plus `ACC-089`: a cumulative input-token cap stops the review, and the figure is reported. |
| Learning survives as an unguarded parallel writer. | Every write is a canonical tool call (decision 1) proven by `ACC-084`, which asserts there is no path that skips policy. |

## Alternatives Considered

### Ungoverned post-turn file write (the peer shape)

A background hook writes `MEMORY.md`/skill files directly at turn end, gated by a
boolean. Rejected: it introduces a second durable writer with no policy, no
approval, and no idempotency, which contradicts
[ADR-0004](0004-canonical-tool-policy-and-mcp.md) and the single-execution-path
rule. The boolean gate is also weaker than an approval record: it cannot bind an
exact action, expire, or carry a scope.

### A separate learning service or store

Rejected: it would duplicate memory provenance, grants, and audit, and would make
"what did JARVIS learn and why" answerable from two places. Learning is a class of
memory and a consumer of tools, not a new subsystem with its own database.

### Do nothing until after v1

Rejected as a *design* decision, accepted as a *scheduling* one. The
requirement, contract, milestone placement, and acceptance scenarios are recorded
now so the primitives are built to support the loop; implementation is scheduled
after the tool fabric and workflow engine, and is not part of the v1 scope.

## Verification

- [`FR-SKL-001` through `FR-SKL-005`](../../PRODUCT.md) own the requirements;
  [`NFR-SKL-001`](../../PRODUCT.md) owns the authority-narrowing invariant.
- [skill-contract.md](../contracts/skill-contract.md) defines the wire and
  persistence shape.
- `ACC-084` through `ACC-089` in
  [acceptance.md](../testing/acceptance.md) are the stable scenarios;
  `MEM-011`..`MEM-013`, `TLS-016`, `PRD-010`, and `BRN-082` in
  [TODO.md](../../TODO.md) own the work.
- The invariant of decision 4 is falsified by attempting to widen a grant through
  a loaded skill; the invariant of decision 5 is falsified by swapping a tool
  implementation behind an unchanged skill name.
